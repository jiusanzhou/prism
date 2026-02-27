package config

import (
	"encoding/json"
	"fmt"
)

// ToRouterConfigJSON converts the gateway config into JSON suitable for prism_init (Rust FFI).
// It auto-generates endpoints and accounts from provider configs so users don't need to
// specify them manually.
func (c *Config) ToRouterConfigJSON() (string, error) {
	type authConfig struct {
		ApiKey *apiKeyAuth `json:"ApiKey,omitempty"`
	}

	type rustProvider struct {
		ID      string      `json:"id"`
		Name    string      `json:"name"`
		BaseURL string      `json:"base_url"`
		Auth    *authConfig `json:"auth,omitempty"`
		Status  string      `json:"status"`
	}

	type rustEndpoint struct {
		ID         string `json:"id"`
		ProviderID string `json:"provider_id"`
		Status     string `json:"status"`
	}

	type rustAccount struct {
		ID         string      `json:"id"`
		EndpointID string      `json:"endpoint_id"`
		Auth       *authConfig `json:"auth,omitempty"`
		Status     string      `json:"status"`
	}

	type rustCapabilities struct {
		MaxContextWindow      int  `json:"max_context_window"`
		MaxOutputTokens       int  `json:"max_output_tokens"`
		SupportsStreaming      bool `json:"supports_streaming"`
		SupportsFunctionCalling bool `json:"supports_function_calling"`
		SupportsVision        bool `json:"supports_vision"`
		SupportsJSONMode      bool `json:"supports_json_mode"`
		SupportsReasoning     bool `json:"supports_reasoning"`
	}

	type rustPricing struct {
		InputPer1MTokens  float64 `json:"input_per_1m_tokens"`
		OutputPer1MTokens float64 `json:"output_per_1m_tokens"`
	}

	type rustModel struct {
		ID           string           `json:"id"`
		DisplayName  string           `json:"display_name"`
		QualityTier  string           `json:"quality_tier"`
		Capabilities rustCapabilities `json:"capabilities"`
		Pricing      rustPricing      `json:"pricing"`
		Specialties  []string         `json:"specialties"`
	}

	type rustPerformance struct {
		P50LatencyMs float64 `json:"p50_latency_ms"`
		P95LatencyMs float64 `json:"p95_latency_ms"`
		P99LatencyMs float64 `json:"p99_latency_ms"`
		TTFTP50Ms    float64 `json:"ttft_p50_ms"`
		ThroughputTPS float64 `json:"throughput_tps"`
		ErrorRate    float64 `json:"error_rate"`
		CurrentQPS   float64 `json:"current_qps"`
		QueueDepth   int     `json:"queue_depth"`
	}

	type rustVariant struct {
		ID          string          `json:"id"`
		ModelID     string          `json:"model_id"`
		EndpointID  string          `json:"endpoint_id"`
		AccountID   string          `json:"account_id"`
		Performance *rustPerformance `json:"performance,omitempty"`
	}

	type rustPool struct {
		ID       string   `json:"id"`
		Name     string   `json:"name"`
		Variants []string `json:"variants"`
	}

	type rustConfig struct {
		Providers []rustProvider `json:"providers"`
		Endpoints []rustEndpoint `json:"endpoints"`
		Accounts  []rustAccount  `json:"accounts"`
		Models    []rustModel    `json:"models"`
		Variants  []rustVariant  `json:"variants"`
		Pools     []rustPool     `json:"pools"`
	}

	rc := rustConfig{}

	// Build providers, endpoints, and accounts from config
	for id, pcfg := range c.Providers {
		var auth *authConfig
		if pcfg.APIKey != "" {
			auth = &authConfig{
				ApiKey: &apiKeyAuth{
					Key:    pcfg.APIKey,
					Header: "Authorization",
				},
			}
		}

		rc.Providers = append(rc.Providers, rustProvider{
			ID:      id,
			Name:    id, // use ID as display name
			BaseURL: pcfg.BaseURL,
			Auth:    auth,
			Status:  "Healthy",
		})

		epID := id + "-ep"
		rc.Endpoints = append(rc.Endpoints, rustEndpoint{
			ID:         epID,
			ProviderID: id,
			Status:     "Healthy",
		})

		acctID := id + "-acct"
		rc.Accounts = append(rc.Accounts, rustAccount{
			ID:         acctID,
			EndpointID: epID,
			Auth:       auth,
			Status:     "Healthy",
		})
	}

	// Build models
	for _, m := range c.Models {
		rc.Models = append(rc.Models, rustModel{
			ID:          m.ID,
			DisplayName: m.DisplayName,
			QualityTier: m.QualityTier,
			Capabilities: rustCapabilities{
				MaxContextWindow:      m.Capabilities.MaxContextWindow,
				MaxOutputTokens:       m.Capabilities.MaxOutputTokens,
				SupportsStreaming:      m.Capabilities.SupportsStreaming,
				SupportsFunctionCalling: m.Capabilities.SupportsFunctionCalling,
				SupportsVision:        m.Capabilities.SupportsVision,
				SupportsJSONMode:      m.Capabilities.SupportsJSONMode,
				SupportsReasoning:     m.Capabilities.SupportsReasoning,
			},
			Pricing: rustPricing{
				InputPer1MTokens:  m.Pricing.InputPer1MTokens,
				OutputPer1MTokens: m.Pricing.OutputPer1MTokens,
			},
			Specialties: m.Specialties,
		})
	}

	// Build variants — map provider_id to endpoint_id/account_id
	for _, v := range c.Variants {
		epID := v.ProviderID + "-ep"
		acctID := v.ProviderID + "-acct"

		rc.Variants = append(rc.Variants, rustVariant{
			ID:         v.ID,
			ModelID:    v.ModelID,
			EndpointID: epID,
			AccountID:  acctID,
		})
	}

	// Build pools
	for _, p := range c.Pools {
		rc.Pools = append(rc.Pools, rustPool{
			ID:       p.ID,
			Name:     p.Name,
			Variants: p.Variants,
		})
	}

	data, err := json.Marshal(rc)
	if err != nil {
		return "", fmt.Errorf("config: failed to marshal router config JSON: %w", err)
	}

	return string(data), nil
}

// apiKeyAuth matches the Rust Auth::ApiKey enum variant.
type apiKeyAuth struct {
	Key    string `json:"key"`
	Header string `json:"header"`
}
