package api_test

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"go.zoe.im/prism-admin/internal/api"
	"go.zoe.im/prism-admin/internal/config"
)

// testConfig returns a sample prism config for testing.
func testConfig() *config.PrismConfig {
	return &config.PrismConfig{
		Server: config.ServerConfig{HTTPPort: 8080, AdminPort: 8081},
		Routing: config.RoutingConfig{
			DefaultStrategy: "balanced",
			DefaultPools:    []string{"default"},
			Balanced: &config.BalancedConfig{
				CostWeight:    0.3,
				QualityWeight: 0.5,
				LatencyWeight: 0.2,
			},
		},
		Providers: map[string]config.ProviderConfig{
			"openai": {
				Type:      "openai",
				APIKey:    "sk-test-key-1234567890abcdef",
				BaseURL:   "https://api.openai.com/v1",
				TimeoutMs: 30000,
			},
			"deepseek": {
				Type:      "deepseek",
				APIKey:    "ds-test-key-abcdef",
				BaseURL:   "https://api.deepseek.com",
				TimeoutMs: 30000,
			},
		},
		Models: []config.ModelConfig{
			{
				ID:          "gpt-4o",
				DisplayName: "GPT-4o",
				QualityTier: "S",
				Capabilities: config.CapabilitiesConfig{
					MaxContextWindow:        128000,
					MaxOutputTokens:         16384,
					SupportsStreaming:        true,
					SupportsFunctionCalling: true,
					SupportsVision:          true,
					SupportsJSONMode:        true,
				},
				Pricing: config.PricingConfig{
					InputPer1MTokens:  2.5,
					OutputPer1MTokens: 10.0,
				},
				Specialties: []string{"Reasoning", "CodeGeneration"},
			},
			{
				ID:          "deepseek-chat",
				DisplayName: "DeepSeek V3",
				QualityTier: "A",
				Capabilities: config.CapabilitiesConfig{
					MaxContextWindow:        128000,
					MaxOutputTokens:         8192,
					SupportsStreaming:        true,
					SupportsFunctionCalling: true,
					SupportsJSONMode:        true,
				},
				Pricing: config.PricingConfig{
					InputPer1MTokens:  0.27,
					OutputPer1MTokens: 1.10,
				},
				Specialties: []string{"Reasoning"},
			},
		},
		Variants: []config.VariantConfig{
			{ID: "gpt-4o@openai", ModelID: "gpt-4o", ProviderID: "openai"},
			{ID: "deepseek-chat@deepseek", ModelID: "deepseek-chat", ProviderID: "deepseek"},
		},
		Pools: []config.PoolConfig{
			{
				ID:       "default",
				Name:     "Default Pool",
				Variants: []string{"gpt-4o@openai", "deepseek-chat@deepseek"},
			},
		},
		Storage: config.StorageConfig{
			Postgres: config.PostgresConfig{DSN: "postgres://user:pass@localhost/prism"},
		},
	}
}

func testServer() *api.Server {
	adminCfg := &config.AdminConfig{
		ListenAddr: ":0",
		GatewayURL: "http://localhost:8080",
		ConfigPath: "test.yaml",
	}
	return api.NewServer(adminCfg, testConfig())
}

func doGet(t *testing.T, handler http.Handler, path string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(http.MethodGet, path, nil)
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)
	return w
}

func decodeResponse(t *testing.T, w *httptest.ResponseRecorder) map[string]interface{} {
	t.Helper()
	var result map[string]interface{}
	if err := json.Unmarshal(w.Body.Bytes(), &result); err != nil {
		t.Fatalf("failed to decode response: %v\nbody: %s", err, w.Body.String())
	}
	return result
}

func TestStatusEndpoint(t *testing.T) {
	srv := testServer()
	w := doGet(t, srv.Handler(), "/api/v1/status")

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	result := decodeResponse(t, w)
	data, ok := result["data"].(map[string]interface{})
	if !ok {
		t.Fatal("expected data field in response")
	}

	if status := data["status"]; status != "ok" {
		t.Errorf("expected status=ok, got %v", status)
	}

	if av := data["active_variants"]; av != float64(2) {
		t.Errorf("expected active_variants=2, got %v", av)
	}

	if tp := data["total_providers"]; tp != float64(2) {
		t.Errorf("expected total_providers=2, got %v", tp)
	}

	if tm := data["total_models"]; tm != float64(2) {
		t.Errorf("expected total_models=2, got %v", tm)
	}

	if strategy := data["routing_strategy"]; strategy != "balanced" {
		t.Errorf("expected routing_strategy=balanced, got %v", strategy)
	}
}

func TestProvidersEndpoint(t *testing.T) {
	srv := testServer()

	t.Run("list", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/providers")
		if w.Code != http.StatusOK {
			t.Fatalf("expected 200, got %d", w.Code)
		}

		result := decodeResponse(t, w)
		meta, ok := result["meta"].(map[string]interface{})
		if !ok {
			t.Fatal("expected meta field")
		}
		if total := meta["total"]; total != float64(2) {
			t.Errorf("expected total=2, got %v", total)
		}
	})

	t.Run("by_id", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/providers/openai")
		if w.Code != http.StatusOK {
			t.Fatalf("expected 200, got %d", w.Code)
		}

		result := decodeResponse(t, w)
		data, ok := result["data"].(map[string]interface{})
		if !ok {
			t.Fatal("expected data field")
		}
		if data["id"] != "openai" {
			t.Errorf("expected id=openai, got %v", data["id"])
		}
		if data["type"] != "openai" {
			t.Errorf("expected type=openai, got %v", data["type"])
		}
	})

	t.Run("not_found", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/providers/nonexistent")
		if w.Code != http.StatusNotFound {
			t.Fatalf("expected 404, got %d", w.Code)
		}
	})
}

func TestModelsEndpoint(t *testing.T) {
	srv := testServer()

	t.Run("list", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/models")
		if w.Code != http.StatusOK {
			t.Fatalf("expected 200, got %d", w.Code)
		}

		result := decodeResponse(t, w)
		meta := result["meta"].(map[string]interface{})
		if total := meta["total"]; total != float64(2) {
			t.Errorf("expected total=2, got %v", total)
		}
	})

	t.Run("by_id", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/models/gpt-4o")
		if w.Code != http.StatusOK {
			t.Fatalf("expected 200, got %d", w.Code)
		}

		result := decodeResponse(t, w)
		data := result["data"].(map[string]interface{})
		if data["id"] != "gpt-4o" {
			t.Errorf("expected id=gpt-4o, got %v", data["id"])
		}
		if data["quality_tier"] != "S" {
			t.Errorf("expected quality_tier=S, got %v", data["quality_tier"])
		}

		// Should include variants.
		variants, ok := data["variants"].([]interface{})
		if !ok {
			t.Fatal("expected variants field")
		}
		if len(variants) != 1 {
			t.Errorf("expected 1 variant for gpt-4o, got %d", len(variants))
		}
	})

	t.Run("not_found", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/models/nonexistent")
		if w.Code != http.StatusNotFound {
			t.Fatalf("expected 404, got %d", w.Code)
		}
	})
}

func TestVariantsEndpoint(t *testing.T) {
	srv := testServer()

	t.Run("list", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/variants")
		if w.Code != http.StatusOK {
			t.Fatalf("expected 200, got %d", w.Code)
		}

		result := decodeResponse(t, w)
		meta := result["meta"].(map[string]interface{})
		if total := meta["total"]; total != float64(2) {
			t.Errorf("expected total=2, got %v", total)
		}
	})

	t.Run("by_id", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/variants/gpt-4o@openai")
		if w.Code != http.StatusOK {
			t.Fatalf("expected 200, got %d", w.Code)
		}

		result := decodeResponse(t, w)
		data := result["data"].(map[string]interface{})
		if data["id"] != "gpt-4o@openai" {
			t.Errorf("expected id=gpt-4o@openai, got %v", data["id"])
		}
		if data["status"] != "active" {
			t.Errorf("expected status=active, got %v", data["status"])
		}
	})

	t.Run("update_status", func(t *testing.T) {
		body := bytes.NewBufferString(`{"status": "disabled"}`)
		req := httptest.NewRequest(http.MethodPut, "/api/v1/variants/gpt-4o@openai/status", body)
		req.Header.Set("Content-Type", "application/json")
		w := httptest.NewRecorder()
		srv.Handler().ServeHTTP(w, req)

		if w.Code != http.StatusOK {
			t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
		}

		result := decodeResponse(t, w)
		data := result["data"].(map[string]interface{})
		if data["status"] != "disabled" {
			t.Errorf("expected status=disabled, got %v", data["status"])
		}

		// Verify it persists on GET.
		w2 := doGet(t, srv.Handler(), "/api/v1/variants/gpt-4o@openai")
		result2 := decodeResponse(t, w2)
		data2 := result2["data"].(map[string]interface{})
		if data2["status"] != "disabled" {
			t.Errorf("expected status=disabled after update, got %v", data2["status"])
		}
	})

	t.Run("update_status_invalid", func(t *testing.T) {
		body := bytes.NewBufferString(`{"status": "invalid_status"}`)
		req := httptest.NewRequest(http.MethodPut, "/api/v1/variants/gpt-4o@openai/status", body)
		req.Header.Set("Content-Type", "application/json")
		w := httptest.NewRecorder()
		srv.Handler().ServeHTTP(w, req)

		if w.Code != http.StatusBadRequest {
			t.Fatalf("expected 400, got %d", w.Code)
		}
	})

	t.Run("not_found", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/variants/nonexistent")
		if w.Code != http.StatusNotFound {
			t.Fatalf("expected 404, got %d", w.Code)
		}
	})
}

func TestDashboardOverview(t *testing.T) {
	srv := testServer()
	w := doGet(t, srv.Handler(), "/api/v1/dashboard/overview")

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	result := decodeResponse(t, w)
	data, ok := result["data"].(map[string]interface{})
	if !ok {
		t.Fatal("expected data field")
	}

	// Verify expected fields exist.
	requiredFields := []string{
		"total_requests", "avg_latency_ms", "error_rate",
		"total_cost_usd", "active_variants", "active_experiments",
		"uptime_seconds", "requests_per_second",
	}
	for _, field := range requiredFields {
		if _, ok := data[field]; !ok {
			t.Errorf("missing field: %s", field)
		}
	}

	if av := data["active_variants"]; av != float64(2) {
		t.Errorf("expected active_variants=2, got %v", av)
	}
}

func TestConfigSanitization(t *testing.T) {
	srv := testServer()
	w := doGet(t, srv.Handler(), "/api/v1/config")

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	body := w.Body.String()

	// Verify API keys are NOT in the response.
	if bytes.Contains([]byte(body), []byte("sk-test-key-1234567890abcdef")) {
		t.Error("response contains unmasked OpenAI API key")
	}
	if bytes.Contains([]byte(body), []byte("ds-test-key-abcdef")) {
		t.Error("response contains unmasked DeepSeek API key")
	}

	// Verify DSN is masked.
	if bytes.Contains([]byte(body), []byte("postgres://user:pass@localhost/prism")) {
		t.Error("response contains unmasked Postgres DSN")
	}

	// Verify the response is valid JSON with providers present.
	result := decodeResponse(t, w)
	data, ok := result["data"].(map[string]interface{})
	if !ok {
		t.Fatal("expected data field")
	}

	providers, ok := data["providers"].(map[string]interface{})
	if !ok {
		t.Fatal("expected providers in config response")
	}
	if len(providers) != 2 {
		t.Errorf("expected 2 providers, got %d", len(providers))
	}

	// Check that the API key is masked.
	openai, ok := providers["openai"].(map[string]interface{})
	if !ok {
		t.Fatal("expected openai provider")
	}
	apiKey, _ := openai["api_key"].(string)
	if apiKey == "sk-test-key-1234567890abcdef" {
		t.Error("openai api_key should be masked")
	}
	if apiKey == "" {
		t.Error("openai api_key should not be empty (should be masked, not removed)")
	}
}

func TestPoolsEndpoint(t *testing.T) {
	srv := testServer()

	t.Run("list", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/pools")
		if w.Code != http.StatusOK {
			t.Fatalf("expected 200, got %d", w.Code)
		}

		result := decodeResponse(t, w)
		meta := result["meta"].(map[string]interface{})
		if total := meta["total"]; total != float64(1) {
			t.Errorf("expected total=1, got %v", total)
		}
	})

	t.Run("by_id", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/pools/default")
		if w.Code != http.StatusOK {
			t.Fatalf("expected 200, got %d", w.Code)
		}

		result := decodeResponse(t, w)
		data := result["data"].(map[string]interface{})
		if data["id"] != "default" {
			t.Errorf("expected id=default, got %v", data["id"])
		}
		variants := data["variants"].([]interface{})
		if len(variants) != 2 {
			t.Errorf("expected 2 variants in pool, got %d", len(variants))
		}
	})

	t.Run("not_found", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/pools/nonexistent")
		if w.Code != http.StatusNotFound {
			t.Fatalf("expected 404, got %d", w.Code)
		}
	})
}

func TestDashboardRouting(t *testing.T) {
	srv := testServer()
	w := doGet(t, srv.Handler(), "/api/v1/dashboard/routing")

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	result := decodeResponse(t, w)
	data := result["data"].(map[string]interface{})

	if _, ok := data["strategy_distribution"]; !ok {
		t.Error("missing strategy_distribution")
	}
	if _, ok := data["model_selection_freq"]; !ok {
		t.Error("missing model_selection_freq")
	}
	if _, ok := data["pool_usage"]; !ok {
		t.Error("missing pool_usage")
	}
}

func TestDashboardProviders(t *testing.T) {
	srv := testServer()
	w := doGet(t, srv.Handler(), "/api/v1/dashboard/providers")

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	result := decodeResponse(t, w)
	meta := result["meta"].(map[string]interface{})
	if total := meta["total"]; total != float64(2) {
		t.Errorf("expected total=2 providers, got %v", total)
	}
}

func TestExperimentsEndpoint(t *testing.T) {
	srv := testServer()

	t.Run("list", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/experiments")
		if w.Code != http.StatusOK {
			t.Fatalf("expected 200, got %d", w.Code)
		}

		result := decodeResponse(t, w)
		meta := result["meta"].(map[string]interface{})
		if total := meta["total"]; total != float64(0) {
			t.Errorf("expected total=0, got %v", total)
		}
	})

	t.Run("results_not_found", func(t *testing.T) {
		w := doGet(t, srv.Handler(), "/api/v1/experiments/nonexistent/results")
		if w.Code != http.StatusNotFound {
			t.Fatalf("expected 404, got %d", w.Code)
		}
	})
}

func TestCORSHeaders(t *testing.T) {
	srv := testServer()

	// Test preflight.
	req := httptest.NewRequest(http.MethodOptions, "/api/v1/status", nil)
	w := httptest.NewRecorder()
	srv.Handler().ServeHTTP(w, req)

	if w.Code != http.StatusNoContent {
		t.Fatalf("expected 204 for OPTIONS, got %d", w.Code)
	}
	if origin := w.Header().Get("Access-Control-Allow-Origin"); origin != "*" {
		t.Errorf("expected CORS origin=*, got %q", origin)
	}

	// Test regular request has CORS headers.
	w2 := doGet(t, srv.Handler(), "/api/v1/status")
	if origin := w2.Header().Get("Access-Control-Allow-Origin"); origin != "*" {
		t.Errorf("expected CORS origin=* on regular response, got %q", origin)
	}
}
