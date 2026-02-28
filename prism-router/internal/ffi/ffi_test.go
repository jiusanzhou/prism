//go:build !ffi

package ffi

import (
	"strings"
	"testing"
)

// ──────────────────────────────────────────────
// StaticRouter tests
// ──────────────────────────────────────────────

func TestStaticRouterRoute(t *testing.T) {
	router := NewStaticRouter("gpt-4o", "openai", "gpt-4o@openai", "openai-acct", 3, 5)

	req := &RouteRequest{
		Messages: []Message{
			{Role: "user", Content: "hello"},
		},
		Strategy: "balanced",
	}

	result, err := router.Route(req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if result.Model != "gpt-4o" {
		t.Errorf("expected model gpt-4o, got %s", result.Model)
	}
	if result.Provider != "openai" {
		t.Errorf("expected provider openai, got %s", result.Provider)
	}
	if result.Variant != "gpt-4o@openai" {
		t.Errorf("expected variant gpt-4o@openai, got %s", result.Variant)
	}
	if result.Account != "openai-acct" {
		t.Errorf("expected account openai-acct, got %s", result.Account)
	}
	if result.Reason != "static" {
		t.Errorf("expected reason static, got %s", result.Reason)
	}
	if result.Score != 1.0 {
		t.Errorf("expected score 1.0, got %f", result.Score)
	}
	if result.DecisionTimeUs != 0 {
		t.Errorf("expected decision_time_us 0, got %d", result.DecisionTimeUs)
	}
	if result.FallbackChain != nil {
		t.Errorf("expected nil fallback chain, got %v", result.FallbackChain)
	}
}

func TestStaticRouterDifferentParams(t *testing.T) {
	router := NewStaticRouter("claude-3-opus", "anthropic", "claude-3@anthropic", "anthropic-acct", 10, 20)

	result, err := router.Route(&RouteRequest{
		Messages: []Message{{Role: "user", Content: "test"}},
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if result.Model != "claude-3-opus" {
		t.Errorf("expected model claude-3-opus, got %s", result.Model)
	}
	if result.Provider != "anthropic" {
		t.Errorf("expected provider anthropic, got %s", result.Provider)
	}
	if result.Variant != "claude-3@anthropic" {
		t.Errorf("expected variant claude-3@anthropic, got %s", result.Variant)
	}
	if result.Account != "anthropic-acct" {
		t.Errorf("expected account anthropic-acct, got %s", result.Account)
	}
}

func TestStaticRouterIgnoresStrategy(t *testing.T) {
	router := NewStaticRouter("gpt-4o", "openai", "gpt-4o@openai", "openai-acct", 1, 1)

	strategies := []string{"balanced", "cost_first", "quality_first", "slo_first", "pinned:gpt-4o"}
	for _, s := range strategies {
		t.Run(s, func(t *testing.T) {
			result, err := router.Route(&RouteRequest{
				Messages: []Message{{Role: "user", Content: "test"}},
				Strategy: s,
			})
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if result.Model != "gpt-4o" {
				t.Errorf("expected model gpt-4o regardless of strategy %s, got %s", s, result.Model)
			}
		})
	}
}

func TestStaticRouterIgnoresPoolIDs(t *testing.T) {
	router := NewStaticRouter("gpt-4o", "openai", "gpt-4o@openai", "openai-acct", 1, 1)

	result, err := router.Route(&RouteRequest{
		Messages: []Message{{Role: "user", Content: "test"}},
		PoolIDs:  []string{"premium", "enterprise"},
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Model != "gpt-4o" {
		t.Errorf("expected model gpt-4o, got %s", result.Model)
	}
}

func TestStaticRouterIgnoresAffinityAndTenant(t *testing.T) {
	router := NewStaticRouter("gpt-4o", "openai", "gpt-4o@openai", "openai-acct", 1, 1)

	result, err := router.Route(&RouteRequest{
		Messages:    []Message{{Role: "user", Content: "test"}},
		AffinityKey: "session-123",
		TenantID:    "tenant-abc",
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Model != "gpt-4o" {
		t.Errorf("expected model gpt-4o, got %s", result.Model)
	}
}

func TestStaticRouterEmptyMessages(t *testing.T) {
	router := NewStaticRouter("gpt-4o", "openai", "gpt-4o@openai", "openai-acct", 1, 1)

	result, err := router.Route(&RouteRequest{
		Messages: []Message{},
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Model != "gpt-4o" {
		t.Errorf("expected model gpt-4o, got %s", result.Model)
	}
}

// ──────────────────────────────────────────────
// Health
// ──────────────────────────────────────────────

func TestStaticRouterHealth(t *testing.T) {
	router := NewStaticRouter("gpt-4o", "openai", "gpt-4o@openai", "openai-acct", 3, 5)

	health := router.Health()

	if health.Status != "healthy" {
		t.Errorf("expected status healthy, got %s", health.Status)
	}
	if health.RegisteredModels != 3 {
		t.Errorf("expected 3 registered models, got %d", health.RegisteredModels)
	}
	if health.ActiveVariants != 5 {
		t.Errorf("expected 5 active variants, got %d", health.ActiveVariants)
	}
}

func TestStaticRouterHealthZeroCounts(t *testing.T) {
	router := NewStaticRouter("gpt-4o", "openai", "gpt-4o@openai", "openai-acct", 0, 0)

	health := router.Health()

	if health.Status != "healthy" {
		t.Errorf("expected status healthy, got %s", health.Status)
	}
	if health.RegisteredModels != 0 {
		t.Errorf("expected 0 registered models, got %d", health.RegisteredModels)
	}
	if health.ActiveVariants != 0 {
		t.Errorf("expected 0 active variants, got %d", health.ActiveVariants)
	}
}

// ──────────────────────────────────────────────
// Version
// ──────────────────────────────────────────────

func TestStaticRouterVersion(t *testing.T) {
	router := NewStaticRouter("gpt-4o", "openai", "gpt-4o@openai", "openai-acct", 1, 1)

	version := router.Version()
	if version != "static-0.1.0" {
		t.Errorf("expected version static-0.1.0, got %s", version)
	}
}

// ──────────────────────────────────────────────
// UpdateStats
// ──────────────────────────────────────────────

func TestStaticRouterUpdateStats(t *testing.T) {
	router := NewStaticRouter("gpt-4o", "openai", "gpt-4o@openai", "openai-acct", 1, 1)

	// UpdateStats is a no-op for static router, should not error
	err := router.UpdateStats(`{"variant_id":"gpt-4o@openai","status_code":200,"latency_ms":100}`)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestStaticRouterUpdateStatsEmptyJSON(t *testing.T) {
	router := NewStaticRouter("gpt-4o", "openai", "gpt-4o@openai", "openai-acct", 1, 1)

	err := router.UpdateStats("{}")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

// ──────────────────────────────────────────────
// NewFFIRouter unavailable
// ──────────────────────────────────────────────

func TestNewFFIRouterUnavailable(t *testing.T) {
	router, err := NewFFIRouter(`{"providers":[],"models":[]}`)

	if router != nil {
		t.Error("expected nil router when FFI is not available")
	}
	if err == nil {
		t.Fatal("expected error when FFI is not available")
	}
	if !strings.Contains(err.Error(), "FFI router not available") {
		t.Errorf("expected error to mention FFI not available, got: %s", err.Error())
	}
}

// ──────────────────────────────────────────────
// NewRouter wrapper
// ──────────────────────────────────────────────

func TestNewRouterWrapper(t *testing.T) {
	impl := &StaticImpl{
		modelID:    "test-model",
		providerID: "test-provider",
		variantID:  "test-variant",
		accountID:  "test-account",
		models:     1,
		variants:   1,
	}

	router := NewRouter(impl)

	// Verify all methods delegate properly
	result, err := router.Route(&RouteRequest{
		Messages: []Message{{Role: "user", Content: "test"}},
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.Model != "test-model" {
		t.Errorf("expected model test-model, got %s", result.Model)
	}

	health := router.Health()
	if health.Status != "healthy" {
		t.Errorf("expected healthy status, got %s", health.Status)
	}

	version := router.Version()
	if version != "static-0.1.0" {
		t.Errorf("expected version static-0.1.0, got %s", version)
	}

	err = router.UpdateStats(`{}`)
	if err != nil {
		t.Fatalf("unexpected error from UpdateStats: %v", err)
	}
}

// ──────────────────────────────────────────────
// Concurrent access
// ──────────────────────────────────────────────

func TestStaticRouterConcurrent(t *testing.T) {
	router := NewStaticRouter("gpt-4o", "openai", "gpt-4o@openai", "openai-acct", 3, 5)

	done := make(chan struct{})
	const goroutines = 20

	for i := 0; i < goroutines; i++ {
		go func() {
			defer func() { done <- struct{}{} }()
			for j := 0; j < 100; j++ {
				result, err := router.Route(&RouteRequest{
					Messages: []Message{{Role: "user", Content: "concurrent"}},
				})
				if err != nil {
					t.Errorf("unexpected error: %v", err)
					return
				}
				if result.Model != "gpt-4o" {
					t.Errorf("unexpected model: %s", result.Model)
					return
				}

				_ = router.Health()
				_ = router.Version()
				_ = router.UpdateStats(`{}`)
			}
		}()
	}

	for i := 0; i < goroutines; i++ {
		<-done
	}
}
