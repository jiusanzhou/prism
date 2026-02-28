package main

import (
	"fmt"
	"os"

	"gopkg.in/yaml.v3"
)

// Config mirrors the prism-gateway config structure (subset needed for routing).
type Config struct {
	Server    ServerConfig               `yaml:"server"`
	Routing   RoutingConfig              `yaml:"routing"`
	Providers map[string]ProviderConfig  `yaml:"providers"`
	Models    []ModelConfig              `yaml:"models"`
	Variants  []VariantConfig            `yaml:"variants"`
	Pools     []PoolConfig               `yaml:"pools"`
}

type ServerConfig struct {
	HTTPPort int `yaml:"http_port"`
}

type RoutingConfig struct {
	DefaultStrategy string          `yaml:"default_strategy"`
	DefaultPools    []string        `yaml:"default_pools"`
	Balanced        *BalancedConfig `yaml:"balanced,omitempty"`
}

type BalancedConfig struct {
	CostWeight    float32 `yaml:"cost_weight"`
	QualityWeight float32 `yaml:"quality_weight"`
	LatencyWeight float32 `yaml:"latency_weight"`
}

type ProviderConfig struct {
	Type          string   `yaml:"type"`
	APIKey        string   `yaml:"api_key"`
	BaseURL       string   `yaml:"base_url"`
	TimeoutMs     int      `yaml:"timeout_ms"`
	MaxConcurrent int      `yaml:"max_concurrent"`
	Protocols     []string `yaml:"protocols"`
}

type ModelConfig struct {
	ID           string             `yaml:"id"`
	DisplayName  string             `yaml:"display_name"`
	QualityTier  string             `yaml:"quality_tier"`
	Capabilities CapabilitiesConfig `yaml:"capabilities"`
	Pricing      PricingConfig      `yaml:"pricing"`
	Specialties  []string           `yaml:"specialties"`
}

type CapabilitiesConfig struct {
	MaxContextWindow        int  `yaml:"max_context_window"`
	MaxOutputTokens         int  `yaml:"max_output_tokens"`
	SupportsStreaming        bool `yaml:"supports_streaming"`
	SupportsFunctionCalling bool `yaml:"supports_function_calling"`
	SupportsVision          bool `yaml:"supports_vision"`
	SupportsJSONMode        bool `yaml:"supports_json_mode"`
	SupportsReasoning       bool `yaml:"supports_reasoning"`
}

type PricingConfig struct {
	InputPer1MTokens  float64 `yaml:"input_per_1m_tokens"`
	OutputPer1MTokens float64 `yaml:"output_per_1m_tokens"`
}

type VariantConfig struct {
	ID         string `yaml:"id"`
	ModelID    string `yaml:"model_id"`
	ProviderID string `yaml:"provider_id"`
}

type PoolConfig struct {
	ID       string   `yaml:"id"`
	Name     string   `yaml:"name"`
	Variants []string `yaml:"variants"`
}

// loadConfig loads the prism configuration from YAML.
func loadConfig(path string) (*Config, error) {
	if path == "" {
		if p := os.Getenv("PRISM_CONFIG"); p != "" {
			path = p
		}
	}

	if path == "" {
		// Search default locations
		for _, p := range []string{"prism.yaml", "configs/prism.yaml", "/etc/prism/prism.yaml"} {
			if _, err := os.Stat(p); err == nil {
				path = p
				break
			}
		}
	}

	if path == "" {
		return defaultConfig(), nil
	}

	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("config: failed to read %s: %w", path, err)
	}

	expanded := os.ExpandEnv(string(data))

	cfg := defaultConfig()
	if err := yaml.Unmarshal([]byte(expanded), cfg); err != nil {
		return nil, fmt.Errorf("config: failed to parse %s: %w", path, err)
	}

	return cfg, nil
}

func defaultConfig() *Config {
	return &Config{
		Server: ServerConfig{HTTPPort: 9080},
		Routing: RoutingConfig{
			DefaultStrategy: "balanced",
			DefaultPools:    []string{"default"},
			Balanced: &BalancedConfig{
				CostWeight:    0.3,
				QualityWeight: 0.5,
				LatencyWeight: 0.2,
			},
		},
		Providers: map[string]ProviderConfig{},
	}
}
