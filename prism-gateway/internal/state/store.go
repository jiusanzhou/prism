// Package state provides pluggable state backends for variant performance tracking.
package state

import (
	"context"
	"time"
)

// StateStore interface for pluggable state backends.
type StateStore interface {
	// UpdateVariantMetrics updates real-time metrics for a variant.
	UpdateVariantMetrics(ctx context.Context, variantID string, metrics *VariantMetrics) error
	// GetVariantMetrics retrieves current metrics for a variant.
	GetVariantMetrics(ctx context.Context, variantID string) (*VariantMetrics, error)
	// IncrementQPS atomically increments the QPS counter for a variant.
	IncrementQPS(ctx context.Context, variantID string) error
	// RecordLatency records a latency sample for a variant.
	RecordLatency(ctx context.Context, variantID string, latencyMs int64) error
	// RecordError records an error for a variant.
	RecordError(ctx context.Context, variantID string) error
	// GetAllMetrics returns metrics for all variants.
	GetAllMetrics(ctx context.Context) (map[string]*VariantMetrics, error)
	// Close closes the store.
	Close() error
}

// VariantMetrics holds real-time performance metrics for a variant.
type VariantMetrics struct {
	CurrentQPS  float32   `json:"current_qps"`
	ErrorRate   float32   `json:"error_rate"`
	P50Latency  uint32    `json:"p50_latency_ms"`
	P95Latency  uint32    `json:"p95_latency_ms"`
	P99Latency  uint32    `json:"p99_latency_ms"`
	TTFTP50     uint32    `json:"ttft_p50_ms"`
	QueueDepth  uint32    `json:"queue_depth"`
	LastUpdated time.Time `json:"last_updated"`
}
