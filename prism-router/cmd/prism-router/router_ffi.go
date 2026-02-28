//go:build ffi

package main

import (
	"encoding/json"
	"log"

	"github.com/aspect-build/prism-router/internal/ffi"
)

// createRouter creates an FFI-backed Router using the Rust prism-core library.
func createRouter(cfg *Config) (*ffi.Router, error) {
	configJSON, err := toRouterConfigJSON(cfg)
	if err != nil {
		return nil, err
	}

	log.Printf("[router] Initializing FFI router with Rust prism-core...")
	r, err := ffi.NewFFIRouter(configJSON)
	if err != nil {
		return nil, err
	}

	log.Printf("[router] FFI router initialized (version: %s)", r.Version())
	return r, nil
}

type apiKeyAuthJSON struct {
	Key    string `json:"key"`
	Header string `json:"header"`
}

type authConfigJSON struct {
	ApiKey *apiKeyAuthJSON `json:"ApiKey,omitempty"`
}

// toRouterConfigJSON converts the config into JSON for prism_init.
// Mirrors prism-gateway/internal/config/loader.go.
func toRouterConfigJSON(cfg *Config) (string, error) {
	type rustProvider struct {
		ID        string          `json:"id"`
		Name      string          `json:"name"`
		BaseURL   string          `json:"base_url"`
		Auth      *authConfigJSON `json:"auth,omitempty"`
		Status    string          `json:"status"`
		Protocols []string        `json:"protocols,omitempty"`
	}
	type rustEndpoint struct {
		ID         string `json:"id"`
		ProviderID string `json:"provider_id"`
		Status     string `json:"status"`
	}
	type rustAccount struct {
		ID         string          `json:"id"`
		EndpointID string          `json:"endpoint_id"`
		Auth       *authConfigJSON `json:"auth,omitempty"`
		Status     string          `json:"status"`
	}
	type rustCapabilities struct {
		MaxContextWindow        int  `json:"max_context_window"`
		MaxOutputTokens         int  `json:"max_output_tokens"`
		SupportsStreaming        bool `json:"supports_streaming"`
		SupportsFunctionCalling bool `json:"supports_function_calling"`
		SupportsVision          bool `json:"supports_vision"`
		SupportsJSONMode        bool `json:"supports_json_mode"`
		SupportsReasoning       bool `json:"supports_reasoning"`
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
	type rustVariant struct {
		ID         string `json:"id"`
		ModelID    string `json:"model_id"`
		EndpointID string `json:"endpoint_id"`
		AccountID  string `json:"account_id"`
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

	for id, pcfg := range cfg.Providers {
		var auth *authConfigJSON
		if pcfg.APIKey != "" {
			auth = &authConfigJSON{
				ApiKey: &apiKeyAuthJSON{Key: pcfg.APIKey, Header: "Authorization"},
			}
		}
		rc.Providers = append(rc.Providers, rustProvider{
			ID: id, Name: id, BaseURL: pcfg.BaseURL, Auth: auth, Status: "Healthy", Protocols: pcfg.Protocols,
		})
		epID := id + "-ep"
		rc.Endpoints = append(rc.Endpoints, rustEndpoint{ID: epID, ProviderID: id, Status: "Healthy"})
		acctID := id + "-acct"
		rc.Accounts = append(rc.Accounts, rustAccount{ID: acctID, EndpointID: epID, Auth: auth, Status: "Healthy"})
	}

	for _, m := range cfg.Models {
		rc.Models = append(rc.Models, rustModel{
			ID: m.ID, DisplayName: m.DisplayName, QualityTier: m.QualityTier,
			Capabilities: rustCapabilities{
				MaxContextWindow:        m.Capabilities.MaxContextWindow,
				MaxOutputTokens:         m.Capabilities.MaxOutputTokens,
				SupportsStreaming:        m.Capabilities.SupportsStreaming,
				SupportsFunctionCalling: m.Capabilities.SupportsFunctionCalling,
				SupportsVision:          m.Capabilities.SupportsVision,
				SupportsJSONMode:        m.Capabilities.SupportsJSONMode,
				SupportsReasoning:       m.Capabilities.SupportsReasoning,
			},
			Pricing:     rustPricing{InputPer1MTokens: m.Pricing.InputPer1MTokens, OutputPer1MTokens: m.Pricing.OutputPer1MTokens},
			Specialties: m.Specialties,
		})
	}

	for _, v := range cfg.Variants {
		rc.Variants = append(rc.Variants, rustVariant{
			ID: v.ID, ModelID: v.ModelID, EndpointID: v.ProviderID + "-ep", AccountID: v.ProviderID + "-acct",
		})
	}

	for _, p := range cfg.Pools {
		rc.Pools = append(rc.Pools, rustPool{ID: p.ID, Name: p.Name, Variants: p.Variants})
	}

	data, err := json.Marshal(rc)
	if err != nil {
		return "", err
	}
	return string(data), nil
}
