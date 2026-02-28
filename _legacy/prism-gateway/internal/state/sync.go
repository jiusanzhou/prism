package state

import (
	"context"
	"encoding/json"
	"log"
	"time"
)

// StatsUpdater is an interface for pushing stats to the routing engine.
type StatsUpdater interface {
	UpdateStats(statsJSON string) error
}

// SyncLoop periodically reads all variant metrics from the StateStore and
// pushes them to the routing engine via StatsUpdater.
// It runs until the context is cancelled.
func SyncLoop(ctx context.Context, store StateStore, updater StatsUpdater, interval time.Duration) {
	if updater == nil {
		return
	}

	ticker := time.NewTicker(interval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			syncMetrics(ctx, store, updater)
		}
	}
}

func syncMetrics(ctx context.Context, store StateStore, updater StatsUpdater) {
	allMetrics, err := store.GetAllMetrics(ctx)
	if err != nil {
		log.Printf("state: sync failed to get metrics: %v", err)
		return
	}

	if len(allMetrics) == 0 {
		return
	}

	// Build the stats JSON for the Rust core.
	// Format: array of variant stats objects.
	type variantStats struct {
		VariantID  string  `json:"variant_id"`
		CurrentQPS float32 `json:"current_qps"`
		ErrorRate  float32 `json:"error_rate"`
		P50Latency uint32  `json:"p50_latency_ms"`
		P95Latency uint32  `json:"p95_latency_ms"`
		P99Latency uint32  `json:"p99_latency_ms"`
		TTFTP50    uint32  `json:"ttft_p50_ms"`
		QueueDepth uint32  `json:"queue_depth"`
	}

	stats := make([]variantStats, 0, len(allMetrics))
	for variantID, m := range allMetrics {
		stats = append(stats, variantStats{
			VariantID:  variantID,
			CurrentQPS: m.CurrentQPS,
			ErrorRate:  m.ErrorRate,
			P50Latency: m.P50Latency,
			P95Latency: m.P95Latency,
			P99Latency: m.P99Latency,
			TTFTP50:    m.TTFTP50,
			QueueDepth: m.QueueDepth,
		})
	}

	data, err := json.Marshal(stats)
	if err != nil {
		log.Printf("state: sync failed to marshal stats: %v", err)
		return
	}

	if err := updater.UpdateStats(string(data)); err != nil {
		log.Printf("state: sync failed to update stats: %v", err)
	}
}
