// Package dashboard provides aggregated data for the admin dashboard.
package dashboard

import (
	"math/rand"
	"time"

	"go.zoe.im/prism-admin/internal/config"
)

// Service aggregates dashboard data from the prism config and (future) metrics sources.
type Service struct {
	cfg *config.PrismConfig
}

// NewService creates a new dashboard service.
func NewService(cfg *config.PrismConfig) *Service {
	return &Service{cfg: cfg}
}

// Overview contains high-level system stats.
type Overview struct {
	TotalRequests      int64   `json:"total_requests"`
	AvgLatencyMs       float64 `json:"avg_latency_ms"`
	ErrorRate          float64 `json:"error_rate"`
	TotalCostUSD       float64 `json:"total_cost_usd"`
	ActiveVariants     int     `json:"active_variants"`
	ActiveExperiments  int     `json:"active_experiments"`
	UptimeSeconds      int64   `json:"uptime_seconds"`
	RequestsPerSecond  float64 `json:"requests_per_second"`
}

// RoutingBreakdown contains routing decision stats.
type RoutingBreakdown struct {
	StrategyDistribution map[string]int     `json:"strategy_distribution"`
	ModelSelectionFreq   map[string]int     `json:"model_selection_freq"`
	PoolUsage            map[string]int     `json:"pool_usage"`
}

// ProviderStats contains per-provider metrics.
type ProviderStats struct {
	ProviderID string  `json:"provider_id"`
	QPS        float64 `json:"qps"`
	AvgLatency float64 `json:"avg_latency_ms"`
	ErrorRate  float64 `json:"error_rate"`
	TotalCost  float64 `json:"total_cost_usd"`
	Status     string  `json:"status"`
}

// VariantMetrics contains variant-level metrics.
type VariantMetrics struct {
	VariantID   string  `json:"variant_id"`
	Requests    int64   `json:"requests"`
	AvgLatency  float64 `json:"avg_latency_ms"`
	P99Latency  float64 `json:"p99_latency_ms"`
	ErrorRate   float64 `json:"error_rate"`
	TotalCost   float64 `json:"total_cost_usd"`
	Status      string  `json:"status"`
}

// Experiment describes a running or completed experiment.
type Experiment struct {
	ID          string    `json:"id"`
	Name        string    `json:"name"`
	Status      string    `json:"status"`
	StartedAt   time.Time `json:"started_at"`
	VariantIDs  []string  `json:"variant_ids"`
	TrafficPct  float64   `json:"traffic_pct"`
}

// ExperimentResults contains results for a single experiment.
type ExperimentResults struct {
	ExperimentID string                  `json:"experiment_id"`
	Results      []ExperimentVariantResult `json:"results"`
}

// ExperimentVariantResult is per-variant results in an experiment.
type ExperimentVariantResult struct {
	VariantID  string  `json:"variant_id"`
	Requests   int64   `json:"requests"`
	AvgLatency float64 `json:"avg_latency_ms"`
	ErrorRate  float64 `json:"error_rate"`
	AvgCost    float64 `json:"avg_cost_usd"`
	AvgQuality float64 `json:"avg_quality"`
}

// GetOverview returns aggregated overview stats.
// Currently returns mock data; will wire to real sources later.
func (s *Service) GetOverview() *Overview {
	return &Overview{
		TotalRequests:     12847,
		AvgLatencyMs:      342.5,
		ErrorRate:         0.012,
		TotalCostUSD:      18.73,
		ActiveVariants:    len(s.cfg.Variants),
		ActiveExperiments: 0,
		UptimeSeconds:     86400,
		RequestsPerSecond: 2.4,
	}
}

// GetRoutingBreakdown returns routing decision stats.
func (s *Service) GetRoutingBreakdown() *RoutingBreakdown {
	modelFreq := make(map[string]int)
	for _, v := range s.cfg.Variants {
		modelFreq[v.ModelID] = 100 + rand.Intn(500)
	}

	poolUsage := make(map[string]int)
	for _, p := range s.cfg.Pools {
		poolUsage[p.ID] = 200 + rand.Intn(800)
	}

	return &RoutingBreakdown{
		StrategyDistribution: map[string]int{
			"balanced":    780,
			"cost_first":  120,
			"quality_first": 100,
		},
		ModelSelectionFreq: modelFreq,
		PoolUsage:          poolUsage,
	}
}

// GetProviderStats returns per-provider metrics.
func (s *Service) GetProviderStats() []ProviderStats {
	stats := make([]ProviderStats, 0, len(s.cfg.Providers))
	for id := range s.cfg.Providers {
		stats = append(stats, ProviderStats{
			ProviderID: id,
			QPS:        1.2 + rand.Float64()*3,
			AvgLatency: 200 + rand.Float64()*400,
			ErrorRate:  rand.Float64() * 0.05,
			TotalCost:  5 + rand.Float64()*20,
			Status:     "healthy",
		})
	}
	return stats
}

// GetVariantMetrics returns metrics for all variants.
func (s *Service) GetVariantMetrics() []VariantMetrics {
	metrics := make([]VariantMetrics, 0, len(s.cfg.Variants))
	for _, v := range s.cfg.Variants {
		metrics = append(metrics, VariantMetrics{
			VariantID:  v.ID,
			Requests:   int64(1000 + rand.Intn(5000)),
			AvgLatency: 200 + rand.Float64()*400,
			P99Latency: 800 + rand.Float64()*1200,
			ErrorRate:  rand.Float64() * 0.05,
			TotalCost:  2 + rand.Float64()*15,
			Status:     "active",
		})
	}
	return metrics
}

// GetVariantMetricsByID returns metrics for a specific variant.
func (s *Service) GetVariantMetricsByID(id string) *VariantMetrics {
	for _, v := range s.cfg.Variants {
		if v.ID == id {
			return &VariantMetrics{
				VariantID:  v.ID,
				Requests:   int64(1000 + rand.Intn(5000)),
				AvgLatency: 200 + rand.Float64()*400,
				P99Latency: 800 + rand.Float64()*1200,
				ErrorRate:  rand.Float64() * 0.05,
				TotalCost:  2 + rand.Float64()*15,
				Status:     "active",
			}
		}
	}
	return nil
}

// GetExperiments returns the list of experiments.
// Currently returns mock data.
func (s *Service) GetExperiments() []Experiment {
	// No experiments by default; return empty list.
	return []Experiment{}
}

// GetExperimentResults returns results for a specific experiment.
func (s *Service) GetExperimentResults(id string) *ExperimentResults {
	return nil
}
