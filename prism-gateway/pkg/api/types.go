// Package api defines OpenAI-compatible request/response types
// and Prism-specific extensions.
package api

// ──────────────────────────────────────────────
// OpenAI-compatible types
// ──────────────────────────────────────────────

// ChatCompletionRequest mirrors the OpenAI chat completion request.
type ChatCompletionRequest struct {
	Model       string    `json:"model"`
	Messages    []Message `json:"messages"`
	Stream      bool      `json:"stream,omitempty"`
	Temperature *float64  `json:"temperature,omitempty"`
	TopP        *float64  `json:"top_p,omitempty"`
	MaxTokens   *int      `json:"max_tokens,omitempty"`
	Stop        []string  `json:"stop,omitempty"`
	N           int       `json:"n,omitempty"`

	// Prism extensions (optional)
	Prism *PrismExtension `json:"prism,omitempty"`
}

// Message represents a single chat message.
type Message struct {
	Role    string `json:"role"`
	Content string `json:"content"`
}

// ChatCompletionResponse mirrors the OpenAI chat completion response.
type ChatCompletionResponse struct {
	ID      string   `json:"id"`
	Object  string   `json:"object"`
	Created int64    `json:"created"`
	Model   string   `json:"model"`
	Choices []Choice `json:"choices"`
	Usage   *Usage   `json:"usage,omitempty"`
}

// Choice is a single completion choice.
type Choice struct {
	Index        int      `json:"index"`
	Message      *Message `json:"message,omitempty"`
	Delta        *Message `json:"delta,omitempty"`
	FinishReason *string  `json:"finish_reason,omitempty"`
}

// Usage tracks token consumption.
type Usage struct {
	PromptTokens     int `json:"prompt_tokens"`
	CompletionTokens int `json:"completion_tokens"`
	TotalTokens      int `json:"total_tokens"`
}

// ChatCompletionChunk is a single streaming chunk (SSE).
type ChatCompletionChunk struct {
	ID      string   `json:"id"`
	Object  string   `json:"object"`
	Created int64    `json:"created"`
	Model   string   `json:"model"`
	Choices []Choice `json:"choices"`
}

// ──────────────────────────────────────────────
// Prism extensions
// ──────────────────────────────────────────────

// PrismExtension carries routing directives in the request body.
type PrismExtension struct {
	Strategy string   `json:"strategy,omitempty"`
	PoolIDs  []string `json:"pool_ids,omitempty"`
	Fallback []string `json:"fallback,omitempty"`
	Exclude  []string `json:"exclude,omitempty"`
	Trace    bool     `json:"trace,omitempty"`

	// SLO constraints
	SLO *SLOConstraint `json:"slo,omitempty"`

	// Balanced weights
	CostWeight    *float32 `json:"cost_weight,omitempty"`
	QualityWeight *float32 `json:"quality_weight,omitempty"`
	LatencyWeight *float32 `json:"latency_weight,omitempty"`

	// AnalyzeContext fields
	CallerType string `json:"caller_type,omitempty"`
	Source     string `json:"source,omitempty"`
}

// SLOConstraint defines latency SLO requirements.
type SLOConstraint struct {
	MaxLatencyMs int `json:"max_latency_ms,omitempty"`
	MaxTTFTMs    int `json:"max_ttft_ms,omitempty"`
}

// ──────────────────────────────────────────────
// Outcome reporting
// ──────────────────────────────────────────────

// OutcomeReport allows clients to report request results back to Prism.
type OutcomeReport struct {
	RequestID string `json:"request_id"`
	VariantID string `json:"variant_id"`
	ProviderID string `json:"provider_id"`
	ModelID   string `json:"model_id"`

	// Timing
	TotalLatencyMs int64  `json:"total_latency_ms"`
	TTFTMs         *int64 `json:"ttft_ms,omitempty"`

	// Result
	Success    bool   `json:"success"`
	StatusCode int    `json:"status_code,omitempty"`
	ErrorType  string `json:"error_type,omitempty"`

	// Usage
	PromptTokens     int `json:"prompt_tokens,omitempty"`
	CompletionTokens int `json:"completion_tokens,omitempty"`

	// Quality feedback (optional)
	UserRating *int `json:"user_rating,omitempty"` // 1-5
}

// OutcomeResponse is the response to an outcome report.
type OutcomeResponse struct {
	Status string `json:"status"`
}

// ──────────────────────────────────────────────
// Prism response metadata (returned in headers)
// ──────────────────────────────────────────────

// RoutingInfo is the metadata about a routing decision, returned to clients.
type RoutingInfo struct {
	Model          string `json:"model"`
	Provider       string `json:"provider"`
	Variant        string `json:"variant"`
	Account        string `json:"account"`
	Strategy       string `json:"strategy"`
	DecisionTimeUs int64  `json:"decision_time_us"`
	Reason         string `json:"reason"`
	FallbackChain  string `json:"fallback_chain"`
	RequestCost    string `json:"request_cost,omitempty"`
}

// ──────────────────────────────────────────────
// Header constants
// ──────────────────────────────────────────────

const (
	// Request headers
	HeaderPrismStrategy     = "X-Prism-Strategy"
	HeaderPrismCostWeight   = "X-Prism-Cost-Weight"
	HeaderPrismQualWeight   = "X-Prism-Quality-Weight"
	HeaderPrismLatWeight    = "X-Prism-Latency-Weight"
	HeaderPrismMaxLatencyMs = "X-Prism-Max-Latency-Ms"
	HeaderPrismMaxTTFTMs    = "X-Prism-Max-Ttft-Ms"
	HeaderPrismModelPool    = "X-Prism-Model-Pool"
	HeaderPrismTraceID      = "X-Prism-Trace-Id"
	HeaderPrismAffinityKey  = "X-Prism-Affinity-Key"
	HeaderPrismSessionID    = "X-Prism-Session-Id"
	HeaderPrismCallerType   = "X-Prism-Caller-Type"
	HeaderPrismSource       = "X-Prism-Source"
	HeaderPrismTenantID     = "X-Prism-Tenant-Id"

	// Response headers
	HeaderPrismModel        = "X-Prism-Model"
	HeaderPrismProvider     = "X-Prism-Provider"
	HeaderPrismVariant      = "X-Prism-Variant"
	HeaderPrismAccount      = "X-Prism-Account"
	HeaderPrismDecisionTime = "X-Prism-Decision-Time-Us"
	HeaderPrismReason       = "X-Prism-Reason"
	HeaderPrismFallback     = "X-Prism-Fallback-Chain"
	HeaderPrismRequestCost  = "X-Prism-Request-Cost"

	// A/B experiment response headers
	HeaderPrismExperimentID      = "X-Prism-Experiment-Id"
	HeaderPrismExperimentVariant = "X-Prism-Experiment-Variant"
)
