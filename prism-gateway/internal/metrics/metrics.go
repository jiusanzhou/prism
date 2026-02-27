// Package metrics defines Prometheus metrics for the Prism gateway.
package metrics

import "github.com/prometheus/client_golang/prometheus"

var (
	// Routing decision metrics
	RoutingDecisionDuration = prometheus.NewHistogramVec(
		prometheus.HistogramOpts{
			Name:    "prism_routing_decision_duration_seconds",
			Help:    "Time taken to make a routing decision",
			Buckets: []float64{0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05},
		},
		[]string{"strategy"},
	)

	RoutingDecisionTotal = prometheus.NewCounterVec(
		prometheus.CounterOpts{
			Name: "prism_routing_decisions_total",
			Help: "Total number of routing decisions",
		},
		[]string{"strategy", "model", "provider", "status"},
	)

	// Provider request metrics
	ProviderRequestDuration = prometheus.NewHistogramVec(
		prometheus.HistogramOpts{
			Name:    "prism_provider_request_duration_seconds",
			Help:    "Time taken for provider requests",
			Buckets: prometheus.DefBuckets,
		},
		[]string{"provider", "model", "stream"},
	)

	ProviderRequestTotal = prometheus.NewCounterVec(
		prometheus.CounterOpts{
			Name: "prism_provider_requests_total",
			Help: "Total provider requests",
		},
		[]string{"provider", "model", "status_code"},
	)

	ProviderErrorTotal = prometheus.NewCounterVec(
		prometheus.CounterOpts{
			Name: "prism_provider_errors_total",
			Help: "Total provider errors",
		},
		[]string{"provider", "model", "error_type"},
	)

	// Token usage
	TokensUsed = prometheus.NewCounterVec(
		prometheus.CounterOpts{
			Name: "prism_tokens_total",
			Help: "Total tokens used",
		},
		[]string{"provider", "model", "type"}, // type: "prompt" or "completion"
	)

	// Active requests gauge
	ActiveRequests = prometheus.NewGaugeVec(
		prometheus.GaugeOpts{
			Name: "prism_active_requests",
			Help: "Currently active requests per provider",
		},
		[]string{"provider"},
	)

	// TTFT (time to first token)
	TTFTDuration = prometheus.NewHistogramVec(
		prometheus.HistogramOpts{
			Name:    "prism_ttft_seconds",
			Help:    "Time to first token for streaming requests",
			Buckets: []float64{0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0},
		},
		[]string{"provider", "model"},
	)
)

// Register registers all Prism metrics with the default Prometheus registry.
func Register() {
	prometheus.MustRegister(
		RoutingDecisionDuration,
		RoutingDecisionTotal,
		ProviderRequestDuration,
		ProviderRequestTotal,
		ProviderErrorTotal,
		TokensUsed,
		ActiveRequests,
		TTFTDuration,
	)
}
