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

	"go.zoe.im/prism-gateway/pkg/api"
)

const (
	defaultAnthropicBaseURL = "https://api.anthropic.com"
	defaultAnthropicVersion = "2023-06-01"
)

// AnthropicProvider implements Provider for Anthropic's Claude API.
type AnthropicProvider struct {
	cfg     ProviderConfig
	client  *http.Client
	baseURL string
	version string

	// Metrics tracking
	mu            sync.Mutex
	totalCalls    atomic.Int64
	totalErrors   atomic.Int64
	latencySum    atomic.Int64
	latencyCount  atomic.Int64
	lastLatencies []float64
}

// NewAnthropicProvider creates a new Anthropic provider.
func NewAnthropicProvider(cfg ProviderConfig) *AnthropicProvider {
	timeout := 30 * time.Second
	if cfg.TimeoutMs > 0 {
		timeout = time.Duration(cfg.TimeoutMs) * time.Millisecond
	}

	baseURL := cfg.BaseURL
	if baseURL == "" {
		baseURL = defaultAnthropicBaseURL
	}

	return &AnthropicProvider{
		cfg: cfg,
		client: &http.Client{
			Timeout: timeout,
		},
		baseURL:       baseURL,
		version:       defaultAnthropicVersion,
		lastLatencies: make([]float64, 0, 1000),
	}
}

func (p *AnthropicProvider) Name() string {
	return p.cfg.Name
}

// ──────────────────────────────────────────────
// Anthropic-specific request/response types
// ──────────────────────────────────────────────

// anthropicRequest is the request format for Anthropic's messages API.
type anthropicRequest struct {
	Model     string            `json:"model"`
	MaxTokens int               `json:"max_tokens"`
	Messages  []anthropicMsg    `json:"messages"`
	System    string            `json:"system,omitempty"`
	Stream    bool              `json:"stream,omitempty"`
	Stop      []string          `json:"stop_sequences,omitempty"`
	Temp      *float64          `json:"temperature,omitempty"`
	TopP      *float64          `json:"top_p,omitempty"`
}

type anthropicMsg struct {
	Role    string `json:"role"`
	Content string `json:"content"`
}

// anthropicResponse is the non-streaming response from Anthropic.
type anthropicResponse struct {
	ID      string              `json:"id"`
	Type    string              `json:"type"`
	Role    string              `json:"role"`
	Model   string              `json:"model"`
	Content []anthropicContent  `json:"content"`
	Stop    *string             `json:"stop_reason"`
	Usage   *anthropicUsage     `json:"usage"`
}

type anthropicContent struct {
	Type string `json:"type"`
	Text string `json:"text"`
}

type anthropicUsage struct {
	InputTokens  int `json:"input_tokens"`
	OutputTokens int `json:"output_tokens"`
}

// anthropicErrorResponse is the error format returned by Anthropic.
type anthropicErrorResponse struct {
	Type  string `json:"type"`
	Error struct {
		Type    string `json:"type"`
		Message string `json:"message"`
	} `json:"error"`
}

// ──────────────────────────────────────────────
// Streaming event types
// ──────────────────────────────────────────────

type anthropicStreamEvent struct {
	Type string `json:"type"`
}

type anthropicMessageStart struct {
	Type    string `json:"type"`
	Message struct {
		ID    string `json:"id"`
		Model string `json:"model"`
		Usage *anthropicUsage `json:"usage"`
	} `json:"message"`
}

type anthropicContentBlockDelta struct {
	Type  string `json:"type"`
	Index int    `json:"index"`
	Delta struct {
		Type string `json:"type"`
		Text string `json:"text"`
	} `json:"delta"`
}

type anthropicMessageDelta struct {
	Type  string `json:"type"`
	Delta struct {
		StopReason *string `json:"stop_reason"`
	} `json:"delta"`
	Usage *anthropicUsage `json:"usage"`
}

// ──────────────────────────────────────────────
// Request conversion
// ──────────────────────────────────────────────

// convertToAnthropicRequest converts an OpenAI-format request to Anthropic format.
func convertToAnthropicRequest(model string, req *api.ChatCompletionRequest) *anthropicRequest {
	ar := &anthropicRequest{
		Model:     model,
		MaxTokens: 4096,
		Temp:      req.Temperature,
		TopP:      req.TopP,
		Stop:      req.Stop,
	}

	if req.MaxTokens != nil {
		ar.MaxTokens = *req.MaxTokens
	}

	// Separate system messages from regular messages
	for _, msg := range req.Messages {
		if msg.Role == "system" {
			if ar.System != "" {
				ar.System += "\n"
			}
			ar.System += msg.Content
		} else {
			ar.Messages = append(ar.Messages, anthropicMsg{
				Role:    msg.Role,
				Content: msg.Content,
			})
		}
	}

	return ar
}

// convertToOpenAIResponse converts an Anthropic response to OpenAI format.
func convertToOpenAIResponse(resp *anthropicResponse) *api.ChatCompletionResponse {
	// Concatenate all text content blocks
	var content strings.Builder
	for _, block := range resp.Content {
		if block.Type == "text" {
			content.WriteString(block.Text)
		}
	}

	finishReason := "stop"
	if resp.Stop != nil {
		switch *resp.Stop {
		case "end_turn":
			finishReason = "stop"
		case "max_tokens":
			finishReason = "length"
		case "stop_sequence":
			finishReason = "stop"
		default:
			finishReason = "stop"
		}
	}

	oaiResp := &api.ChatCompletionResponse{
		ID:      resp.ID,
		Object:  "chat.completion",
		Created: time.Now().Unix(),
		Model:   resp.Model,
		Choices: []api.Choice{
			{
				Index: 0,
				Message: &api.Message{
					Role:    "assistant",
					Content: content.String(),
				},
				FinishReason: &finishReason,
			},
		},
	}

	if resp.Usage != nil {
		oaiResp.Usage = &api.Usage{
			PromptTokens:     resp.Usage.InputTokens,
			CompletionTokens: resp.Usage.OutputTokens,
			TotalTokens:      resp.Usage.InputTokens + resp.Usage.OutputTokens,
		}
	}

	return oaiResp
}

// ──────────────────────────────────────────────
// Provider implementation
// ──────────────────────────────────────────────

// ChatCompletion sends a non-streaming chat completion request to Anthropic.
func (p *AnthropicProvider) ChatCompletion(ctx context.Context, model string, req *api.ChatCompletionRequest) (*api.ChatCompletionResponse, error) {
	p.totalCalls.Add(1)
	start := time.Now()

	ar := convertToAnthropicRequest(model, req)
	ar.Stream = false

	body, err := json.Marshal(ar)
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("anthropic: failed to marshal request: %w", err)
	}

	url := strings.TrimRight(p.baseURL, "/") + "/v1/messages"
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("anthropic: failed to create request: %w", err)
	}

	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("x-api-key", p.cfg.APIKey)
	httpReq.Header.Set("anthropic-version", p.version)

	resp, err := p.client.Do(httpReq)
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("anthropic: request failed: %w", err)
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("anthropic: failed to read response body: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		p.totalErrors.Add(1)
		return nil, p.parseAPIError(resp.StatusCode, respBody)
	}

	var anthropicResp anthropicResponse
	if err := json.Unmarshal(respBody, &anthropicResp); err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("anthropic: failed to unmarshal response: %w", err)
	}

	p.recordLatency(time.Since(start))
	return convertToOpenAIResponse(&anthropicResp), nil
}

// ChatCompletionStream sends a streaming request to Anthropic.
func (p *AnthropicProvider) ChatCompletionStream(ctx context.Context, model string, req *api.ChatCompletionRequest) (Stream, error) {
	p.totalCalls.Add(1)
	start := time.Now()

	ar := convertToAnthropicRequest(model, req)
	ar.Stream = true

	body, err := json.Marshal(ar)
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("anthropic: failed to marshal request: %w", err)
	}

	url := strings.TrimRight(p.baseURL, "/") + "/v1/messages"
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("anthropic: failed to create request: %w", err)
	}

	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("x-api-key", p.cfg.APIKey)
	httpReq.Header.Set("anthropic-version", p.version)

	resp, err := p.client.Do(httpReq)
	if err != nil {
		p.totalErrors.Add(1)
		return nil, fmt.Errorf("anthropic: request failed: %w", err)
	}

	if resp.StatusCode != http.StatusOK {
		defer resp.Body.Close()
		respBody, _ := io.ReadAll(resp.Body)
		p.totalErrors.Add(1)
		return nil, p.parseAPIError(resp.StatusCode, respBody)
	}

	p.recordLatency(time.Since(start))

	return &anthropicStream{
		reader: bufio.NewScanner(resp.Body),
		body:   resp.Body,
	}, nil
}

// HealthCheck verifies the provider is reachable.
func (p *AnthropicProvider) HealthCheck(ctx context.Context) error {
	// Anthropic doesn't have a /models endpoint. We'll do a lightweight check
	// by sending a minimal request. For now, we just verify connectivity.
	url := strings.TrimRight(p.baseURL, "/") + "/v1/messages"
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader([]byte(`{}`)))
	if err != nil {
		return fmt.Errorf("anthropic: failed to create health check request: %w", err)
	}

	httpReq.Header.Set("Content-Type", "application/json")
	httpReq.Header.Set("x-api-key", p.cfg.APIKey)
	httpReq.Header.Set("anthropic-version", p.version)

	resp, err := p.client.Do(httpReq)
	if err != nil {
		return fmt.Errorf("anthropic: health check failed: %w", err)
	}
	defer resp.Body.Close()
	io.ReadAll(resp.Body)

	// 400 is expected (invalid request body) — it means the API is reachable
	// 401 means bad key, which is still "reachable"
	// Only connection errors indicate unhealthy
	return nil
}

// Metrics returns current provider metrics.
func (p *AnthropicProvider) Metrics() *ProviderMetrics {
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
		CurrentQPS:   0,
		LastUpdated:  time.Now(),
	}
}

// ──────────────────────────────────────────────
// Internal helpers
// ──────────────────────────────────────────────

func (p *AnthropicProvider) recordLatency(d time.Duration) {
	ms := float64(d.Microseconds()) / 1000.0
	p.latencySum.Add(int64(d.Microseconds()))
	p.latencyCount.Add(1)

	p.mu.Lock()
	defer p.mu.Unlock()

	if len(p.lastLatencies) >= 1000 {
		p.lastLatencies = p.lastLatencies[1:]
	}
	p.lastLatencies = append(p.lastLatencies, ms)
}

func (p *AnthropicProvider) calculatePercentiles() (p50, p99 float64) {
	n := len(p.lastLatencies)
	if n == 0 {
		return 0, 0
	}
	sorted := make([]float64, n)
	copy(sorted, p.lastLatencies)
	sortFloat64s(sorted)
	p50 = sorted[n*50/100]
	p99 = sorted[n*99/100]
	return p50, p99
}

func (p *AnthropicProvider) parseAPIError(statusCode int, body []byte) *APIError {
	apiErr := &APIError{
		StatusCode: statusCode,
		Message:    string(body),
	}

	var errResp anthropicErrorResponse
	if err := json.Unmarshal(body, &errResp); err == nil && errResp.Error.Message != "" {
		apiErr.Message = errResp.Error.Message
		apiErr.Type = errResp.Error.Type
	}

	return apiErr
}

// ──────────────────────────────────────────────
// Streaming implementation
// ──────────────────────────────────────────────

// anthropicStream converts Anthropic SSE events to OpenAI-format chunks.
type anthropicStream struct {
	reader    *bufio.Scanner
	body      io.ReadCloser
	messageID string
	model     string
}

func (s *anthropicStream) Recv() (*api.ChatCompletionChunk, error) {
	for s.reader.Scan() {
		line := s.reader.Text()

		// Skip empty lines
		if line == "" {
			continue
		}

		// Parse event type
		if strings.HasPrefix(line, "event: ") {
			eventType := strings.TrimPrefix(line, "event: ")
			if eventType == "message_stop" {
				return nil, io.EOF
			}
			continue
		}

		// Skip non-data lines
		if !strings.HasPrefix(line, "data: ") {
			continue
		}

		data := strings.TrimPrefix(line, "data: ")

		// Parse the event type from the JSON
		var event anthropicStreamEvent
		if err := json.Unmarshal([]byte(data), &event); err != nil {
			continue
		}

		switch event.Type {
		case "message_start":
			var msgStart anthropicMessageStart
			if err := json.Unmarshal([]byte(data), &msgStart); err == nil {
				s.messageID = msgStart.Message.ID
				s.model = msgStart.Message.Model
			}
			// Emit initial chunk with role
			return &api.ChatCompletionChunk{
				ID:      s.messageID,
				Object:  "chat.completion.chunk",
				Created: time.Now().Unix(),
				Model:   s.model,
				Choices: []api.Choice{
					{
						Index: 0,
						Delta: &api.Message{
							Role: "assistant",
						},
					},
				},
			}, nil

		case "content_block_delta":
			var delta anthropicContentBlockDelta
			if err := json.Unmarshal([]byte(data), &delta); err != nil {
				continue
			}
			if delta.Delta.Type == "text_delta" {
				return &api.ChatCompletionChunk{
					ID:      s.messageID,
					Object:  "chat.completion.chunk",
					Created: time.Now().Unix(),
					Model:   s.model,
					Choices: []api.Choice{
						{
							Index: 0,
							Delta: &api.Message{
								Content: delta.Delta.Text,
							},
						},
					},
				}, nil
			}

		case "message_delta":
			var msgDelta anthropicMessageDelta
			if err := json.Unmarshal([]byte(data), &msgDelta); err != nil {
				continue
			}
			finishReason := "stop"
			if msgDelta.Delta.StopReason != nil {
				switch *msgDelta.Delta.StopReason {
				case "end_turn":
					finishReason = "stop"
				case "max_tokens":
					finishReason = "length"
				case "stop_sequence":
					finishReason = "stop"
				}
			}
			return &api.ChatCompletionChunk{
				ID:      s.messageID,
				Object:  "chat.completion.chunk",
				Created: time.Now().Unix(),
				Model:   s.model,
				Choices: []api.Choice{
					{
						Index:        0,
						Delta:        &api.Message{},
						FinishReason: &finishReason,
					},
				},
			}, nil

		case "message_stop":
			return nil, io.EOF

		default:
			// Skip ping, content_block_start, content_block_stop, etc.
			continue
		}
	}

	if err := s.reader.Err(); err != nil {
		return nil, fmt.Errorf("anthropic: stream read error: %w", err)
	}
	return nil, io.EOF
}

func (s *anthropicStream) Close() error {
	if s.body != nil {
		return s.body.Close()
	}
	return nil
}
