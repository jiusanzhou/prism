package gateway

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"go.zoe.im/prism-gateway/internal/provider"
	"go.zoe.im/prism-gateway/internal/router"
	"go.zoe.im/prism-gateway/pkg/api"
)

// ──────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────

func setupAnthropicTestHandler(t *testing.T) (*Handler, *httptest.Server) {
	t.Helper()
	return setupAnthropicTestHandlerWithRouter(t, &router.StaticRouter{
		ModelID:    "claude-sonnet-4-20250514",
		ProviderID: "anthropic",
		VariantID:  "claude@anthropic",
		AccountID:  "anthropic-acct",
	})
}

func setupAnthropicTestHandlerWithRouter(t *testing.T, r router.Router) (*Handler, *httptest.Server) {
	t.Helper()
	providers := map[string]provider.Provider{
		"openai":    provider.NewMockProvider("openai"),
		"anthropic": provider.NewMockProvider("anthropic"),
		"deepseek":  provider.NewMockProvider("deepseek"),
	}
	h := NewHandler(r, providers)
	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/messages", h.AnthropicMessages)
	mux.HandleFunc("POST /v1/chat/completions", h.ChatCompletion)
	ts := httptest.NewServer(mux)
	return h, ts
}

func makeAnthropicRequest(t *testing.T, url string, body interface{}, headers map[string]string) *http.Response {
	t.Helper()
	data, err := json.Marshal(body)
	if err != nil {
		t.Fatalf("failed to marshal request: %v", err)
	}
	return makeAnthropicRequestRaw(t, url, data, headers)
}

func makeAnthropicRequestRaw(t *testing.T, url string, body []byte, headers map[string]string) *http.Response {
	t.Helper()
	httpReq, err := http.NewRequest("POST", url+"/v1/messages", bytes.NewReader(body))
	if err != nil {
		t.Fatalf("failed to create request: %v", err)
	}
	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("x-api-key", "sk-test-key")
	httpReq.Header.Set("anthropic-version", "2023-06-01")
	for k, v := range headers {
		httpReq.Header.Set(k, v)
	}
	resp, err := http.DefaultClient.Do(httpReq)
	if err != nil {
		t.Fatalf("request failed: %v", err)
	}
	return resp
}

func simpleAnthropicReq() map[string]interface{} {
	return map[string]interface{}{
		"model":      "claude-sonnet-4-20250514",
		"max_tokens": 1024,
		"messages": []map[string]interface{}{
			{"role": "user", "content": "Hello, Claude!"},
		},
	}
}

// ──────────────────────────────────────────────
// Non-streaming tests
// ──────────────────────────────────────────────

func TestAnthropicMessages_BasicNonStream(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	resp := makeAnthropicRequest(t, ts.URL, simpleAnthropicReq(), nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	var anthropicResp api.AnthropicMessagesResponse
	if err := json.NewDecoder(resp.Body).Decode(&anthropicResp); err != nil {
		t.Fatalf("failed to decode response: %v", err)
	}

	if anthropicResp.Type != "message" {
		t.Errorf("expected type 'message', got '%s'", anthropicResp.Type)
	}
	if anthropicResp.Role != "assistant" {
		t.Errorf("expected role 'assistant', got '%s'", anthropicResp.Role)
	}
	if len(anthropicResp.Content) == 0 {
		t.Fatal("expected at least one content block")
	}
	if anthropicResp.Content[0].Type != "text" {
		t.Errorf("expected content type 'text', got '%s'", anthropicResp.Content[0].Type)
	}
	if anthropicResp.Content[0].Text == "" {
		t.Error("expected non-empty content text")
	}
	if !strings.HasPrefix(anthropicResp.ID, "msg_") {
		t.Errorf("expected ID to start with 'msg_', got '%s'", anthropicResp.ID)
	}
}

func TestAnthropicMessages_ResponseContentType(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	resp := makeAnthropicRequest(t, ts.URL, simpleAnthropicReq(), nil)
	defer resp.Body.Close()

	ct := resp.Header.Get("Content-Type")
	if !strings.Contains(ct, "application/json") {
		t.Errorf("expected application/json, got %s", ct)
	}
}

func TestAnthropicMessages_RoutingHeaders(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	resp := makeAnthropicRequest(t, ts.URL, simpleAnthropicReq(), nil)
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
			t.Errorf("missing response header: %s", h)
		}
	}

	if got := resp.Header.Get(api.HeaderPrismProvider); got != "anthropic" {
		t.Errorf("expected provider 'anthropic', got '%s'", got)
	}
}

func TestAnthropicMessages_PrismHeaderPassthrough(t *testing.T) {
	cr := &anthropicCaptureRouter{
		result: router.RouteResult{
			Model:    "claude-sonnet-4-20250514",
			Provider: "anthropic",
			Variant:  "claude@anthropic",
			Account:  "acct",
			Reason:   "captured",
			Score:    1.0,
		},
	}
	_, ts := setupAnthropicTestHandlerWithRouter(t, cr)
	defer ts.Close()

	headers := map[string]string{
		api.HeaderPrismStrategy:   "cost_first",
		api.HeaderPrismCallerType: "service",
		api.HeaderPrismSource:     "mobile",
	}

	resp := makeAnthropicRequest(t, ts.URL, simpleAnthropicReq(), headers)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	lastReq := cr.getLastRequest()
	if lastReq == nil {
		t.Fatal("expected captured route request")
	}
	if lastReq.Strategy != "cost_first" {
		t.Errorf("expected strategy 'cost_first', got '%s'", lastReq.Strategy)
	}
	if lastReq.CallerType != "service" {
		t.Errorf("expected caller_type 'service', got '%s'", lastReq.CallerType)
	}
	if lastReq.Source != "mobile" {
		t.Errorf("expected source 'mobile', got '%s'", lastReq.Source)
	}
}

// ──────────────────────────────────────────────
// System field handling
// ──────────────────────────────────────────────

func TestAnthropicMessages_SystemString(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	req := map[string]interface{}{
		"model":      "claude-sonnet-4-20250514",
		"max_tokens": 1024,
		"system":     "You are a helpful assistant.",
		"messages": []map[string]interface{}{
			{"role": "user", "content": "Hello"},
		},
	}

	resp := makeAnthropicRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, string(body))
	}
}

func TestAnthropicMessages_SystemContentBlocks(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	req := map[string]interface{}{
		"model":      "claude-sonnet-4-20250514",
		"max_tokens": 1024,
		"system": []map[string]interface{}{
			{"type": "text", "text": "You are a helpful assistant."},
		},
		"messages": []map[string]interface{}{
			{"role": "user", "content": "Hello"},
		},
	}

	resp := makeAnthropicRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, string(body))
	}
}

// ──────────────────────────────────────────────
// Content field handling
// ──────────────────────────────────────────────

func TestAnthropicMessages_ContentString(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	req := map[string]interface{}{
		"model":      "claude-sonnet-4-20250514",
		"max_tokens": 1024,
		"messages": []map[string]interface{}{
			{"role": "user", "content": "Hello, Claude!"},
		},
	}

	resp := makeAnthropicRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, string(body))
	}
}

func TestAnthropicMessages_ContentBlocks(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	req := map[string]interface{}{
		"model":      "claude-sonnet-4-20250514",
		"max_tokens": 1024,
		"messages": []map[string]interface{}{
			{
				"role": "user",
				"content": []map[string]interface{}{
					{"type": "text", "text": "Hello, Claude!"},
				},
			},
		},
	}

	resp := makeAnthropicRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, string(body))
	}
}

// ──────────────────────────────────────────────
// Streaming tests
// ──────────────────────────────────────────────

func TestAnthropicMessages_Streaming(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	req := map[string]interface{}{
		"model":      "claude-sonnet-4-20250514",
		"max_tokens": 1024,
		"stream":     true,
		"messages": []map[string]interface{}{
			{"role": "user", "content": "Tell me a joke"},
		},
	}

	resp := makeAnthropicRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	ct := resp.Header.Get("Content-Type")
	if !strings.Contains(ct, "text/event-stream") {
		t.Errorf("expected text/event-stream, got %s", ct)
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatalf("failed to read stream: %v", err)
	}
	bodyStr := string(body)

	// Verify SSE format: event: + data: pairs
	if !strings.Contains(bodyStr, "event: message_start") {
		t.Error("stream should contain 'event: message_start'")
	}
	if !strings.Contains(bodyStr, "event: content_block_start") {
		t.Error("stream should contain 'event: content_block_start'")
	}
	if !strings.Contains(bodyStr, "event: content_block_delta") {
		t.Error("stream should contain 'event: content_block_delta'")
	}
	if !strings.Contains(bodyStr, "event: message_stop") {
		t.Error("stream should contain 'event: message_stop'")
	}

	// Verify each data line is valid JSON
	for _, line := range strings.Split(bodyStr, "\n") {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "data: ") {
			jsonStr := strings.TrimPrefix(line, "data: ")
			var parsed map[string]interface{}
			if err := json.Unmarshal([]byte(jsonStr), &parsed); err != nil {
				t.Errorf("invalid SSE data JSON: %s, err: %v", jsonStr, err)
			}
			// Each data payload should have a "type" field
			if _, ok := parsed["type"]; !ok {
				t.Errorf("SSE data missing 'type' field: %s", jsonStr)
			}
		}
	}
}

func TestAnthropicMessages_StreamingHasRoutingHeaders(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	req := map[string]interface{}{
		"model":      "claude-sonnet-4-20250514",
		"max_tokens": 1024,
		"stream":     true,
		"messages": []map[string]interface{}{
			{"role": "user", "content": "Hello"},
		},
	}

	resp := makeAnthropicRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if model := resp.Header.Get(api.HeaderPrismModel); model == "" {
		t.Error("streaming response missing X-Prism-Model header")
	}
	if prov := resp.Header.Get(api.HeaderPrismProvider); prov == "" {
		t.Error("streaming response missing X-Prism-Provider header")
	}
}

// ──────────────────────────────────────────────
// Error handling
// ──────────────────────────────────────────────

func TestAnthropicMessages_InvalidJSON(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	resp := makeAnthropicRequestRaw(t, ts.URL, []byte("{invalid"), nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("expected 400, got %d", resp.StatusCode)
	}

	var errResp api.AnthropicErrorResponse
	if err := json.NewDecoder(resp.Body).Decode(&errResp); err != nil {
		t.Fatalf("failed to decode error response: %v", err)
	}

	if errResp.Type != "error" {
		t.Errorf("expected type 'error', got '%s'", errResp.Type)
	}
	if errResp.Error.Type != "invalid_request_error" {
		t.Errorf("expected error type 'invalid_request_error', got '%s'", errResp.Error.Type)
	}
}

func TestAnthropicMessages_MissingModel(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	req := map[string]interface{}{
		"max_tokens": 1024,
		"messages": []map[string]interface{}{
			{"role": "user", "content": "Hello"},
		},
	}

	resp := makeAnthropicRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("expected 400, got %d", resp.StatusCode)
	}

	var errResp api.AnthropicErrorResponse
	if err := json.NewDecoder(resp.Body).Decode(&errResp); err != nil {
		t.Fatalf("failed to decode error response: %v", err)
	}
	if errResp.Type != "error" {
		t.Errorf("expected type 'error', got '%s'", errResp.Type)
	}
}

func TestAnthropicMessages_MissingMaxTokens(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	req := map[string]interface{}{
		"model": "claude-sonnet-4-20250514",
		"messages": []map[string]interface{}{
			{"role": "user", "content": "Hello"},
		},
	}

	resp := makeAnthropicRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("expected 400, got %d", resp.StatusCode)
	}

	var errResp api.AnthropicErrorResponse
	json.NewDecoder(resp.Body).Decode(&errResp)
	if errResp.Type != "error" {
		t.Errorf("expected type 'error', got '%s'", errResp.Type)
	}
}

func TestAnthropicMessages_EmptyBody(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	resp := makeAnthropicRequestRaw(t, ts.URL, []byte(""), nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("expected 400, got %d", resp.StatusCode)
	}

	var errResp api.AnthropicErrorResponse
	json.NewDecoder(resp.Body).Decode(&errResp)
	if errResp.Type != "error" {
		t.Errorf("expected Anthropic error format, got type '%s'", errResp.Type)
	}
}

func TestAnthropicMessages_ProviderNotFound(t *testing.T) {
	r := &router.StaticRouter{
		ModelID:    "model",
		ProviderID: "nonexistent",
		VariantID:  "variant",
		AccountID:  "acct",
	}
	_, ts := setupAnthropicTestHandlerWithRouter(t, r)
	defer ts.Close()

	resp := makeAnthropicRequest(t, ts.URL, simpleAnthropicReq(), nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadGateway {
		t.Errorf("expected 502, got %d", resp.StatusCode)
	}

	var errResp api.AnthropicErrorResponse
	json.NewDecoder(resp.Body).Decode(&errResp)
	if errResp.Type != "error" {
		t.Errorf("expected Anthropic error format, got type '%s'", errResp.Type)
	}
	if errResp.Error.Type != "api_error" {
		t.Errorf("expected error type 'api_error', got '%s'", errResp.Error.Type)
	}
}

func TestAnthropicMessages_ProviderError(t *testing.T) {
	mockProv := provider.NewMockProvider("anthropic").
		WithError("claude-sonnet-4-20250514", io.ErrUnexpectedEOF)

	r := &router.StaticRouter{
		ModelID:    "claude-sonnet-4-20250514",
		ProviderID: "anthropic",
		VariantID:  "claude@anthropic",
		AccountID:  "acct",
	}

	h := NewHandler(r, map[string]provider.Provider{"anthropic": mockProv})
	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/messages", h.AnthropicMessages)
	ts := httptest.NewServer(mux)
	defer ts.Close()

	resp := makeAnthropicRequest(t, ts.URL, simpleAnthropicReq(), nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadGateway {
		t.Errorf("expected 502, got %d", resp.StatusCode)
	}

	var errResp api.AnthropicErrorResponse
	json.NewDecoder(resp.Body).Decode(&errResp)
	if errResp.Type != "error" {
		t.Errorf("expected Anthropic error format")
	}
}

func TestAnthropicMessages_StreamProviderError(t *testing.T) {
	mockProv := provider.NewMockProvider("anthropic").
		WithError("claude-sonnet-4-20250514", io.ErrUnexpectedEOF)

	r := &router.StaticRouter{
		ModelID:    "claude-sonnet-4-20250514",
		ProviderID: "anthropic",
		VariantID:  "claude@anthropic",
		AccountID:  "acct",
	}

	h := NewHandler(r, map[string]provider.Provider{"anthropic": mockProv})
	mux := http.NewServeMux()
	mux.HandleFunc("POST /v1/messages", h.AnthropicMessages)
	ts := httptest.NewServer(mux)
	defer ts.Close()

	req := map[string]interface{}{
		"model":      "claude-sonnet-4-20250514",
		"max_tokens": 1024,
		"stream":     true,
		"messages": []map[string]interface{}{
			{"role": "user", "content": "Hello"},
		},
	}

	resp := makeAnthropicRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadGateway {
		t.Errorf("expected 502, got %d", resp.StatusCode)
	}

	var errResp api.AnthropicErrorResponse
	json.NewDecoder(resp.Body).Decode(&errResp)
	if errResp.Type != "error" {
		t.Errorf("expected Anthropic error format")
	}
}

func TestAnthropicMessages_RouterError(t *testing.T) {
	r := &anthropicErrorRouter{err: io.ErrUnexpectedEOF}
	_, ts := setupAnthropicTestHandlerWithRouter(t, r)
	defer ts.Close()

	resp := makeAnthropicRequest(t, ts.URL, simpleAnthropicReq(), nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusServiceUnavailable {
		t.Errorf("expected 503, got %d", resp.StatusCode)
	}

	var errResp api.AnthropicErrorResponse
	json.NewDecoder(resp.Body).Decode(&errResp)
	if errResp.Type != "error" {
		t.Errorf("expected Anthropic error format")
	}
	if errResp.Error.Type != "api_error" {
		t.Errorf("expected error type 'api_error', got '%s'", errResp.Error.Type)
	}
}

// ──────────────────────────────────────────────
// Model routing (pinned model)
// ──────────────────────────────────────────────

func TestAnthropicMessages_PinnedModel(t *testing.T) {
	cr := &anthropicCaptureRouter{
		result: router.RouteResult{
			Model:    "claude-sonnet-4-20250514",
			Provider: "anthropic",
			Variant:  "claude@anthropic",
			Account:  "acct",
			Reason:   "pinned",
			Score:    1.0,
		},
	}
	_, ts := setupAnthropicTestHandlerWithRouter(t, cr)
	defer ts.Close()

	resp := makeAnthropicRequest(t, ts.URL, simpleAnthropicReq(), nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	lastReq := cr.getLastRequest()
	if lastReq == nil {
		t.Fatal("expected captured route request")
	}
	// Model should trigger pinned strategy
	if lastReq.Strategy != "pinned:claude-sonnet-4-20250514" {
		t.Errorf("expected pinned strategy, got '%s'", lastReq.Strategy)
	}
}

// ──────────────────────────────────────────────
// Concurrent requests
// ──────────────────────────────────────────────

func TestAnthropicMessages_Concurrent(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	const concurrency = 50
	var wg sync.WaitGroup
	errCount := 0
	var mu sync.Mutex

	for i := 0; i < concurrency; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			resp := makeAnthropicRequest(t, ts.URL, simpleAnthropicReq(), nil)
			defer resp.Body.Close()
			io.ReadAll(resp.Body)

			if resp.StatusCode != http.StatusOK {
				mu.Lock()
				errCount++
				mu.Unlock()
			}
		}(i)
	}

	wg.Wait()
	if errCount > 0 {
		t.Errorf("%d/%d concurrent requests failed", errCount, concurrency)
	}
}

func TestAnthropicMessages_ConcurrentStreaming(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	const concurrency = 20
	var wg sync.WaitGroup

	for i := 0; i < concurrency; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			req := map[string]interface{}{
				"model":      "claude-sonnet-4-20250514",
				"max_tokens": 1024,
				"stream":     true,
				"messages": []map[string]interface{}{
					{"role": "user", "content": "Concurrent stream test"},
				},
			}
			resp := makeAnthropicRequest(t, ts.URL, req, nil)
			defer resp.Body.Close()

			if resp.StatusCode != http.StatusOK {
				t.Errorf("streaming request %d failed with %d", i, resp.StatusCode)
				return
			}
			body, _ := io.ReadAll(resp.Body)
			if !strings.Contains(string(body), "message_start") {
				t.Errorf("streaming request %d missing message_start", i)
			}
		}(i)
	}

	wg.Wait()
}

// ──────────────────────────────────────────────
// Verify Anthropic endpoint doesn't break OpenAI endpoint
// ──────────────────────────────────────────────

func TestAnthropicMessages_OpenAIStillWorks(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	// Make an OpenAI-format request
	oaiReq := &api.ChatCompletionRequest{
		Model: "auto",
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
	}
	body, _ := json.Marshal(oaiReq)
	httpReq, _ := http.NewRequest("POST", ts.URL+"/v1/chat/completions", bytes.NewReader(body))
	httpReq.Header.Set("Content-Type", "application/json")

	resp, err := http.DefaultClient.Do(httpReq)
	if err != nil {
		t.Fatalf("request failed: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		respBody, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, string(respBody))
	}

	// Verify it's OpenAI format, not Anthropic
	var oaiResp api.ChatCompletionResponse
	if err := json.NewDecoder(resp.Body).Decode(&oaiResp); err != nil {
		t.Fatalf("failed to decode OpenAI response: %v", err)
	}
	if oaiResp.Object != "chat.completion" {
		t.Errorf("expected object 'chat.completion', got '%s'", oaiResp.Object)
	}
}

// ──────────────────────────────────────────────
// Prism extension in body
// ──────────────────────────────────────────────

func TestAnthropicMessages_PrismExtensionInBody(t *testing.T) {
	cr := &anthropicCaptureRouter{
		result: router.RouteResult{
			Model:    "claude-sonnet-4-20250514",
			Provider: "anthropic",
			Variant:  "claude@anthropic",
			Account:  "acct",
			Reason:   "captured",
			Score:    1.0,
		},
	}
	_, ts := setupAnthropicTestHandlerWithRouter(t, cr)
	defer ts.Close()

	req := map[string]interface{}{
		"model":      "claude-sonnet-4-20250514",
		"max_tokens": 1024,
		"messages": []map[string]interface{}{
			{"role": "user", "content": "Hello"},
		},
		"prism": map[string]interface{}{
			"strategy":    "balanced",
			"caller_type": "api-client",
			"source":      "test",
		},
	}

	resp := makeAnthropicRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, string(body))
	}

	lastReq := cr.getLastRequest()
	if lastReq == nil {
		t.Fatal("expected captured route request")
	}
	if lastReq.Strategy != "balanced" {
		t.Errorf("expected strategy 'balanced', got '%s'", lastReq.Strategy)
	}
	if lastReq.CallerType != "api-client" {
		t.Errorf("expected caller_type 'api-client', got '%s'", lastReq.CallerType)
	}
}

// ──────────────────────────────────────────────
// SSE format correctness
// ──────────────────────────────────────────────

func TestAnthropicMessages_SSEFormatCorrectness(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	req := map[string]interface{}{
		"model":      "claude-sonnet-4-20250514",
		"max_tokens": 1024,
		"stream":     true,
		"messages": []map[string]interface{}{
			{"role": "user", "content": "Hello"},
		},
	}

	resp := makeAnthropicRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	body, _ := io.ReadAll(resp.Body)
	lines := strings.Split(string(body), "\n")

	// Verify event: + data: pair structure
	for i := 0; i < len(lines); i++ {
		line := strings.TrimSpace(lines[i])
		if strings.HasPrefix(line, "event: ") {
			// Next non-empty line should be data:
			for j := i + 1; j < len(lines); j++ {
				nextLine := strings.TrimSpace(lines[j])
				if nextLine == "" {
					continue
				}
				if !strings.HasPrefix(nextLine, "data: ") {
					t.Errorf("expected 'data:' after 'event:' at line %d, got '%s'", i, nextLine)
				}
				break
			}
		}
	}
}

// ──────────────────────────────────────────────
// Multi-turn conversation
// ──────────────────────────────────────────────

func TestAnthropicMessages_MultiTurn(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	req := map[string]interface{}{
		"model":      "claude-sonnet-4-20250514",
		"max_tokens": 1024,
		"messages": []map[string]interface{}{
			{"role": "user", "content": "What is Go?"},
			{"role": "assistant", "content": "Go is a programming language."},
			{"role": "user", "content": "Tell me more."},
		},
	}

	resp := makeAnthropicRequest(t, ts.URL, req, nil)
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		t.Fatalf("expected 200, got %d: %s", resp.StatusCode, string(body))
	}
}

// ──────────────────────────────────────────────
// Stop reason/sequence
// ──────────────────────────────────────────────

func TestAnthropicMessages_StopReasonInResponse(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	resp := makeAnthropicRequest(t, ts.URL, simpleAnthropicReq(), nil)
	defer resp.Body.Close()

	var anthropicResp api.AnthropicMessagesResponse
	json.NewDecoder(resp.Body).Decode(&anthropicResp)

	if anthropicResp.StopReason == nil {
		t.Error("expected stop_reason to be set")
	}
	// Mock provider returns "stop" → should be mapped to "end_turn"
	if *anthropicResp.StopReason != "end_turn" {
		t.Errorf("expected stop_reason 'end_turn', got '%s'", *anthropicResp.StopReason)
	}
}

// ──────────────────────────────────────────────
// Usage in response
// ──────────────────────────────────────────────

func TestAnthropicMessages_UsageInResponse(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	resp := makeAnthropicRequest(t, ts.URL, simpleAnthropicReq(), nil)
	defer resp.Body.Close()

	var anthropicResp api.AnthropicMessagesResponse
	json.NewDecoder(resp.Body).Decode(&anthropicResp)

	// Mock provider returns usage with 10 prompt + 20 completion
	if anthropicResp.Usage.InputTokens != 10 {
		t.Errorf("expected 10 input_tokens, got %d", anthropicResp.Usage.InputTokens)
	}
	if anthropicResp.Usage.OutputTokens != 20 {
		t.Errorf("expected 20 output_tokens, got %d", anthropicResp.Usage.OutputTokens)
	}
}

// ──────────────────────────────────────────────
// Error response JSON format
// ──────────────────────────────────────────────

func TestAnthropicMessages_ErrorResponseFormat(t *testing.T) {
	_, ts := setupAnthropicTestHandler(t)
	defer ts.Close()

	// Send invalid JSON to trigger error
	resp := makeAnthropicRequestRaw(t, ts.URL, []byte("{bad json"), nil)
	defer resp.Body.Close()

	body, _ := io.ReadAll(resp.Body)

	// Verify the error matches Anthropic format exactly
	var errResp api.AnthropicErrorResponse
	if err := json.Unmarshal(body, &errResp); err != nil {
		t.Fatalf("error response is not valid JSON: %v", err)
	}

	if errResp.Type != "error" {
		t.Errorf("top-level type should be 'error', got '%s'", errResp.Type)
	}
	if errResp.Error.Type == "" {
		t.Error("error.type should not be empty")
	}
	if errResp.Error.Message == "" {
		t.Error("error.message should not be empty")
	}

	// Verify it has exactly the expected structure (no extra fields)
	var rawMap map[string]interface{}
	json.Unmarshal(body, &rawMap)
	if len(rawMap) != 2 {
		t.Errorf("expected 2 top-level fields (type, error), got %d", len(rawMap))
	}
}

// ──────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────

type anthropicCaptureRouter struct {
	mu      sync.Mutex
	lastReq *router.RouteRequest
	result  router.RouteResult
}

func (r *anthropicCaptureRouter) Route(req *router.RouteRequest) (*router.RouteResult, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.lastReq = req
	result := r.result
	return &result, nil
}

func (r *anthropicCaptureRouter) getLastRequest() *router.RouteRequest {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.lastReq
}

type anthropicErrorRouter struct {
	err error
}

func (r *anthropicErrorRouter) Route(req *router.RouteRequest) (*router.RouteResult, error) {
	return nil, r.err
}
