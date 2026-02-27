package provider

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/aspect-build/prism-gateway/pkg/api"
)

// newTestServer creates a test HTTP server and returns it along with a configured OpenAIProvider.
func newTestServer(t *testing.T, handler http.HandlerFunc) (*httptest.Server, *OpenAIProvider) {
	t.Helper()
	server := httptest.NewServer(handler)
	provider := NewOpenAIProvider(ProviderConfig{
		ID:      "test",
		Name:    "test-openai",
		BaseURL: server.URL,
		APIKey:  "test-key-123",
	})
	return server, provider
}

func strPtr(s string) *string {
	return &s
}

func TestOpenAIChatCompletion(t *testing.T) {
	expectedResp := &api.ChatCompletionResponse{
		ID:      "chatcmpl-abc123",
		Object:  "chat.completion",
		Created: 1700000000,
		Model:   "gpt-4",
		Choices: []api.Choice{
			{
				Index: 0,
				Message: &api.Message{
					Role:    "assistant",
					Content: "Hello! How can I help you?",
				},
				FinishReason: strPtr("stop"),
			},
		},
		Usage: &api.Usage{
			PromptTokens:     10,
			CompletionTokens: 8,
			TotalTokens:      18,
		},
	}

	server, provider := newTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		// Verify request
		if r.Method != http.MethodPost {
			t.Errorf("expected POST, got %s", r.Method)
		}
		if r.URL.Path != "/chat/completions" {
			t.Errorf("expected /chat/completions, got %s", r.URL.Path)
		}
		if auth := r.Header.Get("Authorization"); auth != "Bearer test-key-123" {
			t.Errorf("expected Bearer test-key-123, got %s", auth)
		}
		if ct := r.Header.Get("Content-Type"); ct != "application/json" {
			t.Errorf("expected application/json, got %s", ct)
		}

		// Verify request body
		var req api.ChatCompletionRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Errorf("failed to decode request body: %v", err)
		}
		if req.Stream {
			t.Error("expected stream=false for non-streaming request")
		}
		if len(req.Messages) != 1 || req.Messages[0].Content != "Hello" {
			t.Errorf("unexpected messages: %+v", req.Messages)
		}

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(expectedResp)
	})
	defer server.Close()

	req := &api.ChatCompletionRequest{
		Messages: []api.Message{
			{Role: "user", Content: "Hello"},
		},
	}

	resp, err := provider.ChatCompletion(context.Background(), "gpt-4", req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if resp.ID != expectedResp.ID {
		t.Errorf("expected ID %s, got %s", expectedResp.ID, resp.ID)
	}
	if len(resp.Choices) != 1 {
		t.Fatalf("expected 1 choice, got %d", len(resp.Choices))
	}
	if resp.Choices[0].Message.Content != "Hello! How can I help you?" {
		t.Errorf("unexpected content: %s", resp.Choices[0].Message.Content)
	}
	if resp.Usage.TotalTokens != 18 {
		t.Errorf("expected 18 total tokens, got %d", resp.Usage.TotalTokens)
	}
}

func TestOpenAIChatCompletionStream(t *testing.T) {
	sseData := "data: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"

	server, provider := newTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		// Verify stream=true in request
		var req api.ChatCompletionRequest
		json.NewDecoder(r.Body).Decode(&req)
		if !req.Stream {
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

	stream, err := provider.ChatCompletionStream(context.Background(), "gpt-4", req)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer stream.Close()

	// First chunk: "Hello"
	chunk, err := stream.Recv()
	if err != nil {
		t.Fatalf("unexpected error on first recv: %v", err)
	}
	if chunk.Choices[0].Delta.Content != "Hello" {
		t.Errorf("expected 'Hello', got '%s'", chunk.Choices[0].Delta.Content)
	}

	// Second chunk: " world"
	chunk, err = stream.Recv()
	if err != nil {
		t.Fatalf("unexpected error on second recv: %v", err)
	}
	if chunk.Choices[0].Delta.Content != " world" {
		t.Errorf("expected ' world', got '%s'", chunk.Choices[0].Delta.Content)
	}

	// Third chunk: finish_reason="stop"
	chunk, err = stream.Recv()
	if err != nil {
		t.Fatalf("unexpected error on third recv: %v", err)
	}
	if chunk.Choices[0].FinishReason == nil || *chunk.Choices[0].FinishReason != "stop" {
		t.Error("expected finish_reason='stop'")
	}

	// Fourth recv: EOF (after [DONE])
	_, err = stream.Recv()
	if err != io.EOF {
		t.Errorf("expected io.EOF, got %v", err)
	}
}

func TestOpenAIChatCompletionError(t *testing.T) {
	tests := []struct {
		name       string
		statusCode int
		body       string
		wantMsg    string
	}{
		{
			name:       "401 Unauthorized",
			statusCode: 401,
			body:       `{"error":{"message":"Invalid API key","type":"invalid_request_error","code":"invalid_api_key"}}`,
			wantMsg:    "Invalid API key",
		},
		{
			name:       "500 Server Error",
			statusCode: 500,
			body:       `{"error":{"message":"Internal server error","type":"server_error"}}`,
			wantMsg:    "Internal server error",
		},
		{
			name:       "Plain text error",
			statusCode: 502,
			body:       "Bad Gateway",
			wantMsg:    "Bad Gateway",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			server, provider := newTestServer(t, func(w http.ResponseWriter, r *http.Request) {
				w.WriteHeader(tt.statusCode)
				w.Write([]byte(tt.body))
			})
			defer server.Close()

			req := &api.ChatCompletionRequest{
				Messages: []api.Message{{Role: "user", Content: "test"}},
			}

			_, err := provider.ChatCompletion(context.Background(), "gpt-4", req)
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
		})
	}
}

func TestOpenAIHealthCheck(t *testing.T) {
	t.Run("healthy", func(t *testing.T) {
		server, provider := newTestServer(t, func(w http.ResponseWriter, r *http.Request) {
			if r.URL.Path != "/models" {
				t.Errorf("expected /models, got %s", r.URL.Path)
			}
			if r.Method != http.MethodGet {
				t.Errorf("expected GET, got %s", r.Method)
			}
			if auth := r.Header.Get("Authorization"); auth != "Bearer test-key-123" {
				t.Errorf("expected Bearer test-key-123, got %s", auth)
			}

			w.Header().Set("Content-Type", "application/json")
			w.Write([]byte(`{"data":[{"id":"gpt-4"}]}`))
		})
		defer server.Close()

		err := provider.HealthCheck(context.Background())
		if err != nil {
			t.Fatalf("unexpected error: %v", err)
		}
	})

	t.Run("unhealthy", func(t *testing.T) {
		server, provider := newTestServer(t, func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusUnauthorized)
			w.Write([]byte(`{"error":{"message":"invalid key"}}`))
		})
		defer server.Close()

		err := provider.HealthCheck(context.Background())
		if err == nil {
			t.Fatal("expected error, got nil")
		}
		if !strings.Contains(err.Error(), "401") {
			t.Errorf("expected error to mention 401, got: %v", err)
		}
	})
}

func TestOpenAITimeout(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(2 * time.Second)
		w.WriteHeader(http.StatusOK)
	}))
	defer server.Close()

	provider := NewOpenAIProvider(ProviderConfig{
		ID:        "test",
		Name:      "test-openai",
		BaseURL:   server.URL,
		APIKey:    "test-key",
		TimeoutMs: 100,
	})

	req := &api.ChatCompletionRequest{
		Messages: []api.Message{{Role: "user", Content: "test"}},
	}

	_, err := provider.ChatCompletion(context.Background(), "gpt-4", req)
	if err == nil {
		t.Fatal("expected timeout error, got nil")
	}
	if !strings.Contains(err.Error(), "request failed") {
		t.Errorf("expected request failed error, got: %v", err)
	}
}

func TestOpenAIStreamDone(t *testing.T) {
	sseData := "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"gpt-4\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n"

	server, provider := newTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(sseData))
	})
	defer server.Close()

	stream, err := provider.ChatCompletionStream(context.Background(), "gpt-4", &api.ChatCompletionRequest{
		Messages: []api.Message{{Role: "user", Content: "test"}},
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer stream.Close()

	chunk, err := stream.Recv()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if chunk.Choices[0].Delta.Content != "hi" {
		t.Errorf("expected 'hi', got '%s'", chunk.Choices[0].Delta.Content)
	}

	_, err = stream.Recv()
	if err != io.EOF {
		t.Errorf("expected io.EOF, got %v", err)
	}
}

func TestOpenAIStreamMalformed(t *testing.T) {
	sseData := "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"gpt-4\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"good\"}}]}\n\ndata: {MALFORMED JSON}\n\ndata: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"gpt-4\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" data\"}}]}\n\nevent: ping\n\ndata: [DONE]\n"

	server, provider := newTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		w.WriteHeader(http.StatusOK)
		w.Write([]byte(sseData))
	})
	defer server.Close()

	stream, err := provider.ChatCompletionStream(context.Background(), "gpt-4", &api.ChatCompletionRequest{
		Messages: []api.Message{{Role: "user", Content: "test"}},
	})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	defer stream.Close()

	// First chunk: "good"
	chunk, err := stream.Recv()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if chunk.Choices[0].Delta.Content != "good" {
		t.Errorf("expected 'good', got '%s'", chunk.Choices[0].Delta.Content)
	}

	// Second chunk: " data" (malformed line should be skipped)
	chunk, err = stream.Recv()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if chunk.Choices[0].Delta.Content != " data" {
		t.Errorf("expected ' data', got '%s'", chunk.Choices[0].Delta.Content)
	}

	// Should get EOF after [DONE]
	_, err = stream.Recv()
	if err != io.EOF {
		t.Errorf("expected io.EOF, got %v", err)
	}
}

func TestOpenAIRateLimitError(t *testing.T) {
	server, provider := newTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Retry-After", "30")
		w.WriteHeader(http.StatusTooManyRequests)
		w.Write([]byte(`{"error":{"message":"Rate limit exceeded","type":"rate_limit_error"}}`))
	})
	defer server.Close()

	req := &api.ChatCompletionRequest{
		Messages: []api.Message{{Role: "user", Content: "test"}},
	}

	_, err := provider.ChatCompletion(context.Background(), "gpt-4", req)
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

	if apiErr.StatusCode != 429 {
		t.Errorf("expected status 429, got %d", apiErr.StatusCode)
	}

	if apiErr.Type != "rate_limit_error" {
		t.Errorf("expected type 'rate_limit_error', got '%s'", apiErr.Type)
	}

	// Also test streaming returns rate limit error
	_, err = provider.ChatCompletionStream(context.Background(), "gpt-4", req)
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

func TestOpenAIConcurrent(t *testing.T) {
	var requestCount atomic.Int64

	server, provider := newTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		requestCount.Add(1)

		finish := "stop"
		resp := api.ChatCompletionResponse{
			ID:      fmt.Sprintf("chatcmpl-%d", requestCount.Load()),
			Object:  "chat.completion",
			Created: time.Now().Unix(),
			Model:   "gpt-4",
			Choices: []api.Choice{
				{
					Index:        0,
					Message:      &api.Message{Role: "assistant", Content: "OK"},
					FinishReason: &finish,
				},
			},
			Usage: &api.Usage{PromptTokens: 5, CompletionTokens: 1, TotalTokens: 6},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})
	defer server.Close()

	const numGoroutines = 50
	var wg sync.WaitGroup
	errors := make(chan error, numGoroutines)

	for i := 0; i < numGoroutines; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()

			req := &api.ChatCompletionRequest{
				Messages: []api.Message{{Role: "user", Content: fmt.Sprintf("request %d", i)}},
			}

			resp, err := provider.ChatCompletion(context.Background(), "gpt-4", req)
			if err != nil {
				errors <- fmt.Errorf("goroutine %d: %w", i, err)
				return
			}
			if len(resp.Choices) == 0 {
				errors <- fmt.Errorf("goroutine %d: no choices", i)
				return
			}
		}(i)
	}

	wg.Wait()
	close(errors)

	for err := range errors {
		t.Errorf("concurrent error: %v", err)
	}

	count := requestCount.Load()
	if count != numGoroutines {
		t.Errorf("expected %d requests, got %d", numGoroutines, count)
	}

	// Verify metrics are consistent
	metrics := provider.Metrics()
	if metrics.ErrorRate != 0 {
		t.Errorf("expected 0 error rate, got %f", metrics.ErrorRate)
	}
	if metrics.LatencyP50Ms <= 0 {
		t.Error("expected positive P50 latency")
	}
}

func TestOpenAIProviderFactory(t *testing.T) {
	cfg := ProviderConfig{
		ID:      "test",
		Name:    "test",
		BaseURL: "https://api.openai.com/v1",
		APIKey:  "sk-test",
	}

	tests := []struct {
		providerType string
		expectErr    bool
	}{
		{"openai", false},
		{"deepseek", false},
		{"openai-compatible", false},
		{"mock", false},
		{"unknown", true},
	}

	for _, tt := range tests {
		t.Run(tt.providerType, func(t *testing.T) {
			p, err := NewProvider(tt.providerType, cfg)
			if tt.expectErr {
				if err == nil {
					t.Error("expected error, got nil")
				}
			} else {
				if err != nil {
					t.Errorf("unexpected error: %v", err)
				}
				if p == nil {
					t.Error("expected non-nil provider")
				}
			}
		})
	}
}

func TestOpenAIMetrics(t *testing.T) {
	var callCount int
	server, provider := newTestServer(t, func(w http.ResponseWriter, r *http.Request) {
		callCount++
		if callCount%3 == 0 {
			w.WriteHeader(http.StatusInternalServerError)
			w.Write([]byte(`{"error":{"message":"server error"}}`))
			return
		}
		finish := "stop"
		resp := api.ChatCompletionResponse{
			ID:      "chatcmpl-metrics",
			Object:  "chat.completion",
			Model:   "gpt-4",
			Choices: []api.Choice{{Index: 0, Message: &api.Message{Role: "assistant", Content: "ok"}, FinishReason: &finish}},
		}
		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(resp)
	})
	defer server.Close()

	req := &api.ChatCompletionRequest{
		Messages: []api.Message{{Role: "user", Content: "test"}},
	}

	for i := 0; i < 9; i++ {
		provider.ChatCompletion(context.Background(), "gpt-4", req)
	}

	metrics := provider.Metrics()

	expectedErrorRate := 3.0 / 9.0
	if diff := metrics.ErrorRate - expectedErrorRate; diff > 0.01 || diff < -0.01 {
		t.Errorf("expected error rate ~%.2f, got %.2f", expectedErrorRate, metrics.ErrorRate)
	}
}
