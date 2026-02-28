//go:build e2e

package e2e

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

	"github.com/prometheus/client_golang/prometheus/promhttp"

	"go.zoe.im/prism-gateway/internal/abtesting"
	"go.zoe.im/prism-gateway/internal/gateway"
	"go.zoe.im/prism-gateway/internal/provider"
	"go.zoe.im/prism-gateway/internal/router"
	"go.zoe.im/prism-gateway/pkg/api"
)

// ──────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────

func setupTestServer(t *testing.T) *httptest.Server {
	t.Helper()
	return setupTestServerWithRouter(t, &router.StaticRouter{
		ModelID:    "gpt-4o",
		ProviderID: "openai",
		VariantID:  "gpt-4o@openai",
		AccountID:  "default-acct",
	})
}

func setupTestServerWithRouter(t *testing.T, r router.Router) *httptest.Server {
	t.Helper()

	providers := map[string]provider.Provider{
		"openai":    provider.NewMockProvider("openai"),
		"anthropic": provider.NewMockProvider("anthropic"),
		"deepseek":  provider.NewMockProvider("deepseek"),
	}

	h := gateway.NewHandler(r, providers)
	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/chat/completions", h.ChatCompletion)
	mux.HandleFunc("POST /v1/outcomes", h.ReportOutcome)
	mux.HandleFunc("GET /health", h.Health)

	return httptest.NewServer(mux)
}

func makeRequest(t *testing.T, url string, req *api.ChatCompletionRequest, headers map[string]string) *http.Response {
	t.Helper()

	body, err := json.Marshal(req)
	if err != nil {
		t.Fatalf("Failed to marshal request: %v", err)
	}

	httpReq, err := http.NewRequest("POST", url+"/v1/chat/completions", bytes.NewReader(body))
	if err != nil {
		t.Fatalf("Failed to create request: %v", err)
	}
	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Authorization", "Bearer sk-test-key")

	for k, v := range headers {
		httpReq.Header.Set(k, v)
	}

	resp, err := http.DefaultClient.Do(httpReq)
	if err != nil {
		t.Fatalf("Request failed: %v", err)
	}
	return resp
}

func simpleReq(content string) *api.ChatCompletionRequest {
	return &api.ChatCompletionRequest{
		Model: "auto",
		Messages: []api.Message{
			{Role: "user", Content: content},
		},
	}
}

// ──────────────────────────────────────────────
// Health
// ──────────────────────────────────────────────

func TestHealthEndpoint(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	resp, err := http.Get(ts.URL + "/health")
	if err != nil {
		t.Fatalf("Health check failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected 200, got %d", resp.StatusCode)
	}
}

// ──────────────────────────────────────────────
// Full request flow
// ──────────────────────────────────────

func TestFullRequestFlow(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	resp := makeRequest(t, ts.URL, simpleReq("What is 2+2?"), nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	// Verify response body
	var chatResp api.ChatCompletionResponse
	if err := json.NewDecoder(resp.Body).Decode(&chatResp); err != nil {
		t.Fatalf("Failed to decode response: %v", err)
	}
	if len(chatResp.Choices) == 0 {
		t.Error("Expected at least one choice")
	}
	if chatResp.Choices[0].Message == nil {
		t.Error("Expected message in choice")
	}
}

// ──────────────────────────────────────────────
// Streaming
// ──────────────────────────────────────────────

func TestStreamingFlow(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	req := &api.ChatCompletionRequest{
		Model:  "auto",
		Stream: true,
		Messages: []api.Message{
			{Role: "user", Content: "Tell me a joke"},
		},
	}

	resp := makeRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	ct := resp.Header.Get("Content-Type")
	if !strings.Contains(ct, "text/event-stream") {
		t.Errorf("Expected text/event-stream, got %s", ct)
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("Failed to read stream: %v", err)
	}
	bodyStr := string(body)

	if !strings.Contains(bodyStr, "data: ") {
		t.Error("Stream should contain 'data: ' lines")
	}
	if !strings.Contains(bodyStr, "[DONE]") {
		t.Error("Stream should end with [DONE]")
	}

	// Verify each SSE chunk is valid JSON
	for _, line := range strings.Split(bodyStr, "\n") {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "data: ") && !strings.Contains(line, "[DONE]") {
			jsonStr := strings.TrimPrefix(line, "data: ")
			var chunk api.ChatCompletionChunk
			if err := json.Unmarshal([]byte(jsonStr), &chunk); err != nil {
				t.Errorf("Invalid SSE chunk JSON: %s, err: %v", jsonStr, err)
			}
		}
	}
}

func TestStreamingHasRoutingHeaders(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	req := &api.ChatCompletionRequest{
		Model:  "auto",
		Stream: true,
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
	}

	resp := makeRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	// Streaming responses should also have routing headers
	if model := resp.Header.Get(api.HeaderPrismModel); model == "" {
		t.Error("Streaming response missing X-Prism-Model header")
	}
	if prov := resp.Header.Get(api.HeaderPrismProvider); prov == "" {
		t.Error("Streaming response missing X-Prism-Provider header")
	}
}

// ──────────────────────────────────────────────
// Routing headers
// ──────────────────────────────────────────────

func TestRoutingHeadersInResponse(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), nil)
	defer resp.Body.Close()

	expectedHeaders := []string{
		api.HeaderPrismModel,
		api.HeaderPrismProvider,
		api.HeaderPrismVariant,
		api.HeaderPrismAccount,
		api.HeaderPrismDecisionTime,
		api.HeaderPrismReason,
	}

	for _, h := range expectedHeaders {
		if v := resp.Header.Get(h); v == "" {
			t.Errorf("Missing response header: %s", h)
		}
	}
}

func TestRoutingHeaderValues(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), nil)
	defer resp.Body.Close()

	// StaticRouter always returns these values
	if got := resp.Header.Get(api.HeaderPrismModel); got != "gpt-4o" {
		t.Errorf("Expected model gpt-4o, got %s", got)
	}
	if got := resp.Header.Get(api.HeaderPrismProvider); got != "openai" {
		t.Errorf("Expected provider openai, got %s", got)
	}
	if got := resp.Header.Get(api.HeaderPrismVariant); got != "gpt-4o@openai" {
		t.Errorf("Expected variant gpt-4o@openai, got %s", got)
	}
	if got := resp.Header.Get(api.HeaderPrismAccount); got != "default-acct" {
		t.Errorf("Expected account default-acct, got %s", got)
	}
}

// ──────────────────────────────────────────────
// Strategy override
// ──────────────────────────────────────────────

func TestStrategyOverrideViaHeader(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	headers := map[string]string{
		api.HeaderPrismStrategy: "cost_first",
	}

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), headers)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}
}

func TestStrategyOverrideViaBody(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	req := &api.ChatCompletionRequest{
		Model: "auto",
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
		Prism: &api.PrismExtension{
			Strategy: "slo_first",
		},
	}

	resp := makeRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}
}

func TestHeaderOverridesBodyStrategy(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	req := &api.ChatCompletionRequest{
		Model: "auto",
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
		Prism: &api.PrismExtension{
			Strategy: "quality_first",
		},
	}

	headers := map[string]string{
		api.HeaderPrismStrategy: "cost_first",
	}

	resp := makeRequest(t, ts.URL, req, headers)
	defer resp.Body.Close()

	// Should succeed — header strategy takes priority over body
	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}
}

// ──────────────────────────────────────────────
// Pool & affinity headers
// ──────────────────────────────────────────────

func TestPoolIDViaHeader(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	headers := map[string]string{
		api.HeaderPrismModelPool: "tier-1",
	}

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), headers)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected 200, got %d", resp.StatusCode)
	}
}

func TestPoolIDViaBody(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	req := &api.ChatCompletionRequest{
		Model: "auto",
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
		Prism: &api.PrismExtension{
			PoolIDs: []string{"tier-1", "tier-2"},
		},
	}

	resp := makeRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected 200, got %d", resp.StatusCode)
	}
}

func TestAffinityKeyViaHeader(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	headers := map[string]string{
		api.HeaderPrismAffinityKey: "user-12345",
	}

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), headers)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected 200, got %d", resp.StatusCode)
	}
}

func TestSessionIDAsAffinity(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	headers := map[string]string{
		api.HeaderPrismSessionID: "session-abc-123",
	}

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), headers)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected 200, got %d", resp.StatusCode)
	}
}

// ──────────────────────────────────────────────
// Pinned model
// ──────────────────────────────────────────────

func TestPinnedModelViaModelField(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	req := &api.ChatCompletionRequest{
		Model: "gpt-4o-mini", // not "auto" — pinned mode
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
	}

	resp := makeRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	// With StaticRouter this still succeeds. When real router is integrated,
	// the model field should be used as pinned model.
	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}
}

// ──────────────────────────────────────────────
// Provider routing (multi-provider)
// ──────────────────────────────────────────────

func TestRouteToCorrectProvider(t *testing.T) {
	// Use a router that routes to anthropic
	r := &router.StaticRouter{
		ModelID:    "claude-sonnet",
		ProviderID: "anthropic",
		VariantID:  "claude@anthropic",
		AccountID:  "anthropic-acct",
	}

	ts := setupTestServerWithRouter(t, r)
	defer ts.Close()

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	if got := resp.Header.Get(api.HeaderPrismProvider); got != "anthropic" {
		t.Errorf("Expected provider anthropic, got %s", got)
	}
}

func TestProviderNotFoundReturns502(t *testing.T) {
	// Route to a provider that doesn't exist
	r := &router.StaticRouter{
		ModelID:    "some-model",
		ProviderID: "nonexistent-provider",
		VariantID:  "variant",
		AccountID:  "acct",
	}

	ts := setupTestServerWithRouter(t, r)
	defer ts.Close()

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadGateway {
		t.Errorf("Expected 502, got %d", resp.StatusCode)
	}
}

// ──────────────────────────────────────────────
// Provider error handling
// ──────────────────────────────────────────────

func TestProviderErrorReturns502(t *testing.T) {
	mockProv := provider.NewMockProvider("openai").
		WithError("gpt-4o", io.ErrUnexpectedEOF)

	r := &router.StaticRouter{
		ModelID:    "gpt-4o",
		ProviderID: "openai",
		VariantID:  "gpt-4o@openai",
		AccountID:  "acct",
	}

	h := gateway.NewHandler(r, map[string]provider.Provider{"openai": mockProv})
	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/chat/completions", h.ChatCompletion)
	ts := httptest.NewServer(mux)
	defer ts.Close()

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadGateway {
		t.Errorf("Expected 502 on provider error, got %d", resp.StatusCode)
	}

	// Error response should be JSON
	var errResp map[string]interface{}
	if err := json.NewDecoder(resp.Body).Decode(&errResp); err != nil {
		t.Errorf("Error response should be valid JSON: %v", err)
	}
	if _, ok := errResp["error"]; !ok {
		t.Error("Error response should have 'error' field")
	}
}

func TestStreamProviderErrorReturns502(t *testing.T) {
	mockProv := provider.NewMockProvider("openai").
		WithError("gpt-4o", io.ErrUnexpectedEOF)

	r := &router.StaticRouter{
		ModelID:    "gpt-4o",
		ProviderID: "openai",
		VariantID:  "gpt-4o@openai",
		AccountID:  "acct",
	}

	h := gateway.NewHandler(r, map[string]provider.Provider{"openai": mockProv})
	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/chat/completions", h.ChatCompletion)
	ts := httptest.NewServer(mux)
	defer ts.Close()

	req := &api.ChatCompletionRequest{
		Model:  "gpt-4o",
		Stream: true,
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
	}

	resp := makeRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadGateway {
		t.Errorf("Expected 502 on stream provider error, got %d", resp.StatusCode)
	}
}

// ──────────────────────────────────────────────
// Router error handling
// ──────────────────────────────────────────────

type errorRouter struct {
	err error
}

func (r *errorRouter) Route(req *router.RouteRequest) (*router.RouteResult, error) {
	return nil, r.err
}

func TestRouterErrorReturns503(t *testing.T) {
	r := &errorRouter{err: io.ErrUnexpectedEOF}

	ts := setupTestServerWithRouter(t, r)
	defer ts.Close()

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusServiceUnavailable {
		t.Errorf("Expected 503, got %d", resp.StatusCode)
	}
}

// ──────────────────────────────────────────────
// Invalid requests
// ──────────────────────────────────────────────

func TestInvalidJSONReturns400(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	httpReq, _ := http.NewRequest("POST", ts.URL+"/v1/chat/completions", strings.NewReader("{invalid"))
	httpReq.Header.Set("Content-Type", "application/json")

	resp, err := http.DefaultClient.Do(httpReq)
	if err != nil {
		t.Fatalf("Request failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("Expected 400, got %d", resp.StatusCode)
	}
}

func TestEmptyBodyReturns400(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	httpReq, _ := http.NewRequest("POST", ts.URL+"/v1/chat/completions", strings.NewReader(""))
	httpReq.Header.Set("Content-Type", "application/json")

	resp, err := http.DefaultClient.Do(httpReq)
	if err != nil {
		t.Fatalf("Request failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("Expected 400, got %d", resp.StatusCode)
	}
}

// ──────────────────────────────────────────────
// Prism body extension
// ──────────────────────────────────────────────

func TestPrismBodyExtensionFull(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	costW := float32(0.2)
	qualW := float32(0.6)
	latW := float32(0.2)

	req := &api.ChatCompletionRequest{
		Model: "auto",
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
		Prism: &api.PrismExtension{
			Strategy:      "balanced",
			PoolIDs:       []string{"tier-1", "tier-2"},
			Exclude:       []string{"gpt-3.5-turbo"},
			Trace:         true,
			CostWeight:    &costW,
			QualityWeight: &qualW,
			LatencyWeight: &latW,
			SLO: &api.SLOConstraint{
				MaxLatencyMs: 2000,
				MaxTTFTMs:    500,
			},
		},
	}

	resp := makeRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}
}

// ──────────────────────────────────────────────
// Concurrent requests
// ──────────────────────────────────────────────

func TestConcurrentRouting(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	const concurrency = 100
	var wg sync.WaitGroup
	errors := make([]error, concurrency)

	for i := 0; i < concurrency; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			resp := makeRequest(t, ts.URL, simpleReq("Concurrent request"), nil)
			defer resp.Body.Close()

			if resp.StatusCode != http.StatusOK {
				body, _ := io.ReadAll(resp.Body)
				errors[i] = io.ErrUnexpectedEOF
				t.Errorf("Request %d failed with %d: %s", i, resp.StatusCode, string(body))
			}
			// Drain body
			io.ReadAll(resp.Body)
		}(i)
	}

	wg.Wait()

	failCount := 0
	for _, err := range errors {
		if err != nil {
			failCount++
		}
	}
	if failCount > 0 {
		t.Errorf("%d/%d concurrent requests failed", failCount, concurrency)
	}
}

func TestConcurrentStreaming(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	const concurrency = 30
	var wg sync.WaitGroup

	for i := 0; i < concurrency; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			req := &api.ChatCompletionRequest{
				Model:  "auto",
				Stream: true,
				Messages: []api.Message{
					{Role: "user", Content: "Stream concurrent"},
				},
			}
			resp := makeRequest(t, ts.URL, req, nil)
			defer resp.Body.Close()

			if resp.StatusCode != http.StatusOK {
				t.Errorf("Streaming request %d failed with %d", i, resp.StatusCode)
				return
			}
			body, _ := io.ReadAll(resp.Body)
			if !strings.Contains(string(body), "[DONE]") {
				t.Errorf("Streaming request %d missing [DONE]", i)
			}
		}(i)
	}

	wg.Wait()
}

// ──────────────────────────────────────────────
// Latency
// ──────────────────────────────────────────────

func TestResponseLatencyUnder100ms(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	start := time.Now()
	resp := makeRequest(t, ts.URL, simpleReq("Hello"), nil)
	elapsed := time.Since(start)
	defer resp.Body.Close()

	// With mock provider, total round trip should be well under 100ms
	if elapsed > 100*time.Millisecond {
		t.Errorf("Response took %v, expected < 100ms", elapsed)
	}
}

// ──────────────────────────────────────────────
// Fallback chain in response
// ──────────────────────────────────────────────

func TestFallbackChainHeader(t *testing.T) {
	r := &router.StaticRouter{
		ModelID:    "gpt-4o",
		ProviderID: "openai",
		VariantID:  "gpt-4o@openai",
		AccountID:  "acct",
	}
	// StaticRouter has nil fallback, so header should be absent
	ts := setupTestServerWithRouter(t, r)
	defer ts.Close()

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), nil)
	defer resp.Body.Close()

	// No fallback chain → header should be empty
	if got := resp.Header.Get(api.HeaderPrismFallback); got != "" {
		t.Errorf("Expected empty fallback header for static router, got %s", got)
	}
}

func TestFallbackChainWithFallbacks(t *testing.T) {
	r := &routerWithFallbacks{
		primary: router.RouteResult{
			Model:    "gpt-4o",
			Provider: "openai",
			Variant:  "gpt-4o@openai",
			Account:  "acct",
			FallbackChain: []string{"claude@anthropic", "ds-v3@deepseek"},
			Reason:   "test",
			Score:    0.9,
		},
	}

	ts := setupTestServerWithRouter(t, r)
	defer ts.Close()

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), nil)
	defer resp.Body.Close()

	fb := resp.Header.Get(api.HeaderPrismFallback)
	if fb == "" {
		t.Error("Expected fallback chain header to be set")
	}

	var chain []string
	if err := json.Unmarshal([]byte(fb), &chain); err != nil {
		t.Fatalf("Failed to parse fallback chain JSON: %v", err)
	}
	if len(chain) != 2 {
		t.Errorf("Expected 2 fallbacks, got %d", len(chain))
	}
}

type routerWithFallbacks struct {
	primary router.RouteResult
}

func (r *routerWithFallbacks) Route(req *router.RouteRequest) (*router.RouteResult, error) {
	result := r.primary
	return &result, nil
}

// ──────────────────────────────────────────────
// Multi-turn conversation
// ──────────────────────────────────────────────

func TestMultiTurnConversation(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	req := &api.ChatCompletionRequest{
		Model: "auto",
		Messages: []api.Message{
			{Role: "system", Content: "You are a helpful assistant."},
			{Role: "user", Content: "What is Rust?"},
			{Role: "assistant", Content: "Rust is a systems programming language."},
			{Role: "user", Content: "How does it compare to Go?"},
		},
	}

	resp := makeRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}
}

// ──────────────────────────────────────────────
// Content-Type validation
// ──────────────────────────────────────────────

func TestResponseContentTypeJSON(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), nil)
	defer resp.Body.Close()

	ct := resp.Header.Get("Content-Type")
	if !strings.Contains(ct, "application/json") {
		t.Errorf("Expected application/json, got %s", ct)
	}
}

func TestErrorResponseContentTypeJSON(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	httpReq, _ := http.NewRequest("POST", ts.URL+"/v1/chat/completions", strings.NewReader("{bad"))
	httpReq.Header.Set("Content-Type", "application/json")

	resp, err := http.DefaultClient.Do(httpReq)
	if err != nil {
		t.Fatalf("Request failed: %v", err)
	}
	defer resp.Body.Close()

	ct := resp.Header.Get("Content-Type")
	if !strings.Contains(ct, "application/json") {
		t.Errorf("Error response should be application/json, got %s", ct)
	}
}

// ──────────────────────────────────────────────
// Capture router for strategy/context tests
// ──────────────────────────────────────────────

// captureRouter records the RouteRequest for inspection.
type captureRouter struct {
	mu       sync.Mutex
	lastReq  *router.RouteRequest
	result   router.RouteResult
}

func newCaptureRouter() *captureRouter {
	return &captureRouter{
		result: router.RouteResult{
			Model:    "gpt-4o",
			Provider: "openai",
			Variant:  "gpt-4o@openai",
			Account:  "default-acct",
			Reason:   "captured",
			Score:    1.0,
		},
	}
}

func (r *captureRouter) Route(req *router.RouteRequest) (*router.RouteResult, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.lastReq = req
	result := r.result
	return &result, nil
}

func (r *captureRouter) getLastRequest() *router.RouteRequest {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.lastReq
}

// ──────────────────────────────────────────────
// Strategy tests
// ──────────────────────────────────────────────

func TestStrategyBalancedWithWeights(t *testing.T) {
	cr := newCaptureRouter()
	ts := setupTestServerWithRouter(t, cr)
	defer ts.Close()

	costW := float32(0.2)
	qualW := float32(0.6)
	latW := float32(0.2)

	req := &api.ChatCompletionRequest{
		Model: "auto",
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
		Prism: &api.PrismExtension{
			Strategy:      "balanced",
			CostWeight:    &costW,
			QualityWeight: &qualW,
			LatencyWeight: &latW,
		},
	}

	resp := makeRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	lastReq := cr.getLastRequest()
	if lastReq == nil {
		t.Fatal("expected captured route request")
	}

	if lastReq.Strategy != "balanced" {
		t.Errorf("expected strategy 'balanced', got '%s'", lastReq.Strategy)
	}
	if lastReq.CostWeight == nil || *lastReq.CostWeight != 0.2 {
		t.Errorf("expected cost_weight 0.2, got %v", lastReq.CostWeight)
	}
	if lastReq.QualityWeight == nil || *lastReq.QualityWeight != 0.6 {
		t.Errorf("expected quality_weight 0.6, got %v", lastReq.QualityWeight)
	}
	if lastReq.LatencyWeight == nil || *lastReq.LatencyWeight != 0.2 {
		t.Errorf("expected latency_weight 0.2, got %v", lastReq.LatencyWeight)
	}
}

func TestStrategyBalancedWithWeightsFromHeaders(t *testing.T) {
	cr := newCaptureRouter()
	ts := setupTestServerWithRouter(t, cr)
	defer ts.Close()

	headers := map[string]string{
		api.HeaderPrismStrategy:   "balanced",
		api.HeaderPrismCostWeight: "0.1",
		api.HeaderPrismQualWeight: "0.8",
		api.HeaderPrismLatWeight:  "0.1",
	}

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), headers)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	lastReq := cr.getLastRequest()
	if lastReq == nil {
		t.Fatal("expected captured route request")
	}

	if lastReq.Strategy != "balanced" {
		t.Errorf("expected strategy 'balanced', got '%s'", lastReq.Strategy)
	}
	if lastReq.CostWeight == nil {
		t.Fatal("expected non-nil cost_weight from header")
	}
	if *lastReq.CostWeight < 0.09 || *lastReq.CostWeight > 0.11 {
		t.Errorf("expected cost_weight ~0.1, got %f", *lastReq.CostWeight)
	}
	if lastReq.QualityWeight == nil {
		t.Fatal("expected non-nil quality_weight from header")
	}
	if *lastReq.QualityWeight < 0.79 || *lastReq.QualityWeight > 0.81 {
		t.Errorf("expected quality_weight ~0.8, got %f", *lastReq.QualityWeight)
	}
}

func TestStrategyCostFirst(t *testing.T) {
	cr := newCaptureRouter()
	ts := setupTestServerWithRouter(t, cr)
	defer ts.Close()

	headers := map[string]string{
		api.HeaderPrismStrategy: "cost_first",
	}

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), headers)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	lastReq := cr.getLastRequest()
	if lastReq == nil {
		t.Fatal("expected captured route request")
	}

	if lastReq.Strategy != "cost_first" {
		t.Errorf("expected strategy 'cost_first', got '%s'", lastReq.Strategy)
	}
}

func TestStrategySloFirst(t *testing.T) {
	cr := newCaptureRouter()
	ts := setupTestServerWithRouter(t, cr)
	defer ts.Close()

	headers := map[string]string{
		api.HeaderPrismStrategy:     "slo_first",
		api.HeaderPrismMaxLatencyMs: "200",
		api.HeaderPrismMaxTTFTMs:    "100",
	}

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), headers)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	lastReq := cr.getLastRequest()
	if lastReq == nil {
		t.Fatal("expected captured route request")
	}

	if lastReq.Strategy != "slo_first" {
		t.Errorf("expected strategy 'slo_first', got '%s'", lastReq.Strategy)
	}
	if lastReq.MaxLatencyMs == nil || *lastReq.MaxLatencyMs != 200 {
		t.Errorf("expected max_latency_ms 200, got %v", lastReq.MaxLatencyMs)
	}
	if lastReq.MaxTTFTMs == nil || *lastReq.MaxTTFTMs != 100 {
		t.Errorf("expected max_ttft_ms 100, got %v", lastReq.MaxTTFTMs)
	}
}

func TestStrategySloFirstFromBody(t *testing.T) {
	cr := newCaptureRouter()
	ts := setupTestServerWithRouter(t, cr)
	defer ts.Close()

	req := &api.ChatCompletionRequest{
		Model: "auto",
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
		Prism: &api.PrismExtension{
			Strategy: "slo_first",
			SLO: &api.SLOConstraint{
				MaxLatencyMs: 300,
				MaxTTFTMs:    150,
			},
		},
	}

	resp := makeRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	lastReq := cr.getLastRequest()
	if lastReq == nil {
		t.Fatal("expected captured route request")
	}

	if lastReq.MaxLatencyMs == nil || *lastReq.MaxLatencyMs != 300 {
		t.Errorf("expected max_latency_ms 300 from body SLO, got %v", lastReq.MaxLatencyMs)
	}
	if lastReq.MaxTTFTMs == nil || *lastReq.MaxTTFTMs != 150 {
		t.Errorf("expected max_ttft_ms 150 from body SLO, got %v", lastReq.MaxTTFTMs)
	}
}

// ──────────────────────────────────────────────
// AnalyzeContext tests
// ──────────────────────────────────────────────

func TestAnalyzeContextHeaders(t *testing.T) {
	cr := newCaptureRouter()
	ts := setupTestServerWithRouter(t, cr)
	defer ts.Close()

	headers := map[string]string{
		api.HeaderPrismCallerType: "service",
		api.HeaderPrismSource:     "mobile",
		api.HeaderPrismTenantID:   "tenant-42",
	}

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), headers)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	lastReq := cr.getLastRequest()
	if lastReq == nil {
		t.Fatal("expected captured route request")
	}

	if lastReq.CallerType != "service" {
		t.Errorf("expected caller_type 'service', got '%s'", lastReq.CallerType)
	}
	if lastReq.Source != "mobile" {
		t.Errorf("expected source 'mobile', got '%s'", lastReq.Source)
	}
	if lastReq.TenantID != "tenant-42" {
		t.Errorf("expected tenant_id 'tenant-42', got '%s'", lastReq.TenantID)
	}
}

func TestAnalyzeContextInBody(t *testing.T) {
	cr := newCaptureRouter()
	ts := setupTestServerWithRouter(t, cr)
	defer ts.Close()

	req := &api.ChatCompletionRequest{
		Model: "auto",
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
		Prism: &api.PrismExtension{
			CallerType: "user",
			Source:     "web",
		},
	}

	resp := makeRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	lastReq := cr.getLastRequest()
	if lastReq == nil {
		t.Fatal("expected captured route request")
	}

	if lastReq.CallerType != "user" {
		t.Errorf("expected caller_type 'user', got '%s'", lastReq.CallerType)
	}
	if lastReq.Source != "web" {
		t.Errorf("expected source 'web', got '%s'", lastReq.Source)
	}
}

func TestAnalyzeContextHeaderOverridesBody(t *testing.T) {
	cr := newCaptureRouter()
	ts := setupTestServerWithRouter(t, cr)
	defer ts.Close()

	req := &api.ChatCompletionRequest{
		Model: "auto",
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
		Prism: &api.PrismExtension{
			CallerType: "user",
			Source:     "web",
		},
	}

	// Header should override body
	headers := map[string]string{
		api.HeaderPrismCallerType: "service",
		api.HeaderPrismSource:     "api",
	}

	resp := makeRequest(t, ts.URL, req, headers)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	lastReq := cr.getLastRequest()
	if lastReq == nil {
		t.Fatal("expected captured route request")
	}

	if lastReq.CallerType != "service" {
		t.Errorf("expected caller_type 'service' (from header), got '%s'", lastReq.CallerType)
	}
	if lastReq.Source != "api" {
		t.Errorf("expected source 'api' (from header), got '%s'", lastReq.Source)
	}
}

// ──────────────────────────────────────────────
// Metrics endpoint
// ──────────────────────────────────────────────

func TestMetricsEndpoint(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	// Make a request first to generate some metrics
	resp := makeRequest(t, ts.URL, simpleReq("Hello"), nil)
	resp.Body.Close()

	// Now check /metrics — note: setupTestServer doesn't register promhttp,
	// so we test via a custom server setup
	providers := map[string]provider.Provider{
		"openai":    provider.NewMockProvider("openai"),
		"anthropic": provider.NewMockProvider("anthropic"),
	}

	h := gateway.NewHandler(&router.StaticRouter{
		ModelID:    "gpt-4o",
		ProviderID: "openai",
		VariantID:  "gpt-4o@openai",
		AccountID:  "default-acct",
	}, providers)

	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/chat/completions", h.ChatCompletion)

	// Use promhttp directly
	mux.Handle("GET /metrics", newTestPromHandler())

	metricsSrv := httptest.NewServer(mux)
	defer metricsSrv.Close()

	metricsResp, err := http.Get(metricsSrv.URL + "/metrics")
	if err != nil {
		t.Fatalf("Failed to get /metrics: %v", err)
	}
	defer metricsResp.Body.Close()

	if metricsResp.StatusCode != http.StatusOK {
		t.Errorf("Expected 200 from /metrics, got %d", metricsResp.StatusCode)
	}

	body, _ := io.ReadAll(metricsResp.Body)
	bodyStr := string(body)

	// Should contain prometheus format
	if !strings.Contains(bodyStr, "# HELP") && !strings.Contains(bodyStr, "# TYPE") {
		// Even if no custom metrics are set, go runtime metrics should be present
		if !strings.Contains(bodyStr, "go_") {
			t.Error("Expected prometheus format in /metrics response")
		}
	}
}

// ──────────────────────────────────────────────
// Outcome reporting
// ──────────────────────────────────────────────

func TestOutcomeReport(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	report := api.OutcomeReport{
		RequestID:      "req-123",
		VariantID:      "gpt-4o@openai",
		ProviderID:     "openai",
		ModelID:        "gpt-4o",
		TotalLatencyMs: 250,
		Success:        true,
		StatusCode:     200,
		PromptTokens:   100,
		CompletionTokens: 50,
	}

	body, _ := json.Marshal(report)
	resp, err := http.Post(ts.URL+"/v1/outcomes", "application/json", bytes.NewReader(body))
	if err != nil {
		t.Fatalf("Failed to post outcome: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		respBody, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(respBody))
	}

	var outcomeResp api.OutcomeResponse
	if err := json.NewDecoder(resp.Body).Decode(&outcomeResp); err != nil {
		t.Fatalf("Failed to decode response: %v", err)
	}
	if outcomeResp.Status != "ok" {
		t.Errorf("Expected status 'ok', got '%s'", outcomeResp.Status)
	}
}

func TestOutcomeReportInvalidBody(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	resp, err := http.Post(ts.URL+"/v1/outcomes", "application/json", strings.NewReader("{invalid json"))
	if err != nil {
		t.Fatalf("Failed to post outcome: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("Expected 400 for invalid JSON, got %d", resp.StatusCode)
	}
}

func TestOutcomeReportWithTTFT(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	ttft := int64(150)
	report := api.OutcomeReport{
		RequestID:      "req-456",
		VariantID:      "gpt-4o@openai",
		ProviderID:     "openai",
		ModelID:        "gpt-4o",
		TotalLatencyMs: 500,
		TTFTMs:         &ttft,
		Success:        true,
		StatusCode:     200,
	}

	body, _ := json.Marshal(report)
	resp, err := http.Post(ts.URL+"/v1/outcomes", "application/json", bytes.NewReader(body))
	if err != nil {
		t.Fatalf("Failed to post outcome: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected 200, got %d", resp.StatusCode)
	}
}

func TestOutcomeReportError(t *testing.T) {
	ts := setupTestServer(t)
	defer ts.Close()

	report := api.OutcomeReport{
		RequestID:      "req-789",
		VariantID:      "gpt-4o@openai",
		ProviderID:     "openai",
		ModelID:        "gpt-4o",
		TotalLatencyMs: 5000,
		Success:        false,
		StatusCode:     500,
		ErrorType:      "timeout",
	}

	body, _ := json.Marshal(report)
	resp, err := http.Post(ts.URL+"/v1/outcomes", "application/json", bytes.NewReader(body))
	if err != nil {
		t.Fatalf("Failed to post outcome: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Expected 200 (fire-and-forget), got %d", resp.StatusCode)
	}
}

// newTestPromHandler returns a handler that serves Prometheus metrics.
func newTestPromHandler() http.Handler {
	// Use promhttp from the default registry
	return promhttp.Handler()
}

// ──────────────────────────────────────────────
// A/B Testing E2E tests
// ──────────────────────────────────────────────

func setupTestServerWithAB(t *testing.T) *httptest.Server {
	t.Helper()

	providers := map[string]provider.Provider{
		"openai":    provider.NewMockProvider("openai"),
		"anthropic": provider.NewMockProvider("anthropic"),
		"deepseek":  provider.NewMockProvider("deepseek"),
	}

	r := &router.StaticRouter{
		ModelID:    "gpt-4o",
		ProviderID: "openai",
		VariantID:  "gpt-4o@openai",
		AccountID:  "default-acct",
	}

	h := gateway.NewHandler(r, providers)

	experiments := []abtesting.Experiment{
		{
			ID:      "test-exp-1",
			Name:    "Test Experiment",
			Enabled: true,
			TrafficSplit: map[string]int{
				"gpt4@openai":      50,
				"claude@anthropic": 50,
			},
			Target: abtesting.ExperimentTarget{
				SampleRate: 100,
			},
			Metrics: []string{"latency", "cost"},
		},
		{
			ID:      "test-exp-2",
			Name:    "Disabled Experiment",
			Enabled: false,
			TrafficSplit: map[string]int{
				"variant-a": 100,
			},
			Target: abtesting.ExperimentTarget{
				SampleRate: 100,
			},
		},
	}
	h.SetABManager(abtesting.NewManager(experiments))

	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/chat/completions", h.ChatCompletion)
	mux.HandleFunc("POST /v1/outcomes", h.ReportOutcome)
	mux.HandleFunc("GET /v1/experiments", h.ListExperiments)
	mux.HandleFunc("GET /v1/experiments/{id}", h.GetExperiment)
	mux.HandleFunc("GET /health", h.Health)

	return httptest.NewServer(mux)
}

func TestExperimentsEndpoint(t *testing.T) {
	ts := setupTestServerWithAB(t)
	defer ts.Close()

	resp, err := http.Get(ts.URL + "/v1/experiments")
	if err != nil {
		t.Fatalf("Failed to GET experiments: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	var result struct {
		Experiments []abtesting.Experiment `json:"experiments"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		t.Fatalf("Failed to decode response: %v", err)
	}

	if len(result.Experiments) != 2 {
		t.Errorf("Expected 2 experiments, got %d", len(result.Experiments))
	}

	// Check that experiment details are correct
	found := false
	for _, exp := range result.Experiments {
		if exp.ID == "test-exp-1" {
			found = true
			if exp.Name != "Test Experiment" {
				t.Errorf("Expected name 'Test Experiment', got '%s'", exp.Name)
			}
			if !exp.Enabled {
				t.Error("Expected experiment to be enabled")
			}
		}
	}
	if !found {
		t.Error("Expected to find experiment 'test-exp-1'")
	}
}

func TestExperimentsEndpointGetByID(t *testing.T) {
	ts := setupTestServerWithAB(t)
	defer ts.Close()

	// Get existing experiment
	resp, err := http.Get(ts.URL + "/v1/experiments/test-exp-1")
	if err != nil {
		t.Fatalf("Failed to GET experiment: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	var exp abtesting.Experiment
	if err := json.NewDecoder(resp.Body).Decode(&exp); err != nil {
		t.Fatalf("Failed to decode response: %v", err)
	}
	if exp.ID != "test-exp-1" {
		t.Errorf("Expected id 'test-exp-1', got '%s'", exp.ID)
	}

	// Non-existent experiment → 404
	resp2, err := http.Get(ts.URL + "/v1/experiments/nonexistent")
	if err != nil {
		t.Fatalf("Failed to GET experiment: %v", err)
	}
	defer resp2.Body.Close()

	if resp2.StatusCode != http.StatusNotFound {
		t.Errorf("Expected 404 for nonexistent experiment, got %d", resp2.StatusCode)
	}
}

func TestExperimentHeaders(t *testing.T) {
	ts := setupTestServerWithAB(t)
	defer ts.Close()

	// A/B experiments use the trace ID for consistent assignment
	headers := map[string]string{
		api.HeaderPrismTraceID: "test-trace-id-for-ab",
	}

	resp := makeRequest(t, ts.URL, simpleReq("Hello"), headers)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("Expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	// Should have experiment headers
	expID := resp.Header.Get(api.HeaderPrismExperimentID)
	expVariant := resp.Header.Get(api.HeaderPrismExperimentVariant)

	if expID == "" {
		t.Error("Expected X-Prism-Experiment-Id header to be set")
	}
	if expVariant == "" {
		t.Error("Expected X-Prism-Experiment-Variant header to be set")
	}

	if expID != "test-exp-1" {
		t.Errorf("Expected experiment 'test-exp-1', got '%s'", expID)
	}

	// Variant should be one of the defined ones
	if expVariant != "gpt4@openai" && expVariant != "claude@anthropic" {
		t.Errorf("Unexpected experiment variant: %s", expVariant)
	}

	// Same trace ID should consistently get the same variant
	resp2 := makeRequest(t, ts.URL, simpleReq("Hello again"), headers)
	defer resp2.Body.Close()

	expVariant2 := resp2.Header.Get(api.HeaderPrismExperimentVariant)
	if expVariant2 != expVariant {
		t.Errorf("Consistent hashing failed: got %s first, then %s", expVariant, expVariant2)
	}
}

