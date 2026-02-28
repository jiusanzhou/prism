// Package config holds the admin service configuration and
// compatible types for reading the shared prism.yaml.
package config

import (
	"fmt"
	"os"
	"strings"
	"time"

	"gopkg.in/yaml.v3"
)

// AdminConfig is the admin-specific configuration.
type AdminConfig struct {
	ListenAddr string           `yaml:"listen_addr" json:"listen_addr"`
	GatewayURL string           `yaml:"gateway_url" json:"gateway_url"`
	ConfigPath string           `yaml:"config_path" json:"config_path"`
	RedisAddr  string           `yaml:"redis_addr" json:"redis_addr"`
	BasicAuth  *BasicAuthConfig `yaml:"basic_auth,omitempty" json:"basic_auth,omitempty"`
}

// BasicAuthConfig for optional basic auth.
type BasicAuthConfig struct {
	Username string `yaml:"username" json:"username"`
	Password string `yaml:"password" json:"password"`
}

// ---------- Prism gateway-compatible config types ----------

// PrismConfig mirrors the gateway's Config so we can read prism.yaml.
type PrismConfig struct {
	Server    ServerConfig              `yaml:"server" json:"server"`
	Routing   RoutingConfig             `yaml:"routing" json:"routing"`
	Providers map[string]ProviderConfig `yaml:"providers" json:"providers"`
	Models    []ModelConfig             `yaml:"models" json:"models"`
	Variants  []VariantConfig           `yaml:"variants" json:"variants"`
	Pools     []PoolConfig              `yaml:"pools" json:"pools"`
	Storage   StorageConfig             `yaml:"storage" json:"storage"`
}

// ServerConfig defines listen addresses.
type ServerConfig struct {
	HTTPPort  int `yaml:"http_port" json:"http_port"`
	GRPCPort  int `yaml:"grpc_port" json:"grpc_port"`
	AdminPort int `yaml:"admin_port" json:"admin_port"`
}

// RoutingConfig defines default routing behavior.
type RoutingConfig struct {
	DefaultStrategy string   `yaml:"default_strategy" json:"default_strategy"`
	DefaultPools    []string `yaml:"default_pools" json:"default_pools"`

	Balanced *BalancedConfig `yaml:"balanced,omitempty" json:"balanced,omitempty"`
	SLO      *SLOConfig      `yaml:"slo,omitempty" json:"slo,omitempty"`

	Analyzer   AnalyzerConfig   `yaml:"analyzer" json:"analyzer"`
	Resilience ResilienceConfig `yaml:"resilience" json:"resilience"`
}

// BalancedConfig for the balanced strategy.
type BalancedConfig struct {
	CostWeight    float32 `yaml:"cost_weight" json:"cost_weight"`
	QualityWeight float32 `yaml:"quality_weight" json:"quality_weight"`
	LatencyWeight float32 `yaml:"latency_weight" json:"latency_weight"`
}

// SLOConfig for SLO-first strategy.
type SLOConfig struct {
	MaxLatencyMs int     `yaml:"max_latency_ms" json:"max_latency_ms"`
	MaxTTFTMs    int     `yaml:"max_ttft_ms" json:"max_ttft_ms"`
	MinQuality   float32 `yaml:"min_quality" json:"min_quality"`
}

// AnalyzerConfig controls the request analyzer.
type AnalyzerConfig struct {
	Tier      string `yaml:"tier" json:"tier"`
	ModelPath string `yaml:"model_path,omitempty" json:"model_path,omitempty"`
}

// ResilienceConfig for retry / fallback / circuit breaker.
type ResilienceConfig struct {
	MaxRetries     int           `yaml:"max_retries" json:"max_retries"`
	RetryScope     string        `yaml:"retry_scope" json:"retry_scope"`
	RetryBackoff   string        `yaml:"retry_backoff" json:"retry_backoff"`
	TimeoutConnect time.Duration `yaml:"timeout_connect" json:"timeout_connect"`
	TimeoutRequest time.Duration `yaml:"timeout_request" json:"timeout_request"`
	TimeoutTTFT    time.Duration `yaml:"timeout_ttft" json:"timeout_ttft"`
}

// ProviderConfig for a single provider.
type ProviderConfig struct {
	Type          string `yaml:"type" json:"type"`
	APIKey        string `yaml:"api_key" json:"api_key"`
	BaseURL       string `yaml:"base_url" json:"base_url"`
	TimeoutMs     int    `yaml:"timeout_ms" json:"timeout_ms"`
	MaxConcurrent int    `yaml:"max_concurrent" json:"max_concurrent"`
}

// ModelConfig describes a model known to the routing engine.
type ModelConfig struct {
	ID           string             `yaml:"id" json:"id"`
	DisplayName  string             `yaml:"display_name" json:"display_name"`
	QualityTier  string             `yaml:"quality_tier" json:"quality_tier"`
	Capabilities CapabilitiesConfig `yaml:"capabilities" json:"capabilities"`
	Pricing      PricingConfig      `yaml:"pricing" json:"pricing"`
	Specialties  []string           `yaml:"specialties" json:"specialties"`
}

// CapabilitiesConfig describes what a model supports.
type CapabilitiesConfig struct {
	MaxContextWindow        int  `yaml:"max_context_window" json:"max_context_window"`
	MaxOutputTokens         int  `yaml:"max_output_tokens" json:"max_output_tokens"`
	SupportsStreaming        bool `yaml:"supports_streaming" json:"supports_streaming"`
	SupportsFunctionCalling bool `yaml:"supports_function_calling" json:"supports_function_calling"`
	SupportsVision          bool `yaml:"supports_vision" json:"supports_vision"`
	SupportsJSONMode        bool `yaml:"supports_json_mode" json:"supports_json_mode"`
	SupportsReasoning       bool `yaml:"supports_reasoning" json:"supports_reasoning"`
}

// PricingConfig for token costs.
type PricingConfig struct {
	InputPer1MTokens  float64 `yaml:"input_per_1m_tokens" json:"input_per_1m_tokens"`
	OutputPer1MTokens float64 `yaml:"output_per_1m_tokens" json:"output_per_1m_tokens"`
}

// VariantConfig ties a model to a provider.
type VariantConfig struct {
	ID         string `yaml:"id" json:"id"`
	ModelID    string `yaml:"model_id" json:"model_id"`
	ProviderID string `yaml:"provider_id" json:"provider_id"`
}

// PoolConfig groups variants for routing.
type PoolConfig struct {
	ID       string   `yaml:"id" json:"id"`
	Name     string   `yaml:"name" json:"name"`
	Variants []string `yaml:"variants" json:"variants"`
}

// StorageConfig for Redis and PostgreSQL.
type StorageConfig struct {
	Redis    RedisConfig    `yaml:"redis" json:"redis"`
	Postgres PostgresConfig `yaml:"postgres" json:"postgres"`
}

// RedisConfig for Redis connection.
type RedisConfig struct {
	Addr string `yaml:"addr" json:"addr"`
	DB   int    `yaml:"db" json:"db"`
}

// PostgresConfig for PostgreSQL connection.
type PostgresConfig struct {
	DSN string `yaml:"dsn" json:"dsn"`
}

// DefaultAdminConfig returns an AdminConfig with sensible defaults.
func DefaultAdminConfig() *AdminConfig {
	return &AdminConfig{
		ListenAddr: ":8081",
		GatewayURL: "http://localhost:8080",
		ConfigPath: "prism.yaml",
	}
}

// LoadPrismConfig reads a prism.yaml and returns the parsed config.
func LoadPrismConfig(path string) (*PrismConfig, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("config: failed to read %s: %w", path, err)
	}

	expanded := os.ExpandEnv(string(data))

	cfg := &PrismConfig{
		Server: ServerConfig{HTTPPort: 8080, AdminPort: 8081},
		Routing: RoutingConfig{
			DefaultStrategy: "balanced",
			DefaultPools:    []string{"default"},
		},
		Providers: map[string]ProviderConfig{},
	}
	if err := yaml.Unmarshal([]byte(expanded), cfg); err != nil {
		return nil, fmt.Errorf("config: failed to parse %s: %w", path, err)
	}

	return cfg, nil
}

// SanitizeConfig returns a deep copy of the PrismConfig with all
// sensitive fields (API keys, passwords, DSNs) masked.
func SanitizeConfig(cfg *PrismConfig) *PrismConfig {
	// Marshal then unmarshal for a quick deep copy.
	data, _ := yaml.Marshal(cfg)
	out := &PrismConfig{}
	_ = yaml.Unmarshal(data, out)

	for id, p := range out.Providers {
		if p.APIKey != "" {
			p.APIKey = maskValue(p.APIKey)
		}
		out.Providers[id] = p
	}
	if out.Storage.Postgres.DSN != "" {
		out.Storage.Postgres.DSN = "***"
	}
	return out
}

// maskValue replaces most of a value with *** but keeps the first/last few chars.
func maskValue(s string) string {
	if len(s) <= 8 {
		return "***"
	}
	if strings.HasPrefix(s, "sk-") || strings.HasPrefix(s, "${") {
		return "***"
	}
	return s[:3] + "***" + s[len(s)-3:]
}
