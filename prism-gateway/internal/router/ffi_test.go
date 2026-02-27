//go:build ffi

package router

import (
	"sync"
	"testing"

	"github.com/aspect-build/prism-gateway/pkg/api"
)

const testConfigJSON = `{
  "providers": [
    {"id": "openai", "name": "OpenAI", "base_url": "https://api.openai.com/v1", "auth": {"ApiKey": {"key": "sk-test", "header": "Authorization"}}}
  ],
  "endpoints": [
    {"id": "openai-ep", "provider_id": "openai"}
  ],
  "accounts": [
    {"id": "openai-acct", "endpoint_id": "openai-ep", "auth": {"ApiKey": {"key": "sk-test", "header": "Authorization"}}}
  ],
  "models": [
    {"id": "gpt-4o", "display_name": "GPT-4o", "quality_tier": "S", "capabilities": {"max_context_window": 128000, "max_output_tokens": 16384, "supports_streaming": true, "supports_function_calling": true, "supports_vision": true, "supports_json_mode": true, "supports_reasoning": false}, "pricing": {"input_per_1m_tokens": 2.5, "output_per_1m_tokens": 10.0}, "specialties": ["Reasoning", "CodeGeneration"]},
    {"id": "gpt-4o-mini", "display_name": "GPT-4o Mini", "quality_tier": "A", "capabilities": {"max_context_window": 128000, "max_output_tokens": 16384, "supports_streaming": true, "supports_function_calling": true, "supports_vision": true, "supports_json_mode": true, "supports_reasoning": false}, "pricing": {"input_per_1m_tokens": 0.15, "output_per_1m_tokens": 0.6}, "specialties": ["SimpleQA"]}
  ],
  "variants": [
    {"id": "gpt-4o@openai", "model_id": "gpt-4o", "endpoint_id": "openai-ep", "account_id": "openai-acct", "performance": {"p50_latency_ms": 300, "p95_latency_ms": 500, "p99_latency_ms": 800, "ttft_p50_ms": 100, "throughput_tps": 50.0, "error_rate": 0.01, "current_qps": 10.0, "queue_depth": 0}},
    {"id": "gpt-4o-mini@openai", "model_id": "gpt-4o-mini", "endpoint_id": "openai-ep", "account_id": "openai-acct", "performance": {"p50_latency_ms": 100, "p95_latency_ms": 200, "p99_latency_ms": 300, "ttft_p50_ms": 50, "throughput_tps": 80.0, "error_rate": 0.005, "current_qps": 30.0, "queue_depth": 0}}
  ],
  "pools": [
    {"id": "default", "name": "Default", "variants": ["gpt-4o@openai", "gpt-4o-mini@openai"]}
  ]
}`

var (
	sharedRouter *FFIRouter
	initOnce     sync.Once
	initErr      error
)

// getTestRouter returns a shared FFIRouter instance (Rust OnceLock only allows one init).
func getTestRouter(t *testing.T) *FFIRouter {
	t.Helper()
	initOnce.Do(func() {
		sharedRouter, initErr = NewFFIRouter(testConfigJSON)
	})
	if initErr != nil {
		t.Fatalf("NewFFIRouter failed: %v", initErr)
	}
	return sharedRouter
}

func TestFFIInit(t *testing.T) {
	r := getTestRouter(t)
	if r == nil {
		t.Fatal("expected non-nil router")
	}
}

func TestFFIRoute(t *testing.T) {
	router := getTestRouter(t)

	req := &RouteRequest{
		Messages: []api.Message{
			{Role: "user", Content: "Hello, how are you?"},
		},
	}

	result, err := router.Route(req)
	if err != nil {
		t.Fatalf("Route failed: %v", err)
	}

	if result.Model == "" {
		t.Error("expected non-empty model")
	}
	if result.Provider == "" {
		t.Error("expected non-empty provider")
	}
	if result.Variant == "" {
		t.Error("expected non-empty variant")
	}
	if result.Account == "" {
		t.Error("expected non-empty account")
	}

	t.Logf("Routed to model=%s provider=%s variant=%s account=%s decision=%dus",
		result.Model, result.Provider, result.Variant, result.Account, result.DecisionTimeUs)
}

func TestFFIRouteWithStrategy(t *testing.T) {
	router := getTestRouter(t)

	strategies := []string{"cost_first", "quality_first", "slo_first", "balanced"}
	for _, strategy := range strategies {
		t.Run(strategy, func(t *testing.T) {
			req := &RouteRequest{
				Messages: []api.Message{
					{Role: "user", Content: "Tell me a joke"},
				},
				Strategy: strategy,
			}

			result, err := router.Route(req)
			if err != nil {
				t.Fatalf("Route with strategy %q failed: %v", strategy, err)
			}
			if result.Model == "" {
				t.Errorf("expected non-empty model for strategy %q", strategy)
			}

			t.Logf("strategy=%s → model=%s variant=%s",
				strategy, result.Model, result.Variant)
		})
	}
}

func TestFFIRouteCodeQuestion(t *testing.T) {
	router := getTestRouter(t)

	req := &RouteRequest{
		Messages: []api.Message{
			{Role: "user", Content: "Write a function to sort an array in Rust"},
		},
	}

	result, err := router.Route(req)
	if err != nil {
		t.Fatalf("Route failed: %v", err)
	}

	t.Logf("Code question → model=%s variant=%s decision=%dus",
		result.Model, result.Variant, result.DecisionTimeUs)
}

func TestFFIRouteWithAffinityKey(t *testing.T) {
	router := getTestRouter(t)

	req := &RouteRequest{
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
		AffinityKey: "session-12345",
	}

	result, err := router.Route(req)
	if err != nil {
		t.Fatalf("Route failed: %v", err)
	}
	if result.Model == "" {
		t.Error("expected non-empty model")
	}
}

func TestFFIRouteConcurrent(t *testing.T) {
	router := getTestRouter(t)

	const concurrency = 50
	var wg sync.WaitGroup
	errCh := make(chan error, concurrency)

	for i := 0; i < concurrency; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			req := &RouteRequest{
				Messages: []api.Message{
					{Role: "user", Content: "Hello concurrent"},
				},
			}
			result, err := router.Route(req)
			if err != nil {
				errCh <- err
				return
			}
			if result.Model == "" {
				errCh <- err
			}
		}()
	}

	wg.Wait()
	close(errCh)

	for err := range errCh {
		t.Errorf("concurrent route error: %v", err)
	}
}

func TestFFIVersion(t *testing.T) {
	router := getTestRouter(t)

	version := router.Version()
	if version == "" {
		t.Error("expected non-empty version string")
	}
	t.Logf("prism_core version: %s", version)
}

func TestFFIHealth(t *testing.T) {
	router := getTestRouter(t)

	health := router.Health()
	if health == "" {
		t.Error("expected non-empty health string")
	}
	t.Logf("prism_core health: %s", health)
}

func TestFFIRouteDecisionTime(t *testing.T) {
	router := getTestRouter(t)

	req := &RouteRequest{
		Messages: []api.Message{
			{Role: "user", Content: "What is 2+2?"},
		},
	}

	result, err := router.Route(req)
	if err != nil {
		t.Fatalf("Route failed: %v", err)
	}

	if result.DecisionTimeUs <= 0 {
		t.Error("expected positive decision time")
	}
	// Decision should be sub-millisecond with in-memory registry
	if result.DecisionTimeUs > 1000 {
		t.Errorf("decision took %d us, expected < 1000us (1ms)", result.DecisionTimeUs)
	}
	t.Logf("decision time: %d us", result.DecisionTimeUs)
}

func TestFFIRouteFallbackChain(t *testing.T) {
	router := getTestRouter(t)

	req := &RouteRequest{
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
	}

	result, err := router.Route(req)
	if err != nil {
		t.Fatalf("Route failed: %v", err)
	}

	// With 2 variants, there should be at least 1 fallback
	if len(result.FallbackChain) == 0 {
		t.Log("No fallback chain (may be expected if only one variant matches)")
	} else {
		t.Logf("Fallback chain: %v", result.FallbackChain)
	}
}
