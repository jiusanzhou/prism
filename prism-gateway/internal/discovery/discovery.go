// Package discovery provides auto-discovery of provider models and capabilities.
//
// It queries provider APIs (like OpenAI's /v1/models) to discover available models,
// then cross-references with the built-in catalog to enrich results with pricing,
// capabilities, and quality tier information.
package discovery

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/aspect-build/prism-gateway/internal/catalog"
)

// ProviderDiscovery auto-discovers models and capabilities from provider APIs.
type ProviderDiscovery struct {
	mu        sync.RWMutex
	providers map[string]*ProviderInfo
	client    *http.Client
	catalog   *catalog.ModelCatalog
}

// ProviderInfo holds information about a single provider including discovered data.
type ProviderInfo struct {
	ID   string
	Type string // "openai", "anthropic", "deepseek", "google"
	// BaseURL for API calls (with /v1 prefix for OpenAI-compatible)
	BaseURL string
	APIKey  string

	// Discovered info
	AvailableModels []DiscoveredModel
	AccountTier     string
	RateLimits      *DiscoveredRateLimits
	LastDiscovery   time.Time
	DiscoveryError  error
}

// DiscoveredModel represents a model discovered from a provider API.
type DiscoveredModel struct {
	ID        string
	OwnedBy  string
	Created   int64
	Available bool
}

// DiscoveredRateLimits holds rate limit info discovered from a provider.
type DiscoveredRateLimits struct {
	RPM int // requests per minute
	TPM int // tokens per minute
	RPD int // requests per day
}

// NewProviderDiscovery creates a new ProviderDiscovery instance.
func NewProviderDiscovery() *ProviderDiscovery {
	return &ProviderDiscovery{
		providers: make(map[string]*ProviderInfo),
		client: &http.Client{
			Timeout: 15 * time.Second,
		},
		catalog: catalog.New(),
	}
}

// AddProvider registers a provider for discovery.
func (d *ProviderDiscovery) AddProvider(info *ProviderInfo) {
	d.mu.Lock()
	defer d.mu.Unlock()
	d.providers[info.ID] = info
}

// DiscoverAll runs discovery for all registered providers.
// It's best-effort: failures are logged but don't prevent other providers from being discovered.
func (d *ProviderDiscovery) DiscoverAll(ctx context.Context) {
	d.mu.RLock()
	providers := make([]*ProviderInfo, 0, len(d.providers))
	for _, p := range d.providers {
		providers = append(providers, p)
	}
	d.mu.RUnlock()

	var wg sync.WaitGroup
	for _, p := range providers {
		wg.Add(1)
		go func(info *ProviderInfo) {
			defer wg.Done()
			d.discoverProvider(ctx, info)
		}(p)
	}
	wg.Wait()
}

// GetProvider returns a discovered provider's info.
func (d *ProviderDiscovery) GetProvider(id string) *ProviderInfo {
	d.mu.RLock()
	defer d.mu.RUnlock()
	return d.providers[id]
}

// GetAllProviders returns all registered providers.
func (d *ProviderDiscovery) GetAllProviders() map[string]*ProviderInfo {
	d.mu.RLock()
	defer d.mu.RUnlock()
	result := make(map[string]*ProviderInfo, len(d.providers))
	for k, v := range d.providers {
		result[k] = v
	}
	return result
}

func (d *ProviderDiscovery) discoverProvider(ctx context.Context, info *ProviderInfo) {
	switch info.Type {
	case "openai", "deepseek", "openai-compatible":
		err := d.discoverOpenAICompatible(ctx, info)
		if err != nil {
			log.Printf("[discovery] %s: API discovery failed: %v, falling back to catalog", info.ID, err)
			info.DiscoveryError = err
			d.fallbackToCatalog(info)
		} else {
			log.Printf("[discovery] %s: discovered %d models from API", info.ID, len(info.AvailableModels))
		}
	case "anthropic":
		// Anthropic doesn't have a models list API — use catalog
		d.fallbackToCatalog(info)
		log.Printf("[discovery] %s: using catalog data (%d models)", info.ID, len(info.AvailableModels))
	case "google":
		// Google Vertex/Gemini — use catalog for now
		d.fallbackToCatalog(info)
		log.Printf("[discovery] %s: using catalog data (%d models)", info.ID, len(info.AvailableModels))
	default:
		log.Printf("[discovery] %s: unknown provider type %q, skipping", info.ID, info.Type)
	}
	info.LastDiscovery = time.Now()
}

// discoverOpenAICompatible calls GET /v1/models for OpenAI-compatible providers.
func (d *ProviderDiscovery) discoverOpenAICompatible(ctx context.Context, info *ProviderInfo) error {
	baseURL := info.BaseURL
	if baseURL == "" {
		baseURL = "https://api.openai.com/v1"
	}
	url := strings.TrimRight(baseURL, "/") + "/models"

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return fmt.Errorf("create request: %w", err)
	}
	req.Header.Set("Authorization", "Bearer "+info.APIKey)

	resp, err := d.client.Do(req)
	if err != nil {
		return fmt.Errorf("request failed: %w", err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return fmt.Errorf("read response: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("API returned status %d: %s", resp.StatusCode, truncate(string(body), 200))
	}

	var modelsResp struct {
		Data []struct {
			ID      string `json:"id"`
			OwnedBy string `json:"owned_by"`
			Created int64  `json:"created"`
		} `json:"data"`
	}

	if err := json.Unmarshal(body, &modelsResp); err != nil {
		return fmt.Errorf("unmarshal response: %w", err)
	}

	info.AvailableModels = make([]DiscoveredModel, 0, len(modelsResp.Data))
	for _, m := range modelsResp.Data {
		info.AvailableModels = append(info.AvailableModels, DiscoveredModel{
			ID:        m.ID,
			OwnedBy:  m.OwnedBy,
			Created:   m.Created,
			Available: true,
		})
	}

	return nil
}

// fallbackToCatalog populates a provider's model list from the built-in catalog.
func (d *ProviderDiscovery) fallbackToCatalog(info *ProviderInfo) {
	// Map provider type to catalog provider name
	catalogProvider := info.Type
	if catalogProvider == "openai-compatible" {
		catalogProvider = "openai"
	}

	models := d.catalog.ModelsForProvider(catalogProvider)
	info.AvailableModels = make([]DiscoveredModel, 0, len(models))
	for _, m := range models {
		info.AvailableModels = append(info.AvailableModels, DiscoveredModel{
			ID:        m.ID,
			Available: true,
		})
	}
}

func truncate(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}
