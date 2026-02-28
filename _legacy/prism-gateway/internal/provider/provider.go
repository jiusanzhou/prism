// Package provider defines the interface for LLM service providers.
package provider

import (
	"context"
	"time"

	"go.zoe.im/prism-gateway/pkg/api"
)

// Provider abstracts an LLM service provider (OpenAI, Anthropic, etc.).
type Provider interface {
	// Name returns the provider identifier.
	Name() string

	// ChatCompletion sends a non-streaming request.
	ChatCompletion(ctx context.Context, model string, req *api.ChatCompletionRequest) (*api.ChatCompletionResponse, error)

	// ChatCompletionStream sends a streaming request.
	ChatCompletionStream(ctx context.Context, model string, req *api.ChatCompletionRequest) (Stream, error)

	// HealthCheck verifies the provider is reachable.
	HealthCheck(ctx context.Context) error

	// Metrics returns current provider metrics.
	Metrics() *ProviderMetrics
}

// Stream represents a streaming response from a provider.
type Stream interface {
	// Recv returns the next chunk. Returns io.EOF when done.
	Recv() (*api.ChatCompletionChunk, error)

	// Close releases resources.
	Close() error
}

// ProviderMetrics holds real-time metrics for a provider.
type ProviderMetrics struct {
	LatencyP50Ms float64
	LatencyP99Ms float64
	ErrorRate    float64
	CurrentQPS   float64
	LastUpdated  time.Time
}

// ProviderConfig holds configuration for a single provider.
type ProviderConfig struct {
	ID            string
	Name          string
	BaseURL       string
	APIKey        string
	TimeoutMs     int
	MaxConcurrent int
}
