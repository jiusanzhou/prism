package discovery

import (
	"testing"

	"go.zoe.im/prism-gateway/internal/catalog"
	"go.zoe.im/prism-gateway/internal/config"
)

func TestCatalogLookup(t *testing.T) {
	cat := catalog.New()

	tests := []struct {
		name     string
		modelID  string
		wantNil  bool
		provider string
		tier     string
	}{
		{"openai model", "gpt-4o", false, "openai", "S"},
		{"anthropic model", "claude-3-5-sonnet-20241022", false, "anthropic", "S"},
		{"deepseek model", "deepseek-chat", false, "deepseek", "A"},
		{"google model", "gemini-2.0-flash", false, "google", "A"},
		{"alias claude-3.5-sonnet", "claude-3.5-sonnet", false, "anthropic", "S"},
		{"alias gemini-2.5-pro", "gemini-2.5-pro", false, "google", "S"},
		{"alias claude-sonnet-4", "claude-sonnet-4", false, "anthropic", "S"},
		{"unknown model", "nonexistent-model", true, "", ""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			entry := cat.Lookup(tt.modelID)
			if tt.wantNil {
				if entry != nil {
					t.Errorf("expected nil for %q, got %v", tt.modelID, entry)
				}
				return
			}
			if entry == nil {
				t.Fatalf("expected non-nil for %q", tt.modelID)
			}
			if entry.Provider != tt.provider {
				t.Errorf("provider = %q, want %q", entry.Provider, tt.provider)
			}
			if entry.QualityTier != tt.tier {
				t.Errorf("tier = %q, want %q", entry.QualityTier, tt.tier)
			}
		})
	}
}

func TestCatalogModelsForProvider(t *testing.T) {
	cat := catalog.New()

	tests := []struct {
		provider string
		minCount int
	}{
		{"openai", 7},
		{"anthropic", 4},
		{"deepseek", 2},
		{"google", 3},
	}

	for _, tt := range tests {
		t.Run(tt.provider, func(t *testing.T) {
			models := cat.ModelsForProvider(tt.provider)
			if len(models) < tt.minCount {
				t.Errorf("got %d models for %q, want at least %d", len(models), tt.provider, tt.minCount)
			}
		})
	}
}

func TestAutoConfigGeneratesModels(t *testing.T) {
	providers := map[string]config.ProviderConfig{
		"openai": {
			Type:   "openai",
			APIKey: "test-key",
		},
		"anthropic": {
			Type:   "anthropic",
			APIKey: "test-key",
		},
	}

	cfg, err := AutoConfigFromProviderConfigs(providers)
	if err != nil {
		t.Fatalf("AutoConfig failed: %v", err)
	}

	if len(cfg.Models) == 0 {
		t.Fatal("expected models to be generated, got 0")
	}

	// Should have models from both providers
	foundOpenAI := false
	foundAnthropic := false
	for _, m := range cfg.Models {
		if m.ID == "gpt-4o" {
			foundOpenAI = true
			if m.QualityTier != "S" {
				t.Errorf("gpt-4o quality tier = %q, want S", m.QualityTier)
			}
			if m.Pricing.InputPer1MTokens != 2.50 {
				t.Errorf("gpt-4o input price = %f, want 2.50", m.Pricing.InputPer1MTokens)
			}
		}
		if m.ID == "claude-3-5-sonnet-20241022" {
			foundAnthropic = true
		}
	}

	if !foundOpenAI {
		t.Error("expected gpt-4o in generated models")
	}
	if !foundAnthropic {
		t.Error("expected claude-3-5-sonnet-20241022 in generated models")
	}
}

func TestAutoConfigGeneratesVariants(t *testing.T) {
	providers := map[string]config.ProviderConfig{
		"openai": {
			Type:   "openai",
			APIKey: "test-key",
		},
	}

	cfg, err := AutoConfigFromProviderConfigs(providers)
	if err != nil {
		t.Fatalf("AutoConfig failed: %v", err)
	}

	if len(cfg.Variants) == 0 {
		t.Fatal("expected variants to be generated, got 0")
	}

	// Each variant should reference a valid model and provider
	for _, v := range cfg.Variants {
		if v.ProviderID != "openai" {
			t.Errorf("variant %q has provider %q, want openai", v.ID, v.ProviderID)
		}

		// Find model
		found := false
		for _, m := range cfg.Models {
			if m.ID == v.ModelID {
				found = true
				break
			}
		}
		if !found {
			t.Errorf("variant %q references model %q which doesn't exist", v.ID, v.ModelID)
		}
	}

	// Variant IDs should be model@provider format
	for _, v := range cfg.Variants {
		expected := v.ModelID + "@" + v.ProviderID
		if v.ID != expected {
			t.Errorf("variant ID = %q, want %q", v.ID, expected)
		}
	}
}

func TestAutoConfigGeneratesPool(t *testing.T) {
	providers := map[string]config.ProviderConfig{
		"openai": {
			Type:   "openai",
			APIKey: "test-key",
		},
		"deepseek": {
			Type:   "deepseek",
			APIKey: "test-key",
		},
	}

	cfg, err := AutoConfigFromProviderConfigs(providers)
	if err != nil {
		t.Fatalf("AutoConfig failed: %v", err)
	}

	if len(cfg.Pools) == 0 {
		t.Fatal("expected at least one pool")
	}

	defaultPool := cfg.Pools[0]
	if defaultPool.ID != "default" {
		t.Errorf("pool ID = %q, want default", defaultPool.ID)
	}

	if len(defaultPool.Variants) != len(cfg.Variants) {
		t.Errorf("pool has %d variants, want %d (all variants)", len(defaultPool.Variants), len(cfg.Variants))
	}

	// Default pools should be set
	if len(cfg.Routing.DefaultPools) == 0 || cfg.Routing.DefaultPools[0] != "default" {
		t.Errorf("default pools = %v, want [default]", cfg.Routing.DefaultPools)
	}
}

func TestAutoConfigWithDiscoveredModels(t *testing.T) {
	providers := map[string]config.ProviderConfig{
		"openai": {
			Type:   "openai",
			APIKey: "test-key",
		},
	}

	discovered := map[string]*ProviderInfo{
		"openai": {
			ID:   "openai",
			Type: "openai",
			AvailableModels: []DiscoveredModel{
				{ID: "gpt-4o", Available: true},
				{ID: "gpt-4o-mini", Available: true},
				// Unknown model — should still be added with warning
				{ID: "gpt-5-ultra", Available: true},
			},
		},
	}

	cfg, err := AutoConfig(providers, discovered)
	if err != nil {
		t.Fatalf("AutoConfig failed: %v", err)
	}

	// Should only have 3 models (the discovered ones)
	if len(cfg.Models) != 3 {
		t.Errorf("got %d models, want 3", len(cfg.Models))
	}

	// gpt-5-ultra should be added with default tier B
	for _, m := range cfg.Models {
		if m.ID == "gpt-5-ultra" {
			if m.QualityTier != "B" {
				t.Errorf("unknown model tier = %q, want B", m.QualityTier)
			}
			return
		}
	}
	t.Error("expected gpt-5-ultra in models")
}

func TestShouldAutoConfig(t *testing.T) {
	tests := []struct {
		name string
		cfg  *config.Config
		want bool
	}{
		{
			"empty config",
			&config.Config{},
			false, // no providers
		},
		{
			"providers only",
			&config.Config{
				Providers: map[string]config.ProviderConfig{
					"openai": {Type: "openai", APIKey: "test"},
				},
			},
			true,
		},
		{
			"has models",
			&config.Config{
				Providers: map[string]config.ProviderConfig{
					"openai": {Type: "openai", APIKey: "test"},
				},
				Models: []config.ModelConfig{
					{ID: "gpt-4o"},
				},
			},
			false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := ShouldAutoConfig(tt.cfg)
			if got != tt.want {
				t.Errorf("ShouldAutoConfig = %v, want %v", got, tt.want)
			}
		})
	}
}

func TestIsChatModel(t *testing.T) {
	tests := []struct {
		model string
		want  bool
	}{
		{"gpt-4o", true},
		{"gpt-4o-mini", true},
		{"claude-3-5-sonnet-20241022", true},
		{"deepseek-chat", true},
		{"gemini-2.0-flash", true},
		{"text-embedding-3-small", false},
		{"text-embedding-ada-002", false},
		{"whisper-1", false},
		{"tts-1", false},
		{"dall-e-3", false},
	}

	for _, tt := range tests {
		t.Run(tt.model, func(t *testing.T) {
			got := IsChatModel(tt.model)
			if got != tt.want {
				t.Errorf("IsChatModel(%q) = %v, want %v", tt.model, got, tt.want)
			}
		})
	}
}
