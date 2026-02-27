package provider

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/aspect-build/prism-gateway/pkg/api"
)

// OpenAIProvider implements Provider for OpenAI and OpenAI-compatible APIs.
type OpenAIProvider struct {
	cfg    ProviderConfig
	client *http.Client

	// Metrics tracking
	mu            sync.Mutex
	totalCalls    atomic.Int64
	totalErrors   atomic.Int64
	latencySum    atomic.Int64 // microseconds
	latencyCount  atomic.Int64
	lastLatencies []float64 // recent latencies in ms for percentile calculation
}

// NewOpenAIProvider creates a new OpenAI provider with the given configuration.
func NewOpenAIProvider(cfg ProviderConfig) *OpenAIProvider {
	timeout := 30 * time.Second
	if cfg.TimeoutMs > 0 {
		timeout = time.Duration(cfg.TimeoutMs) * time.Millisecond
	}

	return &OpenAIProvider{
		cfg: cfg,
		client: &http.Client{
			Timeout: timeout,
		},
		lastLatencies: make([]float64, 0, 1000),
	}
}

func (p *OpenAIProvider) Name() string {
	return p.cfg.Name
}

// ChatCompletion sends a non-streaming chat completion request.
func (p *OpenAIProvider) ChatCompletion(ctx context.Context, model string, req *api.ChatCompletionRequest) (*api.ChatCompletionResponse, error) {
	p.totalCalls.Add(1)
	start := time.Now()

	// Ensure stream is false for non-streaming
	reqCopy := *req
	reqCopy.Stream = false
	if model != "" {
		reqCopy.Model = model
	}

	body, err := json.Marshal(reqCopy)
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("openai: failed to marshal request: %w", err)
	}

	url := strings.TrimRight(p.cfg.BaseURL, "/") + "/chat/completions"
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("openai: failed to create request: %w", err)
	}

	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Authorization", "Bearer "+p.cfg.APIKey)

	resp, err := p.client.Do(httpReq)
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("openai: request failed: %w", err)
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("openai: failed to read response body: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		p.totalErrors.Add(1)
		return nil, p.parseAPIError(resp.StatusCode, respBody)
	}

	var result api.ChatCompletionResponse
	if err := json.Unmarshal(respBody, &result); err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("openai: failed to unmarshal response: %w", err)
	}

	p.recordLatency(time.Since(start))
	return &result, nil
}

// ChatCompletionStream sends a streaming chat completion request.
func (p *OpenAIProvider) ChatCompletionStream(ctx context.Context, model string, req *api.ChatCompletionRequest) (Stream, error) {
	p.totalCalls.Add(1)
	start := time.Now()

	// Force stream to true
	reqCopy := *req
	reqCopy.Stream = true
	if model != "" {
		reqCopy.Model = model
	}

	body, err := json.Marshal(reqCopy)
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("openai: failed to marshal request: %w", err)
	}

	url := strings.TrimRight(p.cfg.BaseURL, "/") + "/chat/completions"
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("openai: failed to create request: %w", err)
	}

	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("Authorization", "Bearer "+p.cfg.APIKey)

	resp, err := p.client.Do(httpReq)
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("openai: request failed: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		defer resp.Body.Close()
		respBody, _ := io.ReadAll(resp.Body)
		p.totalErrors.Add(1)
		return nil, p.parseAPIError(resp.StatusCode, respBody)
	}

	p.recordLatency(time.Since(start))

	return &openaiStream{
		reader: bufio.NewScanner(resp.Body),
		body:   resp.Body,
	}, nil
}

// HealthCheck verifies connectivity by calling the /models endpoint.
func (p *OpenAIProvider) HealthCheck(ctx context.Context) error {
	url := strings.TrimRight(p.cfg.BaseURL, "/") + "/models"
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return fmt.Errorf("openai: failed to create health check request: %w", err)
	}

	httpReq.Header.Set("Authorization", "Bearer "+p.cfg.APIKey)

	resp, err := p.client.Do(httpReq)
	if err != nil {
		return fmt.Errorf("openai: health check failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("openai: health check returned status %d: %s", resp.StatusCode, string(body))
	}

	return nil
}

// Metrics returns current provider metrics.
func (p *OpenAIProvider) Metrics() *ProviderMetrics {
	total := p.totalCalls.Load()
	errors := p.totalErrors.Load()

	var errorRate float64
	if total > 0 {
		errorRate = float64(errors) / float64(total)
	}

	p.mu.Lock()
	p50, p99 := p.calculatePercentiles()
	p.mu.Unlock()

	return &ProviderMetrics{
		LatencyP50Ms: p50,
		LatencyP99Ms: p99,
		ErrorRate:    errorRate,
		CurrentQPS:   0, // Would need a sliding window for real QPS
		LastUpdated:  time.Now(),
	}
}

// recordLatency records a latency measurement.
func (p *OpenAIProvider) recordLatency(d time.Duration) {
	ms := float64(d.Microseconds()) / 1000.0
	p.latencySum.Add(int64(d.Microseconds()))
	p.latencyCount.Add(1)

	p.mu.Lock()
	defer p.mu.Unlock()

	// Keep a bounded window of recent latencies
	if len(p.lastLatencies) >= 1000 {
		p.lastLatencies = p.lastLatencies[1:]
	}
	p.lastLatencies = append(p.lastLatencies, ms)
}

// calculatePercentiles computes p50 and p99 from recent latencies.
// Must be called with p.mu held.
func (p *OpenAIProvider) calculatePercentiles() (p50, p99 float64) {
	n := len(p.lastLatencies)
	if n == 0 {
		return 0, 0
	}

	// Copy and sort
	sorted := make([]float64, n)
	copy(sorted, p.lastLatencies)
	sortFloat64s(sorted)

	p50 = sorted[n*50/100]
	p99 = sorted[n*99/100]
	return p50, p99
}

// sortFloat64s sorts a slice of float64 in ascending order (simple insertion sort for bounded data).
func sortFloat64s(a []float64) {
	for i := 1; i < len(a); i++ {
		key := a[i]
		j := i - 1
		for j >= 0 && a[j] > key {
			a[j+1] = a[j]
			j--
		}
		a[j+1] = key
	}
}

// APIError represents an error returned by the OpenAI API.
type APIError struct {
	StatusCode int
	Message    string
	Type       string
}

func (e *APIError) Error() string {
	if e.Type != "" {
		return fmt.Sprintf("openai: API error (status %d, type %s): %s", e.StatusCode, e.Type, e.Message)
	}
	return fmt.Errorf("openai: API error (status %d): %s", e.StatusCode, e.Message).Error()
}

// IsRateLimit returns true if this is a rate limit (429) error.
func (e *APIError) IsRateLimit() bool {
	return e.StatusCode == http.StatusTooManyRequests
}

// parseAPIError parses an error response body into an APIError.
func (p *OpenAIProvider) parseAPIError(statusCode int, body []byte) *APIError {
	apiErr := &APIError{
		StatusCode: statusCode,
		Message:    string(body),
	}

	// Try to parse structured error response
	var errResp struct {
		Error struct {
			Message string `json:"message"`
			Type    string `json:"type"`
			Code    string `json:"code"`
		} `json:"error"`
	}
	if err := json.Unmarshal(body, &errResp); err == nil && errResp.Error.Message != "" {
		apiErr.Message = errResp.Error.Message
		apiErr.Type = errResp.Error.Type
	}

	return apiErr
}

// openaiStream implements Stream for SSE-based streaming responses.
type openaiStream struct {
	reader *bufio.Scanner
	body   io.ReadCloser
}

func (s *openaiStream) Recv() (*api.ChatCompletionChunk, error) {
	for s.reader.Scan() {
		line := s.reader.Text()

		// Skip empty lines
		if line == "" {
			continue
		}

		// Skip non-data lines (e.g., event:, id:, retry:)
		if !strings.HasPrefix(line, "data: ") {
			continue
		}

		data := strings.TrimPrefix(line, "data: ")

		// Check for stream termination
		if data == "[DONE]" {
			return nil, io.EOF
		}

		var chunk api.ChatCompletionChunk
		if err := json.Unmarshal([]byte(data), &chunk); err != nil {
			// Skip malformed chunks and try to read the next one
			continue
		}

		return &chunk, nil
	}

	// Scanner finished (connection closed or error)
	if err := s.reader.Err(); err != nil {
		return nil, fmt.Errorf("openai: stream read error: %w", err)
	}
	return nil, io.EOF
}

func (s *openaiStream) Close() error {
	if s.body != nil {
		return s.body.Close()
	}
	return nil
}
