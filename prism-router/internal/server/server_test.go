package server

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/aspect-build/prism-router/internal/ffi"
)

// helper: create a Server backed by a StaticRouter
func newTestServer() *Server {
	router := ffi.NewStaticRouter("gpt-4o", "openai", "gpt-4o@openai", "openai-acct", 3, 5)
	return New(router)
}

func strPtr(s string) *string { return &s }

func float32Ptr(f float32) *float32 { return &f }

func uint32Ptr(u uint32) *uint32 { return &u }

// ──────────────────────────────────────────────
// POST /v1/route
// ──────────────────────────────────────────────

func TestRouteBasic(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	body := `{"messages":[{"role":"user","content":"hello"}]}`
	req := httptest.NewRequest(http.MethodPost, "/v1/route", strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp RouteResponse
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("failed to decode response: %v", err)
	}

	if resp.Model != "gpt-4o" {
		t.Errorf("expected model gpt-4o, got %s", resp.Model)
	}
	if resp.Provider != "openai" {
		t.Errorf("expected provider openai, got %s", resp.Provider)
	}
	if resp.Variant != "gpt-4o@openai" {
		t.Errorf("expected variant gpt-4o@openai, got %s", resp.Variant)
	}
	if resp.Account != "openai-acct" {
		t.Errorf("expected account openai-acct, got %s", resp.Account)
	}
	if resp.Upstream == nil {
		t.Fatal("expected upstream to be non-nil")
	}
	if resp.Upstream.ModelName != "gpt-4o" {
		t.Errorf("expected upstream model_name gpt-4o, got %s", resp.Upstream.ModelName)
	}
	if resp.Upstream.Path != "/v1/chat/completions" {
		t.Errorf("expected upstream path /v1/chat/completions, got %s", resp.Upstream.Path)
	}
	if resp.Score != 1.0 {
		t.Errorf("expected score 1.0, got %f", resp.Score)
	}
}

func TestRouteStrategies(t *testing.T) {
	strategies := []string{"balanced", "cost_first", "quality_first", "slo_first"}

	for _, strategy := range strategies {
		t.Run(strategy, func(t *testing.T) {
			srv := newTestServer()
			handler := srv.Handler()

			reqBody := RouteRequest{
				Messages: []ChatMessage{{Role: "user", Content: "test"}},
				Strategy: strPtr(strategy),
			}
			bodyBytes, _ := json.Marshal(reqBody)
			req := httptest.NewRequest(http.MethodPost, "/v1/route", bytes.NewReader(bodyBytes))
			req.Header.Set("Content-Type", "application/json")
			w := httptest.NewRecorder()

			handler.ServeHTTP(w, req)

			if w.Code != http.StatusOK {
				t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
			}

			var resp RouteResponse
			if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
				t.Fatalf("failed to decode response: %v", err)
			}

			// Static router always returns the same result regardless of strategy
			if resp.Model != "gpt-4o" {
				t.Errorf("expected model gpt-4o, got %s", resp.Model)
			}
		})
	}
}

func TestRouteModelFieldAutoPinned(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	// When model is set to a concrete model (not "auto"), strategy should be pinned
	reqBody := RouteRequest{
		Messages: []ChatMessage{{Role: "user", Content: "test"}},
		Model:    strPtr("claude-3-opus"),
	}
	bodyBytes, _ := json.Marshal(reqBody)
	req := httptest.NewRequest(http.MethodPost, "/v1/route", bytes.NewReader(bodyBytes))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp RouteResponse
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("failed to decode response: %v", err)
	}
	// Static router always returns gpt-4o regardless of pinned model
	if resp.Model != "gpt-4o" {
		t.Errorf("expected model gpt-4o, got %s", resp.Model)
	}
}

func TestRouteModelAuto(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	reqBody := RouteRequest{
		Messages: []ChatMessage{{Role: "user", Content: "test"}},
		Model:    strPtr("auto"),
	}
	bodyBytes, _ := json.Marshal(reqBody)
	req := httptest.NewRequest(http.MethodPost, "/v1/route", bytes.NewReader(bodyBytes))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}
}

func TestRouteAffinityKeyAndTenantID(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	reqBody := RouteRequest{
		Messages:    []ChatMessage{{Role: "user", Content: "test"}},
		AffinityKey: strPtr("session-123"),
		TenantID:    strPtr("tenant-abc"),
	}
	bodyBytes, _ := json.Marshal(reqBody)
	req := httptest.NewRequest(http.MethodPost, "/v1/route", bytes.NewReader(bodyBytes))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp RouteResponse
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("failed to decode response: %v", err)
	}
	if resp.Model != "gpt-4o" {
		t.Errorf("expected model gpt-4o, got %s", resp.Model)
	}
}

func TestRouteStrategyParamsWeightOverride(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	reqBody := RouteRequest{
		Messages: []ChatMessage{{Role: "user", Content: "test"}},
		Strategy: strPtr("balanced"),
		StrategyParams: &StrategyParams{
			CostWeight:    float32Ptr(0.8),
			QualityWeight: float32Ptr(0.1),
			LatencyWeight: float32Ptr(0.1),
			MaxLatencyMs:  uint32Ptr(500),
			MaxTTFTMs:     uint32Ptr(200),
		},
	}
	bodyBytes, _ := json.Marshal(reqBody)
	req := httptest.NewRequest(http.MethodPost, "/v1/route", bytes.NewReader(bodyBytes))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}
}

func TestRoutePoolIDs(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	reqBody := RouteRequest{
		Messages: []ChatMessage{{Role: "user", Content: "test"}},
		PoolIDs:  []string{"premium", "default"},
	}
	bodyBytes, _ := json.Marshal(reqBody)
	req := httptest.NewRequest(http.MethodPost, "/v1/route", bytes.NewReader(bodyBytes))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}
}

func TestRouteMetadataPassthrough(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	reqBody := RouteRequest{
		Messages: []ChatMessage{{Role: "user", Content: "test"}},
		Metadata: map[string]string{
			"trace_id":   "abc-123",
			"request_id": "req-456",
		},
	}
	bodyBytes, _ := json.Marshal(reqBody)
	req := httptest.NewRequest(http.MethodPost, "/v1/route", bytes.NewReader(bodyBytes))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp RouteResponse
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("failed to decode response: %v", err)
	}

	if resp.ResponseMetadata == nil {
		t.Fatal("expected response_metadata to be non-nil")
	}
	if resp.ResponseMetadata["trace_id"] != "abc-123" {
		t.Errorf("expected trace_id abc-123, got %s", resp.ResponseMetadata["trace_id"])
	}
	if resp.ResponseMetadata["request_id"] != "req-456" {
		t.Errorf("expected request_id req-456, got %s", resp.ResponseMetadata["request_id"])
	}
}

func TestRouteInvalidJSON(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	req := httptest.NewRequest(http.MethodPost, "/v1/route", strings.NewReader("{invalid json"))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d", w.Code)
	}

	var errResp errorResponse
	if err := json.Unmarshal(w.Body.Bytes(), &errResp); err != nil {
		t.Fatalf("failed to decode error response: %v", err)
	}
	if errResp.Error == "" {
		t.Error("expected non-empty error message")
	}
}

func TestRouteEmptyBody(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	req := httptest.NewRequest(http.MethodPost, "/v1/route", strings.NewReader(""))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d", w.Code)
	}
}

func TestRouteContentTypeJSON(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	body := `{"messages":[{"role":"user","content":"hello"}]}`
	req := httptest.NewRequest(http.MethodPost, "/v1/route", strings.NewReader(body))
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	ct := w.Header().Get("Content-Type")
	if ct != "application/json" {
		t.Errorf("expected Content-Type application/json, got %s", ct)
	}
}

// ──────────────────────────────────────────────
// POST /v1/route/batch
// ──────────────────────────────────────────────

func TestRouteBatchBasic(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	reqBody := RouteBatchRequest{
		Requests: []RouteRequest{
			{Messages: []ChatMessage{{Role: "user", Content: "hello"}}},
			{Messages: []ChatMessage{{Role: "user", Content: "world"}}},
			{Messages: []ChatMessage{{Role: "user", Content: "test"}}},
		},
	}
	bodyBytes, _ := json.Marshal(reqBody)
	req := httptest.NewRequest(http.MethodPost, "/v1/route/batch", bytes.NewReader(bodyBytes))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp RouteBatchResponse
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("failed to decode response: %v", err)
	}

	if len(resp.Responses) != 3 {
		t.Fatalf("expected 3 responses, got %d", len(resp.Responses))
	}

	for i, r := range resp.Responses {
		if r.Model != "gpt-4o" {
			t.Errorf("response[%d]: expected model gpt-4o, got %s", i, r.Model)
		}
	}
}

func TestRouteBatchEmpty(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	reqBody := RouteBatchRequest{Requests: []RouteRequest{}}
	bodyBytes, _ := json.Marshal(reqBody)
	req := httptest.NewRequest(http.MethodPost, "/v1/route/batch", bytes.NewReader(bodyBytes))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusBadRequest {
		t.Fatalf("expected 400 for empty batch, got %d", w.Code)
	}
}

func TestRouteBatchInvalidJSON(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	req := httptest.NewRequest(http.MethodPost, "/v1/route/batch", strings.NewReader("not json"))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d", w.Code)
	}
}

// ──────────────────────────────────────────────
// POST /v1/outcome
// ──────────────────────────────────────────────

func TestOutcomeBasic(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	report := OutcomeReport{
		RequestID:    "req-001",
		VariantID:    "gpt-4o@openai",
		StatusCode:   200,
		LatencyMs:    150,
		TTFTMs:       50,
		InputTokens:  100,
		OutputTokens: 200,
	}
	bodyBytes, _ := json.Marshal(report)
	req := httptest.NewRequest(http.MethodPost, "/v1/outcome", bytes.NewReader(bodyBytes))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	var resp ReportResponse
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("failed to decode response: %v", err)
	}
}

func TestOutcomeWithError(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	errMsg := "rate limit exceeded"
	report := OutcomeReport{
		RequestID:         "req-002",
		VariantID:         "gpt-4o@openai",
		StatusCode:        429,
		LatencyMs:         10,
		InputTokens:       50,
		OutputTokens:      0,
		Error:             &errMsg,
		UsedFallback:      true,
		FallbackVariantID: strPtr("claude-3@anthropic"),
	}
	bodyBytes, _ := json.Marshal(report)
	req := httptest.NewRequest(http.MethodPost, "/v1/outcome", bytes.NewReader(bodyBytes))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}
}

func TestOutcomeInvalidJSON(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	req := httptest.NewRequest(http.MethodPost, "/v1/outcome", strings.NewReader("{bad"))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d", w.Code)
	}
}

// ──────────────────────────────────────────────
// GET /health
// ──────────────────────────────────────────────

func TestHealthEndpoint(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	req := httptest.NewRequest(http.MethodGet, "/health", nil)
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	var resp HealthCheckResponse
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("failed to decode response: %v", err)
	}

	if resp.Status != "healthy" {
		t.Errorf("expected status healthy, got %s", resp.Status)
	}
	if resp.UptimeSeconds < 0 {
		t.Errorf("expected non-negative uptime, got %d", resp.UptimeSeconds)
	}
	if resp.RegisteredModels != 3 {
		t.Errorf("expected 3 registered models, got %d", resp.RegisteredModels)
	}
	if resp.ActiveVariants != 5 {
		t.Errorf("expected 5 active variants, got %d", resp.ActiveVariants)
	}
}

func TestHealthContentType(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	req := httptest.NewRequest(http.MethodGet, "/health", nil)
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	ct := w.Header().Get("Content-Type")
	if ct != "application/json" {
		t.Errorf("expected Content-Type application/json, got %s", ct)
	}
}

// ──────────────────────────────────────────────
// GET /metrics
// ──────────────────────────────────────────────

func TestMetricsEndpoint(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	req := httptest.NewRequest(http.MethodGet, "/metrics", nil)
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	ct := w.Header().Get("Content-Type")
	if ct != "text/plain; version=0.0.4" {
		t.Errorf("expected prometheus content type, got %s", ct)
	}

	body := w.Body.String()
	expectedMetrics := []string{
		"prism_router_requests_total",
		"prism_router_outcomes_total",
		"prism_router_errors_total",
		"prism_router_uptime_seconds",
	}
	for _, m := range expectedMetrics {
		if !strings.Contains(body, m) {
			t.Errorf("expected metrics body to contain %s", m)
		}
	}

	// Should contain HELP and TYPE annotations
	if !strings.Contains(body, "# HELP") {
		t.Error("expected metrics to contain # HELP annotations")
	}
	if !strings.Contains(body, "# TYPE") {
		t.Error("expected metrics to contain # TYPE annotations")
	}
}

func TestMetricsCounterIncrement(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	// Make 3 route requests
	for i := 0; i < 3; i++ {
		body := `{"messages":[{"role":"user","content":"test"}]}`
		req := httptest.NewRequest(http.MethodPost, "/v1/route", strings.NewReader(body))
		w := httptest.NewRecorder()
		handler.ServeHTTP(w, req)
		if w.Code != http.StatusOK {
			t.Fatalf("route request %d failed: %d", i, w.Code)
		}
	}

	// Make 2 outcome requests
	for i := 0; i < 2; i++ {
		report := OutcomeReport{
			RequestID:  "req",
			VariantID:  "v1",
			StatusCode: 200,
			LatencyMs:  100,
		}
		bodyBytes, _ := json.Marshal(report)
		req := httptest.NewRequest(http.MethodPost, "/v1/outcome", bytes.NewReader(bodyBytes))
		w := httptest.NewRecorder()
		handler.ServeHTTP(w, req)
		if w.Code != http.StatusOK {
			t.Fatalf("outcome request %d failed: %d", i, w.Code)
		}
	}

	// Check metrics
	req := httptest.NewRequest(http.MethodGet, "/metrics", nil)
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)

	body := w.Body.String()
	if !strings.Contains(body, "prism_router_requests_total 3") {
		t.Errorf("expected requests_total 3, got:\n%s", body)
	}
	if !strings.Contains(body, "prism_router_outcomes_total 2") {
		t.Errorf("expected outcomes_total 2, got:\n%s", body)
	}
	if !strings.Contains(body, "prism_router_errors_total 0") {
		t.Errorf("expected errors_total 0, got:\n%s", body)
	}
}

// ──────────────────────────────────────────────
// Concurrency
// ──────────────────────────────────────────────

func TestConcurrentRouteRequests(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	const numRequests = 50
	var wg sync.WaitGroup
	errCh := make(chan error, numRequests)

	for i := 0; i < numRequests; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			body := `{"messages":[{"role":"user","content":"concurrent test"}]}`
			req := httptest.NewRequest(http.MethodPost, "/v1/route", strings.NewReader(body))
			req.Header.Set("Content-Type", "application/json")
			w := httptest.NewRecorder()

			handler.ServeHTTP(w, req)

			if w.Code != http.StatusOK {
				errCh <- &concurrencyError{code: w.Code, body: w.Body.String()}
				return
			}

			var resp RouteResponse
			if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
				errCh <- err
				return
			}
			if resp.Model != "gpt-4o" {
				errCh <- &concurrencyError{code: 0, body: "unexpected model: " + resp.Model}
			}
		}()
	}

	wg.Wait()
	close(errCh)

	for err := range errCh {
		t.Errorf("concurrent request error: %v", err)
	}

	// Check the route counter matches
	srv.mu.RLock()
	count := srv.routeCount
	srv.mu.RUnlock()

	if count != numRequests {
		t.Errorf("expected route count %d, got %d", numRequests, count)
	}
}

type concurrencyError struct {
	code int
	body string
}

func (e *concurrencyError) Error() string {
	return "status " + http.StatusText(e.code) + ": " + e.body
}

// ──────────────────────────────────────────────
// Latency
// ──────────────────────────────────────────────

func TestRouteResponseLatency(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	body := `{"messages":[{"role":"user","content":"latency test"}]}`
	req := httptest.NewRequest(http.MethodPost, "/v1/route", strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	start := time.Now()
	handler.ServeHTTP(w, req)
	elapsed := time.Since(start)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	if elapsed > 100*time.Millisecond {
		t.Errorf("response took %v, expected < 100ms", elapsed)
	}
}

// ──────────────────────────────────────────────
// convertToFFIRequest tests
// ──────────────────────────────────────────────

func TestConvertToFFIRequest(t *testing.T) {
	srv := newTestServer()

	t.Run("basic conversion", func(t *testing.T) {
		req := &RouteRequest{
			Messages: []ChatMessage{
				{Role: "system", Content: "you are helpful"},
				{Role: "user", Content: "hello"},
			},
		}
		ffiReq := srv.convertToFFIRequest(req)
		if len(ffiReq.Messages) != 2 {
			t.Fatalf("expected 2 messages, got %d", len(ffiReq.Messages))
		}
		if ffiReq.Messages[0].Role != "system" {
			t.Errorf("expected role system, got %s", ffiReq.Messages[0].Role)
		}
		if ffiReq.Messages[1].Content != "hello" {
			t.Errorf("expected content hello, got %s", ffiReq.Messages[1].Content)
		}
	})

	t.Run("strategy set", func(t *testing.T) {
		req := &RouteRequest{
			Messages: []ChatMessage{{Role: "user", Content: "test"}},
			Strategy: strPtr("cost_first"),
		}
		ffiReq := srv.convertToFFIRequest(req)
		if ffiReq.Strategy != "cost_first" {
			t.Errorf("expected strategy cost_first, got %s", ffiReq.Strategy)
		}
	})

	t.Run("model pinned", func(t *testing.T) {
		req := &RouteRequest{
			Messages: []ChatMessage{{Role: "user", Content: "test"}},
			Model:    strPtr("claude-3-opus"),
		}
		ffiReq := srv.convertToFFIRequest(req)
		if ffiReq.Strategy != "pinned:claude-3-opus" {
			t.Errorf("expected strategy pinned:claude-3-opus, got %s", ffiReq.Strategy)
		}
	})

	t.Run("model auto does not pin", func(t *testing.T) {
		req := &RouteRequest{
			Messages: []ChatMessage{{Role: "user", Content: "test"}},
			Model:    strPtr("auto"),
			Strategy: strPtr("balanced"),
		}
		ffiReq := srv.convertToFFIRequest(req)
		if ffiReq.Strategy != "balanced" {
			t.Errorf("expected strategy balanced (auto should not pin), got %s", ffiReq.Strategy)
		}
	})

	t.Run("pool ids", func(t *testing.T) {
		req := &RouteRequest{
			Messages: []ChatMessage{{Role: "user", Content: "test"}},
			PoolIDs:  []string{"premium", "default"},
		}
		ffiReq := srv.convertToFFIRequest(req)
		if len(ffiReq.PoolIDs) != 2 {
			t.Fatalf("expected 2 pool ids, got %d", len(ffiReq.PoolIDs))
		}
		if ffiReq.PoolIDs[0] != "premium" || ffiReq.PoolIDs[1] != "default" {
			t.Errorf("unexpected pool ids: %v", ffiReq.PoolIDs)
		}
	})

	t.Run("affinity and tenant", func(t *testing.T) {
		req := &RouteRequest{
			Messages:    []ChatMessage{{Role: "user", Content: "test"}},
			AffinityKey: strPtr("sess-1"),
			TenantID:    strPtr("tenant-x"),
		}
		ffiReq := srv.convertToFFIRequest(req)
		if ffiReq.AffinityKey != "sess-1" {
			t.Errorf("expected affinity_key sess-1, got %s", ffiReq.AffinityKey)
		}
		if ffiReq.TenantID != "tenant-x" {
			t.Errorf("expected tenant_id tenant-x, got %s", ffiReq.TenantID)
		}
	})

	t.Run("strategy params", func(t *testing.T) {
		cw := float32(0.7)
		qw := float32(0.2)
		lw := float32(0.1)
		ml := uint32(300)
		mt := uint32(100)
		req := &RouteRequest{
			Messages: []ChatMessage{{Role: "user", Content: "test"}},
			StrategyParams: &StrategyParams{
				CostWeight:    &cw,
				QualityWeight: &qw,
				LatencyWeight: &lw,
				MaxLatencyMs:  &ml,
				MaxTTFTMs:     &mt,
			},
		}
		ffiReq := srv.convertToFFIRequest(req)
		if ffiReq.CostWeight == nil || *ffiReq.CostWeight != 0.7 {
			t.Errorf("expected cost_weight 0.7, got %v", ffiReq.CostWeight)
		}
		if ffiReq.QualityWeight == nil || *ffiReq.QualityWeight != 0.2 {
			t.Errorf("expected quality_weight 0.2, got %v", ffiReq.QualityWeight)
		}
		if ffiReq.LatencyWeight == nil || *ffiReq.LatencyWeight != 0.1 {
			t.Errorf("expected latency_weight 0.1, got %v", ffiReq.LatencyWeight)
		}
		if ffiReq.MaxLatencyMs == nil || *ffiReq.MaxLatencyMs != 300 {
			t.Errorf("expected max_latency_ms 300, got %v", ffiReq.MaxLatencyMs)
		}
		if ffiReq.MaxTTFTMs == nil || *ffiReq.MaxTTFTMs != 100 {
			t.Errorf("expected max_ttft_ms 100, got %v", ffiReq.MaxTTFTMs)
		}
	})
}

// ──────────────────────────────────────────────
// convertFromFFIResult tests
// ──────────────────────────────────────────────

func TestConvertFromFFIResult(t *testing.T) {
	srv := newTestServer()

	t.Run("basic result", func(t *testing.T) {
		result := &ffi.RouteResult{
			Model:          "gpt-4o",
			Provider:       "openai",
			Variant:        "gpt-4o@openai",
			Account:        "openai-acct",
			Score:          0.95,
			DecisionTimeUs: 42,
			Reason:         "balanced",
		}
		req := &RouteRequest{
			Messages: []ChatMessage{{Role: "user", Content: "test"}},
		}
		resp := srv.convertFromFFIResult(result, req)

		if resp.Model != "gpt-4o" {
			t.Errorf("expected model gpt-4o, got %s", resp.Model)
		}
		if resp.Score != 0.95 {
			t.Errorf("expected score 0.95, got %f", resp.Score)
		}
		if resp.DecisionTimeUs != 42 {
			t.Errorf("expected decision_time_us 42, got %d", resp.DecisionTimeUs)
		}
		if resp.Upstream == nil {
			t.Fatal("expected upstream to be non-nil")
		}
		if resp.Upstream.VariantID != "gpt-4o@openai" {
			t.Errorf("expected upstream variant_id gpt-4o@openai, got %s", resp.Upstream.VariantID)
		}
	})

	t.Run("with fallback chain", func(t *testing.T) {
		result := &ffi.RouteResult{
			Model:         "gpt-4o",
			Provider:      "openai",
			Variant:       "gpt-4o@openai",
			Account:       "openai-acct",
			FallbackChain: []string{"claude-3@anthropic", "gemini@google"},
			Reason:        "balanced",
		}
		req := &RouteRequest{
			Messages: []ChatMessage{{Role: "user", Content: "test"}},
		}
		resp := srv.convertFromFFIResult(result, req)

		if len(resp.FallbackChain) != 2 {
			t.Fatalf("expected 2 fallbacks, got %d", len(resp.FallbackChain))
		}
		if resp.FallbackChain[0].VariantID != "claude-3@anthropic" {
			t.Errorf("expected fallback[0] claude-3@anthropic, got %s", resp.FallbackChain[0].VariantID)
		}
		if resp.FallbackChain[1].VariantID != "gemini@google" {
			t.Errorf("expected fallback[1] gemini@google, got %s", resp.FallbackChain[1].VariantID)
		}
	})

	t.Run("reason populated", func(t *testing.T) {
		result := &ffi.RouteResult{
			Model:    "gpt-4o",
			Provider: "openai",
			Variant:  "gpt-4o@openai",
			Account:  "openai-acct",
			Reason:   "balanced scoring",
		}
		req := &RouteRequest{Messages: []ChatMessage{{Role: "user", Content: "test"}}}
		resp := srv.convertFromFFIResult(result, req)

		if resp.Reason == nil {
			t.Fatal("expected reason to be non-nil")
		}
		if resp.Reason.L1Model != "gpt-4o" {
			t.Errorf("expected reason l1_model gpt-4o, got %s", resp.Reason.L1Model)
		}
		if resp.Reason.L2Provider != "openai" {
			t.Errorf("expected reason l2_provider openai, got %s", resp.Reason.L2Provider)
		}
	})

	t.Run("no reason when empty", func(t *testing.T) {
		result := &ffi.RouteResult{
			Model:    "gpt-4o",
			Provider: "openai",
			Variant:  "gpt-4o@openai",
			Account:  "openai-acct",
			Reason:   "",
		}
		req := &RouteRequest{Messages: []ChatMessage{{Role: "user", Content: "test"}}}
		resp := srv.convertFromFFIResult(result, req)

		if resp.Reason != nil {
			t.Error("expected reason to be nil when empty")
		}
	})

	t.Run("metadata passthrough", func(t *testing.T) {
		result := &ffi.RouteResult{
			Model: "gpt-4o", Provider: "openai", Variant: "gpt-4o@openai", Account: "a",
		}
		req := &RouteRequest{
			Messages: []ChatMessage{{Role: "user", Content: "test"}},
			Metadata: map[string]string{"key": "value"},
		}
		resp := srv.convertFromFFIResult(result, req)

		if resp.ResponseMetadata["key"] != "value" {
			t.Errorf("expected metadata key=value, got %v", resp.ResponseMetadata)
		}
	})

	t.Run("no metadata when nil", func(t *testing.T) {
		result := &ffi.RouteResult{
			Model: "gpt-4o", Provider: "openai", Variant: "gpt-4o@openai", Account: "a",
		}
		req := &RouteRequest{
			Messages: []ChatMessage{{Role: "user", Content: "test"}},
		}
		resp := srv.convertFromFFIResult(result, req)

		if resp.ResponseMetadata != nil {
			t.Error("expected no response_metadata when request metadata is nil")
		}
	})
}

// ──────────────────────────────────────────────
// reportOutcome
// ──────────────────────────────────────────────

func TestReportOutcome(t *testing.T) {
	srv := newTestServer()

	report := &OutcomeReport{
		RequestID:    "req-1",
		VariantID:    "gpt-4o@openai",
		StatusCode:   200,
		LatencyMs:    150,
		TTFTMs:       30,
		InputTokens:  100,
		OutputTokens: 500,
	}

	if err := srv.reportOutcome(report); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

// ──────────────────────────────────────────────
// Method not allowed / not found
// ──────────────────────────────────────────────

func TestMethodNotAllowed(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	// GET on a POST-only endpoint
	req := httptest.NewRequest(http.MethodGet, "/v1/route", nil)
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)

	// Go 1.22+ mux returns 405 for method mismatch
	if w.Code != http.StatusMethodNotAllowed {
		t.Logf("GET /v1/route returned %d (expected 405)", w.Code)
	}
}

func TestNotFoundEndpoint(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	req := httptest.NewRequest(http.MethodGet, "/nonexistent", nil)
	w := httptest.NewRecorder()
	handler.ServeHTTP(w, req)

	if w.Code != http.StatusNotFound {
		t.Errorf("expected 404, got %d", w.Code)
	}
}

// ──────────────────────────────────────────────
// Full request/response JSON round-trip
// ──────────────────────────────────────────────

func TestRouteFullJSONRoundTrip(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	jsonReq := `{
		"messages": [
			{"role": "system", "content": "You are a helpful assistant."},
			{"role": "user", "content": "What is 2+2?"}
		],
		"strategy": "balanced",
		"pool_ids": ["default"],
		"affinity_key": "session-abc",
		"tenant_id": "org-123",
		"strategy_params": {
			"cost_weight": 0.3,
			"quality_weight": 0.5,
			"latency_weight": 0.2
		},
		"metadata": {"trace_id": "t1"}
	}`

	req := httptest.NewRequest(http.MethodPost, "/v1/route", strings.NewReader(jsonReq))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d: %s", w.Code, w.Body.String())
	}

	// Verify response is valid JSON
	respBody := w.Body.Bytes()
	if !json.Valid(respBody) {
		t.Fatal("response is not valid JSON")
	}

	var resp RouteResponse
	if err := json.Unmarshal(respBody, &resp); err != nil {
		t.Fatalf("failed to unmarshal: %v", err)
	}

	// Verify all expected fields
	if resp.Model == "" {
		t.Error("model should not be empty")
	}
	if resp.Provider == "" {
		t.Error("provider should not be empty")
	}
	if resp.Variant == "" {
		t.Error("variant should not be empty")
	}
	if resp.Upstream == nil {
		t.Error("upstream should not be nil")
	}
	if resp.ResponseMetadata["trace_id"] != "t1" {
		t.Errorf("expected trace_id t1, got %v", resp.ResponseMetadata)
	}
}

// ──────────────────────────────────────────────
// writeJSON / writeError helpers
// ──────────────────────────────────────────────

func TestWriteJSON(t *testing.T) {
	w := httptest.NewRecorder()
	data := map[string]string{"key": "value"}
	writeJSON(w, http.StatusCreated, data)

	if w.Code != http.StatusCreated {
		t.Errorf("expected 201, got %d", w.Code)
	}
	if w.Header().Get("Content-Type") != "application/json" {
		t.Errorf("expected application/json content type")
	}

	var result map[string]string
	if err := json.Unmarshal(w.Body.Bytes(), &result); err != nil {
		t.Fatalf("failed to decode: %v", err)
	}
	if result["key"] != "value" {
		t.Errorf("expected key=value, got %v", result)
	}
}

func TestWriteError(t *testing.T) {
	w := httptest.NewRecorder()
	writeError(w, http.StatusBadRequest, "something went wrong")

	if w.Code != http.StatusBadRequest {
		t.Errorf("expected 400, got %d", w.Code)
	}

	var errResp errorResponse
	if err := json.Unmarshal(w.Body.Bytes(), &errResp); err != nil {
		t.Fatalf("failed to decode: %v", err)
	}
	if errResp.Error != "something went wrong" {
		t.Errorf("expected error 'something went wrong', got %s", errResp.Error)
	}
}

// ──────────────────────────────────────────────
// Large batch handling
// ──────────────────────────────────────────────

func TestRouteBatchLarge(t *testing.T) {
	srv := newTestServer()
	handler := srv.Handler()

	requests := make([]RouteRequest, 100)
	for i := range requests {
		requests[i] = RouteRequest{
			Messages: []ChatMessage{{Role: "user", Content: "batch item"}},
		}
	}

	reqBody := RouteBatchRequest{Requests: requests}
	bodyBytes, _ := json.Marshal(reqBody)
	req := httptest.NewRequest(http.MethodPost, "/v1/route/batch", bytes.NewReader(bodyBytes))
	w := httptest.NewRecorder()

	handler.ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	var resp RouteBatchResponse
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("failed to decode: %v", err)
	}
	if len(resp.Responses) != 100 {
		t.Errorf("expected 100 responses, got %d", len(resp.Responses))
	}
}

// ──────────────────────────────────────────────
// Benchmark
// ──────────────────────────────────────────────

func BenchmarkRoute(b *testing.B) {
	srv := newTestServer()
	handler := srv.Handler()
	body := []byte(`{"messages":[{"role":"user","content":"benchmark"}]}`)

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		req := httptest.NewRequest(http.MethodPost, "/v1/route", bytes.NewReader(body))
		w := httptest.NewRecorder()
		handler.ServeHTTP(w, req)
		io.Copy(io.Discard, w.Result().Body)
	}
}
