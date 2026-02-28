// Package config holds the gateway configuration.
package config

import (
	"fmt"
	"os"
	"time"

	"github.com/aspect-build/prism-gateway/internal/abtesting"
	"gopkg.in/yaml.v3"
)

// Config is the top-level configuration for the Prism gateway.
type Config struct {
	Server      ServerConfig               `yaml:"server"`
	Routing     RoutingConfig              `yaml:"routing"`
	Providers   map[string]ProviderConfig  `yaml:"providers"`
	Models      []ModelConfig              `yaml:"models"`
	Variants    []VariantConfig            `yaml:"variants"`
	Pools       []PoolConfig               `yaml:"pools"`
	Storage     StorageConfig              `yaml:"storage"`
	Redis       *TopLevelRedisConfig       `yaml:"redis,omitempty"`
	Experiments []abtesting.Experiment     `yaml:"experiments"`
}

// TopLevelRedisConfig is the optional top-level Redis configuration for the state store.
type TopLevelRedisConfig struct {
	Addr     string `yaml:"addr"`
	Password string `yaml:"password"`
	DB       int    `yaml:"db"`
}

// ServerConfig defines listen addresses.
type ServerConfig struct {
	HTTPPort  int `yaml:"http_port"`
	GRPCPort  int `yaml:"grpc_port"`
	AdminPort int `yaml:"admin_port"`
}

// RoutingConfig defines default routing behavior.
type RoutingConfig struct {
	DefaultStrategy string   `yaml:"default_strategy"`
	DefaultPools    []string `yaml:"default_pools"`

	Balanced *BalancedConfig `yaml:"balanced,omitempty"`
	SLO      *SLOConfig      `yaml:"slo,omitempty"`

	Selector   SelectorConfig   `yaml:"selector"`
	Analyzer   AnalyzerConfig   `yaml:"analyzer"`
	Resilience ResilienceConfig `yaml:"resilience"`
}

// SelectorConfig selects which implementation to use at each routing layer.
type SelectorConfig struct {
	L1 string `yaml:"l1"` // "scored"
	L2 string `yaml:"l2"` // "default", "lowest_latency", "lowest_cost", "priority"
	L3 string `yaml:"l3"` // "affinity", "round_robin", "least_load"
	L4 string `yaml:"l4"` // "best", "weighted"
}

// BalancedConfig for the balanced strategy.
type BalancedConfig struct {
	CostWeight    float32 `yaml:"cost_weight"`
	QualityWeight float32 `yaml:"quality_weight"`
	LatencyWeight float32 `yaml:"latency_weight"`
}

// SLOConfig for SLO-first strategy.
type SLOConfig struct {
	MaxLatencyMs int     `yaml:"max_latency_ms"`
	MaxTTFTMs    int     `yaml:"max_ttft_ms"`
	MinQuality   float32 `yaml:"min_quality"`
}

// AnalyzerConfig controls the request analyzer.
type AnalyzerConfig struct {
	Tier      string `yaml:"tier"` // "rule", "feature", "model"
	ModelPath string `yaml:"model_path,omitempty"`
}

// ResilienceConfig for retry / fallback / circuit breaker.
type ResilienceConfig struct {
	MaxRetries     int           `yaml:"max_retries"`
	RetryScope     string        `yaml:"retry_scope"`
	RetryBackoff   string        `yaml:"retry_backoff"`
	TimeoutConnect time.Duration `yaml:"timeout_connect"`
	TimeoutRequest time.Duration `yaml:"timeout_request"`
	TimeoutTTFT    time.Duration `yaml:"timeout_ttft"`
}

// ProviderConfig for a single provider.
type ProviderConfig struct {
	Type          string   `yaml:"type"`
	APIKey        string   `yaml:"api_key"`
	BaseURL       string   `yaml:"base_url"`
	TimeoutMs     int      `yaml:"timeout_ms"`
	MaxConcurrent int      `yaml:"max_concurrent"`
	Protocols     []string `yaml:"protocols"` // e.g., ["openai"], ["anthropic"], ["openai", "anthropic"]
}

// ModelConfig describes a model known to the routing engine.
type ModelConfig struct {
	ID           string      `yaml:"id"`
	DisplayName  string            `yaml:"display_name"`
	QualityTier  string            `yaml:"quality_tier"`
	Capabilities CapabilitiesConfig `yaml:"capabilities"`
	Pricing      PricingConfig     `yaml:"pricing"`
	Specialties  []string          `yaml:"specialties"`
}

// CapabilitiesConfig describes what a model supports.
type CapabilitiesConfig struct {
	MaxContextWindow      int  `yaml:"max_context_window"`
	MaxOutputTokens       int  `yaml:"max_output_tokens"`
	SupportsStreaming      bool `yaml:"supports_streaming"`
	SupportsFunctionCalling bool `yaml:"supports_function_calling"`
	SupportsVision        bool `yaml:"supports_vision"`
	SupportsJSONMode      bool `yaml:"supports_json_mode"`
	SupportsReasoning     bool `yaml:"supports_reasoning"`
}

// PricingConfig for token costs.
type PricingConfig struct {
	InputPer1MTokens  float64 `yaml:"input_per_1m_tokens"`
	OutputPer1MTokens float64 `yaml:"output_per_1m_tokens"`
}

// VariantConfig ties a model to a provider.
type VariantConfig struct {
	ID         string `yaml:"id"`
	ModelID    string `yaml:"model_id"`
	ProviderID string `yaml:"provider_id"`
}

// PoolConfig groups variants for routing.
type PoolConfig struct {
	ID       string   `yaml:"id"`
	Name     string   `yaml:"name"`
	Variants []string `yaml:"variants"`
}

// StorageConfig for Redis and PostgreSQL.
type StorageConfig struct {
	Redis    RedisConfig    `yaml:"redis"`
	Postgres PostgresConfig `yaml:"postgres"`
}

// RedisConfig for Redis connection.
type RedisConfig struct {
	Addr string `yaml:"addr"`
	DB   int    `yaml:"db"`
}

// PostgresConfig for PostgreSQL connection.
type PostgresConfig struct {
	DSN string `yaml:"dsn"`
}

// DefaultConfig returns a config with sensible defaults.
func DefaultConfig() *Config {
	return &Config{
		Server: ServerConfig{
			HTTPPort:  8080,
			GRPCPort:  9090,
			AdminPort: 8081,
		},
		Routing: RoutingConfig{
			DefaultStrategy: "balanced",
			DefaultPools:    []string{"default"},
			Balanced: &BalancedConfig{
				CostWeight:    0.3,
				QualityWeight: 0.5,
				LatencyWeight: 0.2,
			},
			Analyzer: AnalyzerConfig{
				Tier: "rule",
			},
			Resilience: ResilienceConfig{
				MaxRetries:     2,
				RetryScope:     "cross_provider",
				RetryBackoff:   "exponential",
				TimeoutConnect: 3 * time.Second,
				TimeoutRequest: 30 * time.Second,
				TimeoutTTFT:    10 * time.Second,
			},
		},
		Providers: map[string]ProviderConfig{},
		Storage: StorageConfig{
			Redis: RedisConfig{Addr: "localhost:6379", DB: 0},
			Postgres: PostgresConfig{
				DSN: "postgres://prism:prism@localhost:5432/prism?sslmode=disable",
			},
		},
	}
}

// LoadConfig reads a YAML configuration file and returns a Config.
// String values containing ${VAR} are expanded using os.ExpandEnv.
func LoadConfig(path string) (*Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("config: failed to read %s: %w", path, err)
	}

	// Expand environment variables in the raw YAML text
	expanded := os.ExpandEnv(string(data))

	cfg := DefaultConfig()
	if err := yaml.Unmarshal([]byte(expanded), cfg); err != nil {
		return nil, fmt.Errorf("config: failed to parse %s: %w", path, err)
	}

	return cfg, nil
}

// LoadConfigFromEnv loads a config from the path specified by PRISM_CONFIG,
// or falls back to default search paths: ./prism.yaml, ./configs/prism.yaml.
func LoadConfigFromEnv() (*Config, error) {
	if p := os.Getenv("PRISM_CONFIG"); p != "" {
		return LoadConfig(p)
	}

	searchPaths := []string{
		"prism.yaml",
		"configs/prism.yaml",
	}

	for _, p := range searchPaths {
		if _, err := os.Stat(p); err == nil {
			return LoadConfig(p)
		}
	}

	// No config file found — return defaults
	return DefaultConfig(), nil
}
