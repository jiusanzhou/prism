package catalog

import (
	"testing"
)

func TestCatalogLookupOpenAI(t *testing.T) {
	cat := New()
	entry := cat.Lookup("gpt-4o")
	if entry == nil {
		t.Fatal("gpt-4o should exist")
	}
	if entry.Provider != "openai" {
		t.Errorf("provider = %q, want openai", entry.Provider)
	}
	if entry.QualityTier != "S" {
		t.Errorf("tier = %q, want S", entry.QualityTier)
	}
	if entry.Capabilities.MaxContextWindow != 128_000 {
		t.Errorf("context window = %d, want 128000", entry.Capabilities.MaxContextWindow)
	}
	if !entry.Capabilities.SupportsVision {
		t.Error("expected supports_vision = true")
	}
	if !entry.Capabilities.SupportsFunctionCalling {
		t.Error("expected supports_function_calling = true")
	}
	if entry.Pricing.InputPer1MTokens != 2.50 {
		t.Errorf("input price = %f, want 2.50", entry.Pricing.InputPer1MTokens)
	}
	if entry.Pricing.OutputPer1MTokens != 10.00 {
		t.Errorf("output price = %f, want 10.00", entry.Pricing.OutputPer1MTokens)
	}
}

func TestCatalogLookupAnthropic(t *testing.T) {
	cat := New()
	entry := cat.Lookup("claude-3-5-sonnet-20241022")
	if entry == nil {
		t.Fatal("claude-3-5-sonnet-20241022 should exist")
	}
	if entry.Provider != "anthropic" {
		t.Errorf("provider = %q, want anthropic", entry.Provider)
	}
	if entry.QualityTier != "S" {
		t.Errorf("tier = %q, want S", entry.QualityTier)
	}
	if entry.Capabilities.MaxContextWindow != 200_000 {
		t.Errorf("context window = %d, want 200000", entry.Capabilities.MaxContextWindow)
	}
	if !entry.Capabilities.SupportsVision {
		t.Error("expected supports_vision = true")
	}
	if entry.Pricing.InputPer1MTokens != 3.00 {
		t.Errorf("input price = %f, want 3.00", entry.Pricing.InputPer1MTokens)
	}
}

func TestCatalogLookupDeepSeek(t *testing.T) {
	cat := New()
	entry := cat.Lookup("deepseek-chat")
	if entry == nil {
		t.Fatal("deepseek-chat should exist")
	}
	if entry.Provider != "deepseek" {
		t.Errorf("provider = %q, want deepseek", entry.Provider)
	}
	if entry.QualityTier != "A" {
		t.Errorf("tier = %q, want A", entry.QualityTier)
	}
	if !entry.Capabilities.SupportsFunctionCalling {
		t.Error("expected supports_function_calling = true")
	}
	if !entry.Capabilities.SupportsJSONMode {
		t.Error("expected supports_json_mode = true")
	}
	found := map[string]bool{}
	for _, s := range entry.Specialties {
		found[s] = true
	}
	if !found["Reasoning"] {
		t.Error("expected Reasoning specialty")
	}
	if !found["CodeGeneration"] {
		t.Error("expected CodeGeneration specialty")
	}
}

func TestCatalogLookupAlias(t *testing.T) {
	cat := New()

	tests := []struct {
		alias       string
		displayName string
	}{
		{"claude-3.5-sonnet", "Claude 3.5 Sonnet"},
		{"claude-sonnet-4", "Claude Sonnet 4"},
		{"gemini-2.5-pro", "Gemini 2.5 Pro"},
		{"gemini-2.5-flash", "Gemini 2.5 Flash"},
		{"claude-3.5-haiku", "Claude 3.5 Haiku"},
		{"claude-3-opus", "Claude 3 Opus"},
		{"gpt-4o-2024-11-20", "GPT-4o"},
		{"deepseek-r1", "DeepSeek Reasoner (R1)"},
	}

	for _, tt := range tests {
		t.Run(tt.alias, func(t *testing.T) {
			entry := cat.Lookup(tt.alias)
			if entry == nil {
				t.Fatalf("alias %q should resolve", tt.alias)
			}
			if entry.DisplayName != tt.displayName {
				t.Errorf("display_name = %q, want %q", entry.DisplayName, tt.displayName)
			}
		})
	}
}

func TestCatalogLookupUnknown(t *testing.T) {
	cat := New()
	if cat.Lookup("nonexistent-model-xyz") != nil {
		t.Error("expected nil for unknown model")
	}
}

func TestCatalogModelsForProvider(t *testing.T) {
	cat := New()

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

func TestCatalogAllModelsNotEmpty(t *testing.T) {
	cat := New()
	all := cat.AllModels()
	if len(all) < 16 {
		t.Errorf("expected at least 16 models, got %d", len(all))
	}
}

func TestCatalogGoogleContextWindow(t *testing.T) {
	cat := New()
	entry := cat.Lookup("gemini-2.0-flash")
	if entry == nil {
		t.Fatal("gemini-2.0-flash should exist")
	}
	if entry.Capabilities.MaxContextWindow != 1_048_576 {
		t.Errorf("context window = %d, want 1048576 (1M)", entry.Capabilities.MaxContextWindow)
	}
}

func TestCatalogPricingAccuracy(t *testing.T) {
	cat := New()

	// DeepSeek Reasoner
	entry := cat.Lookup("deepseek-reasoner")
	if entry == nil {
		t.Fatal("deepseek-reasoner should exist")
	}
	if entry.Pricing.InputPer1MTokens != 0.55 {
		t.Errorf("deepseek-reasoner input = %f, want 0.55", entry.Pricing.InputPer1MTokens)
	}
	if entry.Pricing.OutputPer1MTokens != 2.19 {
		t.Errorf("deepseek-reasoner output = %f, want 2.19", entry.Pricing.OutputPer1MTokens)
	}

	// o3-mini
	entry = cat.Lookup("o3-mini")
	if entry == nil {
		t.Fatal("o3-mini should exist")
	}
	if entry.Pricing.InputPer1MTokens != 1.10 {
		t.Errorf("o3-mini input = %f, want 1.10", entry.Pricing.InputPer1MTokens)
	}
	if entry.Pricing.OutputPer1MTokens != 4.40 {
		t.Errorf("o3-mini output = %f, want 4.40", entry.Pricing.OutputPer1MTokens)
	}
}
