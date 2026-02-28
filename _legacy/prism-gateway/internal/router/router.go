// Package router defines the routing interface that bridges Go with the Rust core.
package router

import (
	"go.zoe.im/prism-gateway/pkg/api"
)

// RouteRequest is the input to the router.
type RouteRequest struct {
	Messages    []api.Message
	Strategy    string            // "auto", "balanced", "cost_first", "slo_first", "quality_first", "pinned:MODEL_ID"
	PoolIDs     []string
	AffinityKey string
	TenantID    string
	Region      string
	Headers     map[string]string // raw Prism headers

	// Strategy parameters
	CostWeight    *float32
	QualityWeight *float32
	LatencyWeight *float32
	MaxLatencyMs  *uint32
	MaxTTFTMs     *uint32

	// AnalyzeContext fields
	CallerType string
	Source     string
	Tags       map[string]string

	// Protocol filtering: "openai", "anthropic", or "" (no filter)
	RequiredProtocol string
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

// Router decides which model/provider/variant to use for a request.
type Router interface {
	// Route performs a routing decision.
	Route(req *RouteRequest) (*RouteResult, error)
}

// StaticRouter is a simple router that always returns a fixed model (for testing).
type StaticRouter struct {
	ModelID    string
	ProviderID string
	VariantID  string
	AccountID  string
}

func (r *StaticRouter) Route(req *RouteRequest) (*RouteResult, error) {
	return &RouteResult{
		Model:          r.ModelID,
		Provider:       r.ProviderID,
		Variant:        r.VariantID,
		Account:        r.AccountID,
		FallbackChain:  nil,
		Reason:         "static",
		Score:          1.0,
		DecisionTimeUs: 0,
	}, nil
}
