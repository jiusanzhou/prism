package provider

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/aspect-build/prism-gateway/pkg/api"
)

func newAnthropicTestServer(t *testing.T, handler http.HandlerFunc) (*httptest.Server, *AnthropicProvider) {
	t.Helper()
	server := httptest.NewServer(handler)
	provider := NewAnthropicProvider(ProviderConfig{
		ID:      "test-anthropic",
		Name:    "test-anthropic",
		BaseURL: server.URL,
		APIKey:  "test-anthropic-key-123",
	})
	return server, provider
}

func TestAnthropicChatCompletion(t *testing.T) {
	anthropicResp := `{
		"id": "msg_abc123",
		"type": "message",
		"role": "assistant",
		"model": "claude-sonnet-4-20250514",
		"content": [
			{"type": "text", "text": "Hello! How can I help you today?"}
		],
		"stop_reason": "end_turn",
		"usage": {
			"input_tokens": 12,
			"output_tokens": 8
		}
	}`

	server, provider := newAnthropicTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		// Verify request path
		if r.URL.Path != "/v1/messages" {
			t.Errorf("expected /v1/messages, got %s", r.URL.Path)
		}
		if r.Method != http.MethodPost {
			t.Errorf("expected POST, got %s", r.Method)
		}

		// Verify Anthropic-specific headers
		if apiKey := r.Header.Get("x-api-key"); apiKey != "test-anthropic-key-123" {
			t.Errorf("expected x-api-key test-anthropic-key-123, got %s", apiKey)
		}
		if version := r.Header.Get("anthropic-version"); version != "2023-06-01" {
			t.Errorf("expected anthropic-version 2023-06-01, got %s", version)
		}
		if ct := r.Header.Get("Content-Type"); ct != "application/json" {
			t.Errorf("expected application/json, got %s", ct)
		}

		// Should NOT have Authorization: Bearer header
		if auth := r.Header.Get("Authorization"); auth != "" {
			t.Errorf("expected no Authorization header for Anthropic, got %s", auth)
		}

		// Verify request body format
		var ar anthropicRequest
		if err := json.NewDecoder(r.Body).Decode(&ar); err != nil {
			t.Errorf("failed to decode request body: %v", err)
		}
		if ar.Stream {
			t.Error("expected stream=false for non-streaming request")
		}
		if ar.Model == "" {
			t.Error("expected non-empty model")
		}
		if len(ar.Messages) != 1 || ar.Messages[0].Content != "Hello" {
			t.Errorf("unexpected messages: %+v", ar.Messages)
		}
		if ar.MaxTokens <= 0 {
			t.Error("expected positive max_tokens")
		}

		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(anthropicResp))
	})
	defer server.Close()

	req := &api.ChatCompletionRequest{
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
	}

	resp, err := provider.ChatCompletion(context.Background(), "claude-sonnet-4-20250514", req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Verify OpenAI-format response
	if resp.ID != "msg_abc123" {
		t.Errorf("expected ID msg_abc123, got %s", resp.ID)
	}
	if resp.Object != "chat.completion" {
		t.Errorf("expected object chat.completion, got %s", resp.Object)
	}
	if len(resp.Choices) != 1 {
		t.Fatalf("expected 1 choice, got %d", len(resp.Choices))
	}
	if resp.Choices[0].Message == nil {
		t.Fatal("expected non-nil message")
	}
	if resp.Choices[0].Message.Role != "assistant" {
		t.Errorf("expected role assistant, got %s", resp.Choices[0].Message.Role)
	}
	if resp.Choices[0].Message.Content != "Hello! How can I help you today?" {
		t.Errorf("unexpected content: %s", resp.Choices[0].Message.Content)
	}
	if resp.Choices[0].FinishReason == nil || *resp.Choices[0].FinishReason != "stop" {
		t.Error("expected finish_reason 'stop'")
	}
	if resp.Usage == nil {
		t.Fatal("expected non-nil usage")
	}
	if resp.Usage.PromptTokens != 12 {
		t.Errorf("expected 12 prompt tokens, got %d", resp.Usage.PromptTokens)
	}
	if resp.Usage.CompletionTokens != 8 {
		t.Errorf("expected 8 completion tokens, got %d", resp.Usage.CompletionTokens)
	}
	if resp.Usage.TotalTokens != 20 {
		t.Errorf("expected 20 total tokens, got %d", resp.Usage.TotalTokens)
	}
}

func TestAnthropicSystemMessage(t *testing.T) {
	server, provider := newAnthropicTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		var ar anthropicRequest
		if err := json.NewDecoder(r.Body).Decode(&ar); err != nil {
			t.Errorf("failed to decode: %v", err)
		}

		// System message should be extracted
		if ar.System != "You are helpful." {
			t.Errorf("expected system 'You are helpful.', got '%s'", ar.System)
		}

		// Messages should not contain system role
		for _, msg := range ar.Messages {
			if msg.Role == "system" {
				t.Error("system message should not be in messages array")
			}
		}

		resp := `{"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":1}}`
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(resp))
	})
	defer server.Close()

	req := &api.ChatCompletionRequest{
		Messages: []api.Message{
			{Role: "system", Content: "You are helpful."},
			{Role: "user", Content: "Hello"},
		},
	}

	_, err := provider.ChatCompletion(context.Background(), "claude-sonnet-4-20250514", req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestAnthropicStreaming(t *testing.T) {
	sseData := `event: message_start
data: {"type":"message_start","message":{"id":"msg_stream1","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","content":[],"stop_reason":null,"usage":{"input_tokens":10,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}

event: message_stop
data: {"type":"message_stop"}

`

	server, provider := newAnthropicTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		var ar anthropicRequest
		json.NewDecoder(r.Body).Decode(&ar)
		if !ar.Stream {
			t.Error("expected stream=true for streaming request")
		}

		w.Header().Set("Content-Type", "text/event-stream")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(sseData))
	})
	defer server.Close()

	req := &api.ChatCompletionRequest{
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
	}

	stream, err := provider.ChatCompletionStream(context.Background(), "claude-sonnet-4-20250514", req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer stream.Close()

	// First chunk: message_start → role="assistant"
	chunk, err := stream.Recv()
	if err != nil {
		t.Fatalf("unexpected error on first recv: %v", err)
	}
	if chunk.ID != "msg_stream1" {
		t.Errorf("expected ID msg_stream1, got %s", chunk.ID)
	}
	if chunk.Object != "chat.completion.chunk" {
		t.Errorf("expected chat.completion.chunk, got %s", chunk.Object)
	}
	if chunk.Choices[0].Delta.Role != "assistant" {
		t.Errorf("expected role 'assistant', got '%s'", chunk.Choices[0].Delta.Role)
	}

	// Second chunk: content_block_delta → "Hello"
	chunk, err = stream.Recv()
	if err != nil {
		t.Fatalf("unexpected error on second recv: %v", err)
	}
	if chunk.Choices[0].Delta.Content != "Hello" {
		t.Errorf("expected 'Hello', got '%s'", chunk.Choices[0].Delta.Content)
	}

	// Third chunk: content_block_delta → " world"
	chunk, err = stream.Recv()
	if err != nil {
		t.Fatalf("unexpected error on third recv: %v", err)
	}
	if chunk.Choices[0].Delta.Content != " world" {
		t.Errorf("expected ' world', got '%s'", chunk.Choices[0].Delta.Content)
	}

	// Fourth chunk: message_delta → finish_reason="stop"
	chunk, err = stream.Recv()
	if err != nil {
		t.Fatalf("unexpected error on fourth recv: %v", err)
	}
	if chunk.Choices[0].FinishReason == nil || *chunk.Choices[0].FinishReason != "stop" {
		t.Error("expected finish_reason='stop'")
	}

	// Fifth recv: EOF (message_stop)
	_, err = stream.Recv()
	if err != io.EOF {
		t.Errorf("expected io.EOF, got %v", err)
	}
}

func TestAnthropicErrorHandling(t *testing.T) {
	tests := []struct {
		name       string
		statusCode int
		body       string
		wantMsg    string
		wantType   string
	}{
		{
			name:       "401 Authentication Error",
			statusCode: 401,
			body:       `{"type":"error","error":{"type":"authentication_error","message":"Invalid API key"}}`,
			wantMsg:    "Invalid API key",
			wantType:   "authentication_error",
		},
		{
			name:       "429 Rate Limit",
			statusCode: 429,
			body:       `{"type":"error","error":{"type":"rate_limit_error","message":"Rate limit exceeded"}}`,
			wantMsg:    "Rate limit exceeded",
			wantType:   "rate_limit_error",
		},
		{
			name:       "500 Server Error",
			statusCode: 500,
			body:       `{"type":"error","error":{"type":"api_error","message":"Internal server error"}}`,
			wantMsg:    "Internal server error",
			wantType:   "api_error",
		},
		{
			name:       "400 Bad Request",
			statusCode: 400,
			body:       `{"type":"error","error":{"type":"invalid_request_error","message":"Messages must not be empty"}}`,
			wantMsg:    "Messages must not be empty",
			wantType:   "invalid_request_error",
		},
		{
			name:       "Plain text error",
			statusCode: 502,
			body:       "Bad Gateway",
			wantMsg:    "Bad Gateway",
			wantType:   "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server, provider := newAnthropicTestServer(t, func(w http.ResponseWriter, r *http.Request) {
				w.WriteHeader(tt.statusCode)
				w.Write([]byte(tt.body))
			})
			defer server.Close()

			req := &api.ChatCompletionRequest{
				Messages: []api.Message{{Role: "user", Content: "test"}},
			}

			_, err := provider.ChatCompletion(context.Background(), "claude-sonnet-4-20250514", req)
			if err == nil {
				t.Fatal("expected error, got nil")
			}

			apiErr, ok := err.(*APIError)
			if !ok {
				t.Fatalf("expected *APIError, got %T: %v", err, err)
			}

			if apiErr.StatusCode != tt.statusCode {
				t.Errorf("expected status %d, got %d", tt.statusCode, apiErr.StatusCode)
			}

			if !strings.Contains(apiErr.Message, tt.wantMsg) {
				t.Errorf("expected message to contain '%s', got '%s'", tt.wantMsg, apiErr.Message)
			}

			if tt.wantType != "" && apiErr.Type != tt.wantType {
				t.Errorf("expected type '%s', got '%s'", tt.wantType, apiErr.Type)
			}
		})
	}
}

func TestAnthropicAuthHeaders(t *testing.T) {
	server, provider := newAnthropicTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		// Verify x-api-key header (NOT Authorization: Bearer)
		apiKey := r.Header.Get("x-api-key")
		if apiKey != "test-anthropic-key-123" {
			t.Errorf("expected x-api-key 'test-anthropic-key-123', got '%s'", apiKey)
		}

		// Verify anthropic-version header
		version := r.Header.Get("anthropic-version")
		if version != "2023-06-01" {
			t.Errorf("expected anthropic-version '2023-06-01', got '%s'", version)
		}

		// Verify NO Authorization header
		auth := r.Header.Get("Authorization")
		if auth != "" {
			t.Errorf("expected no Authorization header, got '%s'", auth)
		}

		resp := `{"id":"msg_auth","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":1}}`
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(resp))
	})
	defer server.Close()

	req := &api.ChatCompletionRequest{
		Messages: []api.Message{{Role: "user", Content: "test"}},
	}

	_, err := provider.ChatCompletion(context.Background(), "claude-sonnet-4-20250514", req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestAnthropicRateLimitError(t *testing.T) {
	server, provider := newAnthropicTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusTooManyRequests)
		w.Write([]byte(`{"type":"error","error":{"type":"rate_limit_error","message":"Rate limit exceeded"}}`))
	})
	defer server.Close()

	req := &api.ChatCompletionRequest{
		Messages: []api.Message{{Role: "user", Content: "test"}},
	}

	_, err := provider.ChatCompletion(context.Background(), "claude-sonnet-4-20250514", req)
	if err == nil {
		t.Fatal("expected error, got nil")
	}

	apiErr, ok := err.(*APIError)
	if !ok {
		t.Fatalf("expected *APIError, got %T: %v", err, err)
	}

	if !apiErr.IsRateLimit() {
		t.Errorf("expected IsRateLimit()=true, got false (status=%d)", apiErr.StatusCode)
	}

	// Also test streaming returns rate limit error
	_, err = provider.ChatCompletionStream(context.Background(), "claude-sonnet-4-20250514", req)
	if err == nil {
		t.Fatal("expected error, got nil")
	}

	apiErr2, ok := err.(*APIError)
	if !ok {
		t.Fatalf("expected *APIError, got %T", err)
	}
	if !apiErr2.IsRateLimit() {
		t.Error("expected IsRateLimit()=true for streaming")
	}
}

func TestAnthropicStreamingError(t *testing.T) {
	server, provider := newAnthropicTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
		w.Write([]byte(`{"type":"error","error":{"type":"api_error","message":"overloaded"}}`))
	})
	defer server.Close()

	req := &api.ChatCompletionRequest{
		Messages: []api.Message{{Role: "user", Content: "test"}},
	}

	_, err := provider.ChatCompletionStream(context.Background(), "claude-sonnet-4-20250514", req)
	if err == nil {
		t.Fatal("expected error, got nil")
	}

	apiErr, ok := err.(*APIError)
	if !ok {
		t.Fatalf("expected *APIError, got %T: %v", err, err)
	}

	if apiErr.StatusCode != 500 {
		t.Errorf("expected status 500, got %d", apiErr.StatusCode)
	}
}

func TestAnthropicMaxTokensFromRequest(t *testing.T) {
	server, provider := newAnthropicTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		var ar anthropicRequest
		json.NewDecoder(r.Body).Decode(&ar)

		if ar.MaxTokens != 1024 {
			t.Errorf("expected max_tokens 1024, got %d", ar.MaxTokens)
		}

		resp := `{"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":1}}`
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(resp))
	})
	defer server.Close()

	maxTokens := 1024
	req := &api.ChatCompletionRequest{
		Messages:  []api.Message{{Role: "user", Content: "test"}},
		MaxTokens: &maxTokens,
	}

	_, err := provider.ChatCompletion(context.Background(), "claude-sonnet-4-20250514", req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestAnthropicMaxTokensFinishReason(t *testing.T) {
	resp := `{
		"id": "msg_trunc",
		"type": "message",
		"role": "assistant",
		"model": "claude-sonnet-4-20250514",
		"content": [{"type": "text", "text": "Truncated response..."}],
		"stop_reason": "max_tokens",
		"usage": {"input_tokens": 10, "output_tokens": 100}
	}`

	server, provider := newAnthropicTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(resp))
	})
	defer server.Close()

	req := &api.ChatCompletionRequest{
		Messages: []api.Message{{Role: "user", Content: "test"}},
	}

	result, err := provider.ChatCompletion(context.Background(), "claude-sonnet-4-20250514", req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if result.Choices[0].FinishReason == nil || *result.Choices[0].FinishReason != "length" {
		fr := "<nil>"
		if result.Choices[0].FinishReason != nil {
			fr = *result.Choices[0].FinishReason
		}
		t.Errorf("expected finish_reason 'length' for max_tokens stop, got '%s'", fr)
	}
}

func TestAnthropicProviderFactory(t *testing.T) {
	cfg := ProviderConfig{
		ID:      "test",
		Name:    "test",
		BaseURL: "https://api.anthropic.com",
		APIKey:  "sk-test",
	}

	p, err := NewProvider("anthropic", cfg)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if p == nil {
		t.Fatal("expected non-nil provider")
	}
	if p.Name() != "test" {
		t.Errorf("expected name 'test', got '%s'", p.Name())
	}
}

func TestAnthropicMetrics(t *testing.T) {
	var callCount int
	server, provider := newAnthropicTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		callCount++
		if callCount%3 == 0 {
			w.WriteHeader(http.StatusInternalServerError)
			w.Write([]byte(`{"type":"error","error":{"type":"api_error","message":"server error"}}`))
			return
		}
		resp := `{"id":"msg_m","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":1}}`
		w.Header().Set("Content-Type", "application/json")
		w.Write([]byte(resp))
	})
	defer server.Close()

	req := &api.ChatCompletionRequest{
		Messages: []api.Message{{Role: "user", Content: "test"}},
	}

	for i := 0; i < 9; i++ {
		provider.ChatCompletion(context.Background(), "claude-sonnet-4-20250514", req)
	}

	metrics := provider.Metrics()
	expectedErrorRate := 3.0 / 9.0
	if diff := metrics.ErrorRate - expectedErrorRate; diff > 0.01 || diff < -0.01 {
		t.Errorf("expected error rate ~%.2f, got %.2f", expectedErrorRate, metrics.ErrorRate)
	}
}
