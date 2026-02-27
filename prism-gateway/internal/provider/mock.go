package provider

import (
	"context"
	"fmt"
	"io"
	"time"

	"github.com/aspect-build/prism-gateway/pkg/api"
)

// MockProvider implements Provider for testing.
type MockProvider struct {
	name      string
	responses map[string]*api.ChatCompletionResponse
	errors    map[string]error
	metrics   *ProviderMetrics
	calls     []MockCall
}

// MockCall records a call made to the mock.
type MockCall struct {
	Model   string
	Request *api.ChatCompletionRequest
	Time    time.Time
}

// NewMockProvider creates a mock provider with default healthy metrics.
func NewMockProvider(name string) *MockProvider {
	return &MockProvider{
		name:      name,
		responses: make(map[string]*api.ChatCompletionResponse),
		errors:    make(map[string]error),
		metrics: &ProviderMetrics{
			LatencyP50Ms: 100,
			LatencyP99Ms: 500,
			ErrorRate:    0.01,
			CurrentQPS:   10,
			LastUpdated:  time.Now(),
		},
	}
}

// WithResponse configures a response for a given model.
func (m *MockProvider) WithResponse(model string, resp *api.ChatCompletionResponse) *MockProvider {
	m.responses[model] = resp
	return m
}

// WithError configures an error for a given model.
func (m *MockProvider) WithError(model string, err error) *MockProvider {
	m.errors[model] = err
	return m
}

// Calls returns all recorded calls.
func (m *MockProvider) Calls() []MockCall {
	return m.calls
}

func (m *MockProvider) Name() string { return m.name }

func (m *MockProvider) ChatCompletion(ctx context.Context, model string, req *api.ChatCompletionRequest) (*api.ChatCompletionResponse, error) {
	m.calls = append(m.calls, MockCall{Model: model, Request: req, Time: time.Now()})

	if err, ok := m.errors[model]; ok {
		return nil, err
	}
	if resp, ok := m.responses[model]; ok {
		return resp, nil
	}

	// Default response
	finish := "stop"
	return &api.ChatCompletionResponse{
		ID:      "chatcmpl-mock-" + model,
		Object:  "chat.completion",
		Created: time.Now().Unix(),
		Model:   model,
		Choices: []api.Choice{
			{
				Index: 0,
				Message: &api.Message{
					Role:    "assistant",
					Content: fmt.Sprintf("Mock response from %s/%s", m.name, model),
				},
				FinishReason: &finish,
			},
		},
		Usage: &api.Usage{
			PromptTokens:     10,
			CompletionTokens: 20,
			TotalTokens:      30,
		},
	}, nil
}

func (m *MockProvider) ChatCompletionStream(ctx context.Context, model string, req *api.ChatCompletionRequest) (Stream, error) {
	m.calls = append(m.calls, MockCall{Model: model, Request: req, Time: time.Now()})

	if err, ok := m.errors[model]; ok {
		return nil, err
	}

	return &mockStream{
		model:    model,
		provider: m.name,
		index:    0,
		chunks: []string{
			"Mock ",
			"streaming ",
			"response",
		},
	}, nil
}

func (m *MockProvider) HealthCheck(ctx context.Context) error {
	return nil
}

func (m *MockProvider) Metrics() *ProviderMetrics {
	return m.metrics
}

// mockStream implements Stream for testing.
type mockStream struct {
	model    string
	provider string
	index    int
	chunks   []string
}

func (s *mockStream) Recv() (*api.ChatCompletionChunk, error) {
	if s.index >= len(s.chunks) {
		return nil, io.EOF
	}

	chunk := &api.ChatCompletionChunk{
		ID:      fmt.Sprintf("chatcmpl-stream-%d", s.index),
		Object:  "chat.completion.chunk",
		Created: time.Now().Unix(),
		Model:   s.model,
		Choices: []api.Choice{
			{
				Index: 0,
				Delta: &api.Message{
					Content: s.chunks[s.index],
				},
			},
		},
	}
	s.index++
	return chunk, nil
}

func (s *mockStream) Close() error {
	return nil
}
