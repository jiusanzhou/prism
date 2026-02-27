package metrics

import (
	"testing"

	dto "github.com/prometheus/client_model/go"
	"github.com/prometheus/client_golang/prometheus"
)

func TestMetricsRegistration(t *testing.T) {
	// Create a new registry to avoid conflicts with other tests
	reg := prometheus.NewRegistry()

	collectors := []prometheus.Collector{
		RoutingDecisionDuration,
		RoutingDecisionTotal,
		ProviderRequestDuration,
		ProviderRequestTotal,
		ProviderErrorTotal,
		TokensUsed,
		ActiveRequests,
		TTFTDuration,
	}

	for _, c := range collectors {
		if err := reg.Register(c); err != nil {
			t.Errorf("Failed to register collector: %v", err)
		}
	}

	// Verify all registered correctly — gather should succeed
	_, err := reg.Gather()
	if err != nil {
		t.Fatalf("Failed to gather metrics: %v", err)
	}
}

func TestRoutingDecisionMetrics(t *testing.T) {
	// Record routing decision duration
	RoutingDecisionDuration.WithLabelValues("balanced").Observe(0.001)
	RoutingDecisionDuration.WithLabelValues("balanced").Observe(0.002)
	RoutingDecisionDuration.WithLabelValues("cost_first").Observe(0.0005)

	// Record routing decision totals
	RoutingDecisionTotal.WithLabelValues("balanced", "gpt-4o", "openai", "ok").Inc()
	RoutingDecisionTotal.WithLabelValues("balanced", "gpt-4o", "openai", "ok").Inc()
	RoutingDecisionTotal.WithLabelValues("cost_first", "gpt-3.5", "openai", "ok").Inc()
	RoutingDecisionTotal.WithLabelValues("balanced", "gpt-4o", "openai", "error").Inc()

	// Verify counter values
	reg := prometheus.NewRegistry()
	reg.MustRegister(RoutingDecisionTotal)

	mfs, err := reg.Gather()
	if err != nil {
		t.Fatalf("Failed to gather metrics: %v", err)
	}

	var found bool
	for _, mf := range mfs {
		if mf.GetName() == "prism_routing_decisions_total" {
			found = true
			if len(mf.GetMetric()) < 1 {
				t.Error("Expected at least 1 metric series")
			}
			// Verify one of the counters has value 2 (balanced/ok was incremented twice)
			for _, m := range mf.GetMetric() {
				strategy := findLabel(m.GetLabel(), "strategy")
				status := findLabel(m.GetLabel(), "status")
				if strategy == "balanced" && status == "ok" {
					if m.GetCounter().GetValue() != 2 {
						t.Errorf("Expected balanced/ok counter = 2, got %f", m.GetCounter().GetValue())
					}
				}
			}
		}
	}

	if !found {
		t.Error("prism_routing_decisions_total metric not found")
	}

	reg.Unregister(RoutingDecisionTotal)
}

func findLabel(labels []*dto.LabelPair, name string) string {
	for _, l := range labels {
		if l.GetName() == name {
			return l.GetValue()
		}
	}
	return ""
}
