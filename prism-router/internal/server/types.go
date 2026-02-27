// Package server defines request/response types matching the proto definitions
// in proto/prism/router/v1/router.proto.
//
// These types are defined manually to avoid a protoc/protoc-gen-go dependency.
// They serve as the HTTP/JSON contract for Mode B (standalone routing service).
package server

// ──────────────────────────────────────────────
// Route Request
// ──────────────────────────────────────────────

// RouteRequest is the JSON equivalent of the proto RouteRequest message.
type RouteRequest struct {
	// Chat messages for complexity / task-type analysis.
	Messages []ChatMessage `json:"messages"`

	// Routing strategy override: "balanced", "cost_first", "slo_first", "quality_first", "pinned".
	Strategy *string `json:"strategy,omitempty"`

	// Model hint. "auto" triggers intelligent routing; a concrete model id activates Pinned mode.
	Model *string `json:"model,omitempty"`

	// Restrict candidates to these model pools.
	PoolIDs []string `json:"pool_ids,omitempty"`

	// Exclude specific models from candidates.
	ExcludeModels []string `json:"exclude_models,omitempty"`

	// Exclude specific providers from candidates.
	ExcludeProviders []string `json:"exclude_providers,omitempty"`

	// Affinity key (e.g. session id) for account-level cache affinity.
	AffinityKey *string `json:"affinity_key,omitempty"`

	// Tenant id for cost-isolation routing.
	TenantID *string `json:"tenant_id,omitempty"`

	// Hard capability constraints — models that don't satisfy are filtered.
	Capabilities *CapabilityRequirements `json:"capabilities,omitempty"`

	// Strategy parameter overrides (weights, SLO thresholds, budgets).
	StrategyParams *StrategyParams `json:"strategy_params,omitempty"`

	// Opaque metadata passed through to the response.
	Metadata map[string]string `json:"metadata,omitempty"`
}

// ChatMessage represents a single chat message.
type ChatMessage struct {
	Role    string `json:"role"`
	Content string `json:"content"`
}

// CapabilityRequirements defines hard constraints on model capabilities.
type CapabilityRequirements struct {
	MinContextWindow *uint32 `json:"min_context_window,omitempty"`
	Vision           *bool   `json:"vision,omitempty"`
	FunctionCalling  *bool   `json:"function_calling,omitempty"`
	JSONMode         *bool   `json:"json_mode,omitempty"`
	Reasoning        *bool   `json:"reasoning,omitempty"`
}

// StrategyParams overrides for routing strategy weights and constraints.
type StrategyParams struct {
	CostWeight    *float32 `json:"cost_weight,omitempty"`
	QualityWeight *float32 `json:"quality_weight,omitempty"`
	LatencyWeight *float32 `json:"latency_weight,omitempty"`

	MaxLatencyMs    *uint32  `json:"max_latency_ms,omitempty"`
	MaxTTFTMs       *uint32  `json:"max_ttft_ms,omitempty"`
	MinQualityScore *float32 `json:"min_quality_score,omitempty"`

	BudgetPerRequest *float64 `json:"budget_per_request,omitempty"`
}

// ──────────────────────────────────────────────
// Route Response
// ──────────────────────────────────────────────

// RouteResponse is the JSON equivalent of the proto RouteResponse message.
type RouteResponse struct {
	Model    string `json:"model"`
	Provider string `json:"provider"`
	Variant  string `json:"variant"`
	Account  string `json:"account"`

	// Connection details for the gateway to forward the request.
	Upstream *Upstream `json:"upstream,omitempty"`

	// Ordered fallback chain (try next on failure).
	FallbackChain []Upstream `json:"fallback_chain,omitempty"`

	// Observability.
	Reason         *RoutingReason `json:"reason,omitempty"`
	Score          float32        `json:"score"`
	DecisionTimeUs int64          `json:"decision_time_us"`

	// Extra metadata to pass back as response headers.
	ResponseMetadata map[string]string `json:"response_metadata,omitempty"`
}

// Upstream carries everything a gateway needs to forward a request.
type Upstream struct {
	BaseURL   string            `json:"base_url"`
	ModelName string            `json:"model_name"`
	Headers   map[string]string `json:"headers,omitempty"`
	Path      string            `json:"path"`
	TimeoutMs int32             `json:"timeout_ms,omitempty"`
	VariantID string            `json:"variant_id"`
}

// RoutingReason provides observability into the routing decision.
type RoutingReason struct {
	L1Model              string   `json:"l1_model"`
	L2Provider           string   `json:"l2_provider"`
	L3Account            string   `json:"l3_account"`
	L4Variant            string   `json:"l4_variant"`
	ConstraintsApplied   []string `json:"constraints_applied,omitempty"`
	CandidatesFiltered   uint32   `json:"candidates_filtered"`
}

// ──────────────────────────────────────────────
// Outcome Report (feedback loop)
// ──────────────────────────────────────────────

// OutcomeReport reports the result of a routed request for real-time tracking.
type OutcomeReport struct {
	RequestID         string  `json:"request_id"`
	VariantID         string  `json:"variant_id"`
	StatusCode        int32   `json:"status_code"`
	LatencyMs         int64   `json:"latency_ms"`
	TTFTMs            int64   `json:"ttft_ms,omitempty"`
	InputTokens       int32   `json:"input_tokens"`
	OutputTokens      int32   `json:"output_tokens"`
	Error             *string `json:"error,omitempty"`
	UsedFallback      bool    `json:"used_fallback"`
	FallbackVariantID *string `json:"fallback_variant_id,omitempty"`
}

// ReportResponse is an empty response for outcome reports.
type ReportResponse struct{}

// ──────────────────────────────────────────────
// Batch & Health
// ──────────────────────────────────────────────

// RouteBatchRequest wraps multiple route requests.
type RouteBatchRequest struct {
	Requests []RouteRequest `json:"requests"`
}

// RouteBatchResponse wraps multiple route responses.
type RouteBatchResponse struct {
	Responses []RouteResponse `json:"responses"`
}

// HealthCheckResponse reports the router's health status.
type HealthCheckResponse struct {
	Status           string `json:"status"`
	UptimeSeconds    int64  `json:"uptime_seconds"`
	RegisteredModels uint32 `json:"registered_models"`
	ActiveVariants   uint32 `json:"active_variants"`
}
