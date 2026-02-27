// Package ffi defines the interface for the Rust prism-core routing engine.
package ffi

// Message represents a single chat message.
type Message struct {
	Role    string `json:"role"`
	Content string `json:"content"`
}

// RouteRequest is the input to the router.
type RouteRequest struct {
	Messages    []Message
	Strategy    string   // "auto", "balanced", "cost_first", "slo_first", "quality_first", "pinned:MODEL_ID"
	PoolIDs     []string
	AffinityKey string
	TenantID    string
	Region      string

	// Strategy parameters
	CostWeight    *float32
	QualityWeight *float32
	LatencyWeight *float32
	MaxLatencyMs  *uint32
	MaxTTFTMs     *uint32
}

// RouteResult is the output of a routing decision.
type RouteResult struct {
	Model          string
	Provider       string
	Variant        string
	Account        string
	FallbackChain  []string
	Reason         string
	Score          float32
	DecisionTimeUs int64
}

// HealthStatus holds health information from the router.
type HealthStatus struct {
	Status           string
	RegisteredModels uint32
	ActiveVariants   uint32
}

// Router is the interface wrapping the Rust FFI core or a static fallback.
type Router struct {
	impl routerImpl
}

type routerImpl interface {
	Route(req *RouteRequest) (*RouteResult, error)
	UpdateStats(statsJSON string) error
	Health() *HealthStatus
	Version() string
}
