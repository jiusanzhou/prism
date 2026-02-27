package discovery

import (
	"fmt"
	"log"
	"sort"
	"strings"

	"github.com/aspect-build/prism-gateway/internal/catalog"
	"github.com/aspect-build/prism-gateway/internal/config"
)

// AutoConfig generates a complete routing config from minimal user input.
//
// When users provide only provider configs (with API keys), AutoConfig fills in
// the models, variants, and pools sections automatically using the built-in catalog.
//
// It also accepts discovered models (from API calls) to cross-reference with the
// catalog for enrichment.
func AutoConfig(
	providers map[string]config.ProviderConfig,
	discovered map[string]*ProviderInfo,
) (*config.Config, error) {
	if len(providers) == 0 {
		return nil, fmt.Errorf("auto_config: no providers configured")
	}

	cat := catalog.New()
	cfg := config.DefaultConfig()
	cfg.Providers = providers

	// Track which models we've added (by canonical ID) to avoid duplicates
	addedModels := make(map[string]bool)
	var variants []config.VariantConfig
	var poolVariants []string

	// Sort provider IDs for deterministic output
	providerIDs := make([]string, 0, len(providers))
	for id := range providers {
		providerIDs = append(providerIDs, id)
	}
	sort.Strings(providerIDs)

	for _, providerID := range providerIDs {
		pcfg := providers[providerID]
		providerType := pcfg.Type
		if providerType == "" {
			providerType = "openai-compatible"
		}

		// Determine which models to use for this provider
		var modelIDs []string

		// If we have discovered models, use those (filtered through catalog)
		if info, ok := discovered[providerID]; ok && len(info.AvailableModels) > 0 {
			for _, dm := range info.AvailableModels {
				if dm.Available {
					modelIDs = append(modelIDs, dm.ID)
				}
			}
		} else {
			// Fall back to catalog models for this provider type
			catalogProvider := providerType
			if catalogProvider == "openai-compatible" {
				catalogProvider = "openai"
			}
			for _, entry := range cat.ModelsForProvider(catalogProvider) {
				modelIDs = append(modelIDs, entry.ID)
			}
		}

		// Sort for deterministic output
		sort.Strings(modelIDs)

		for _, modelID := range modelIDs {
			// Look up in catalog for enrichment
			entry := cat.Lookup(modelID)

			if entry != nil {
				// Use the canonical ID
				canonicalID := entry.ID

				// Add model config if not already added
				if !addedModels[canonicalID] {
					addedModels[canonicalID] = true

					mc := config.ModelConfig{
						ID:          canonicalID,
						DisplayName: entry.DisplayName,
						QualityTier: entry.QualityTier,
						Capabilities: config.CapabilitiesConfig{
							MaxContextWindow:        entry.Capabilities.MaxContextWindow,
							MaxOutputTokens:         entry.Capabilities.MaxOutputTokens,
							SupportsStreaming:        entry.Capabilities.SupportsStreaming,
							SupportsFunctionCalling: entry.Capabilities.SupportsFunctionCalling,
							SupportsVision:          entry.Capabilities.SupportsVision,
							SupportsJSONMode:        entry.Capabilities.SupportsJSONMode,
							SupportsReasoning:       entry.Capabilities.SupportsReasoning,
						},
						Pricing: config.PricingConfig{
							InputPer1MTokens:  entry.Pricing.InputPer1MTokens,
							OutputPer1MTokens: entry.Pricing.OutputPer1MTokens,
						},
						Specialties: entry.Specialties,
					}
					cfg.Models = append(cfg.Models, mc)
				}

				// Create variant: one per model-provider pair
				variantID := fmt.Sprintf("%s@%s", canonicalID, providerID)
				variants = append(variants, config.VariantConfig{
					ID:         variantID,
					ModelID:    canonicalID,
					ProviderID: providerID,
				})
				poolVariants = append(poolVariants, variantID)
			} else {
				// Model not in catalog — still add it with minimal info
				log.Printf("[auto_config] WARNING: model %q from provider %q not found in catalog", modelID, providerID)

				if !addedModels[modelID] {
					addedModels[modelID] = true
					cfg.Models = append(cfg.Models, config.ModelConfig{
						ID:          modelID,
						DisplayName: modelID,
						QualityTier: "B", // default unknown tier
						Capabilities: config.CapabilitiesConfig{
							SupportsStreaming: true,
						},
					})
				}

				variantID := fmt.Sprintf("%s@%s", modelID, providerID)
				variants = append(variants, config.VariantConfig{
					ID:         variantID,
					ModelID:    modelID,
					ProviderID: providerID,
				})
				poolVariants = append(poolVariants, variantID)
			}
		}
	}

	cfg.Variants = variants

	// Create "default" pool with all variants
	if len(poolVariants) > 0 {
		cfg.Pools = []config.PoolConfig{
			{
				ID:       "default",
				Name:     "Default Pool (auto-generated)",
				Variants: poolVariants,
			},
		}
	}

	cfg.Routing.DefaultPools = []string{"default"}

	return cfg, nil
}

// AutoConfigFromProviderConfigs is a convenience function that generates config
// from provider configs without running discovery (uses catalog only).
func AutoConfigFromProviderConfigs(providers map[string]config.ProviderConfig) (*config.Config, error) {
	return AutoConfig(providers, nil)
}

// ShouldAutoConfig checks if auto-config should be used based on the current config.
// Returns true if providers are configured but models/variants/pools are empty.
func ShouldAutoConfig(cfg *config.Config) bool {
	return len(cfg.Providers) > 0 &&
		len(cfg.Models) == 0 &&
		len(cfg.Variants) == 0 &&
		len(cfg.Pools) == 0
}

// EnrichConfig takes an existing config and fills in any missing models/variants/pools
// using auto-discovery and the catalog.
func EnrichConfig(cfg *config.Config, discovered map[string]*ProviderInfo) (*config.Config, error) {
	if !ShouldAutoConfig(cfg) {
		return cfg, nil // Config already has models defined
	}

	autoCfg, err := AutoConfig(cfg.Providers, discovered)
	if err != nil {
		return cfg, fmt.Errorf("auto_config: %w", err)
	}

	// Merge auto-generated into existing config
	cfg.Models = autoCfg.Models
	cfg.Variants = autoCfg.Variants
	cfg.Pools = autoCfg.Pools

	if len(cfg.Routing.DefaultPools) == 0 {
		cfg.Routing.DefaultPools = autoCfg.Routing.DefaultPools
	}

	return cfg, nil
}

// FilterCatalogModels returns only catalog models that match a specific provider type,
// useful for filtering discovered models against the catalog.
func FilterCatalogModels(providerType string) []string {
	cat := catalog.New()
	catalogProvider := providerType
	if catalogProvider == "openai-compatible" {
		catalogProvider = "openai"
	}

	var result []string
	for _, entry := range cat.ModelsForProvider(catalogProvider) {
		result = append(result, entry.ID)
	}
	sort.Strings(result)
	return result
}

// IsChatModel checks if a model ID looks like a chat/completion model
// (vs. embedding, whisper, tts, etc.) based on common naming patterns.
func IsChatModel(modelID string) bool {
	lower := strings.ToLower(modelID)

	// Known non-chat model prefixes
	nonChat := []string{
		"text-embedding", "embedding", "whisper", "tts", "dall-e",
		"davinci", "babbage", "curie", "ada",
		"text-moderation", "text-search", "text-similarity",
		"code-search", "code-davinci",
	}
	for _, prefix := range nonChat {
		if strings.HasPrefix(lower, prefix) {
			return false
		}
	}

	// Known chat model patterns
	chatPatterns := []string{
		"gpt-", "o1", "o3", "claude", "deepseek",
		"gemini", "mistral", "llama", "command",
	}
	for _, pattern := range chatPatterns {
		if strings.Contains(lower, pattern) {
			return true
		}
	}

	// If in catalog, it's a chat model
	cat := catalog.New()
	if cat.Lookup(modelID) != nil {
		return true
	}

	return false
}
