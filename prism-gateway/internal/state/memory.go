package state

import (
	"context"
	"math"
	"sort"
	"sync"
	"time"
)

const (
	// qpsWindow is the sliding window for QPS calculation.
	qpsWindow = 10 * time.Second
	// maxLatencySamples is the maximum number of latency samples to keep.
	maxLatencySamples = 1000
	// errorWindow is the sliding window for error rate calculation.
	errorWindow = 60 * time.Second
)

// variantData holds in-memory tracking data for a single variant.
type variantData struct {
	mu sync.Mutex

	// QPS: timestamps of recent requests
	requestTimes []time.Time

	// Latency: recent samples in milliseconds
	latencySamples []int64

	// Errors: timestamps of recent errors
	errorTimes []time.Time

	// Total request count (within error window) for error rate calculation
	totalRequests int64
}

// MemoryStore implements StateStore using in-memory data structures.
// This is the default (zero-config) fallback when Redis is not configured.
type MemoryStore struct {
	variants sync.Map // map[string]*variantData
}

// NewMemoryStore creates a new in-memory state store.
func NewMemoryStore() *MemoryStore {
	return &MemoryStore{}
}

func (m *MemoryStore) getOrCreate(variantID string) *variantData {
	v, _ := m.variants.LoadOrStore(variantID, &variantData{})
	return v.(*variantData)
}

func (m *MemoryStore) UpdateVariantMetrics(ctx context.Context, variantID string, metrics *VariantMetrics) error {
	// Direct update — replace computed metrics. Not commonly used for memory store
	// since metrics are computed from raw data, but useful for external sync.
	_ = m.getOrCreate(variantID)
	return nil
}

func (m *MemoryStore) GetVariantMetrics(ctx context.Context, variantID string) (*VariantMetrics, error) {
	vd := m.getOrCreate(variantID)
	vd.mu.Lock()
	defer vd.mu.Unlock()

	now := time.Now()

	return &VariantMetrics{
		CurrentQPS:  computeQPS(vd.requestTimes, now),
		ErrorRate:   computeErrorRate(vd.errorTimes, vd.requestTimes, now),
		P50Latency:  computePercentile(vd.latencySamples, 0.50),
		P95Latency:  computePercentile(vd.latencySamples, 0.95),
		P99Latency:  computePercentile(vd.latencySamples, 0.99),
		TTFTP50:     0, // Not tracked in basic memory store
		QueueDepth:  0,
		LastUpdated: now,
	}, nil
}

func (m *MemoryStore) IncrementQPS(ctx context.Context, variantID string) error {
	vd := m.getOrCreate(variantID)
	vd.mu.Lock()
	defer vd.mu.Unlock()

	now := time.Now()
	vd.requestTimes = append(vd.requestTimes, now)
	vd.totalRequests++

	// Prune old request times
	vd.requestTimes = pruneOldTimes(vd.requestTimes, now, errorWindow)

	return nil
}

func (m *MemoryStore) RecordLatency(ctx context.Context, variantID string, latencyMs int64) error {
	vd := m.getOrCreate(variantID)
	vd.mu.Lock()
	defer vd.mu.Unlock()

	vd.latencySamples = append(vd.latencySamples, latencyMs)

	// Keep only the most recent samples
	if len(vd.latencySamples) > maxLatencySamples {
		vd.latencySamples = vd.latencySamples[len(vd.latencySamples)-maxLatencySamples:]
	}

	return nil
}

func (m *MemoryStore) RecordError(ctx context.Context, variantID string) error {
	vd := m.getOrCreate(variantID)
	vd.mu.Lock()
	defer vd.mu.Unlock()

	now := time.Now()
	vd.errorTimes = append(vd.errorTimes, now)

	// Prune old error times
	vd.errorTimes = pruneOldTimes(vd.errorTimes, now, errorWindow)

	return nil
}

func (m *MemoryStore) GetAllMetrics(ctx context.Context) (map[string]*VariantMetrics, error) {
	result := make(map[string]*VariantMetrics)
	now := time.Now()

	m.variants.Range(func(key, value any) bool {
		variantID := key.(string)
		vd := value.(*variantData)
		vd.mu.Lock()
		defer vd.mu.Unlock()

		result[variantID] = &VariantMetrics{
			CurrentQPS:  computeQPS(vd.requestTimes, now),
			ErrorRate:   computeErrorRate(vd.errorTimes, vd.requestTimes, now),
			P50Latency:  computePercentile(vd.latencySamples, 0.50),
			P95Latency:  computePercentile(vd.latencySamples, 0.95),
			P99Latency:  computePercentile(vd.latencySamples, 0.99),
			TTFTP50:     0,
			QueueDepth:  0,
			LastUpdated: now,
		}
		return true
	})

	return result, nil
}

func (m *MemoryStore) Close() error {
	return nil
}

// pruneOldTimes removes timestamps older than the given window.
func pruneOldTimes(times []time.Time, now time.Time, window time.Duration) []time.Time {
	cutoff := now.Add(-window)
	idx := 0
	for _, t := range times {
		if t.After(cutoff) {
			times[idx] = t
			idx++
		}
	}
	return times[:idx]
}

// computeQPS calculates requests per second over the QPS window.
func computeQPS(requestTimes []time.Time, now time.Time) float32 {
	cutoff := now.Add(-qpsWindow)
	count := 0
	for _, t := range requestTimes {
		if t.After(cutoff) {
			count++
		}
	}
	return float32(count) / float32(qpsWindow.Seconds())
}

// computeErrorRate calculates error rate as errors / total requests in the error window.
func computeErrorRate(errorTimes, requestTimes []time.Time, now time.Time) float32 {
	cutoff := now.Add(-errorWindow)

	requestCount := 0
	for _, t := range requestTimes {
		if t.After(cutoff) {
			requestCount++
		}
	}
	if requestCount == 0 {
		return 0
	}

	errorCount := 0
	for _, t := range errorTimes {
		if t.After(cutoff) {
			errorCount++
		}
	}

	return float32(errorCount) / float32(requestCount)
}

// computePercentile calculates a percentile from a sorted copy of the samples.
func computePercentile(samples []int64, pct float64) uint32 {
	if len(samples) == 0 {
		return 0
	}

	sorted := make([]int64, len(samples))
	copy(sorted, samples)
	sort.Slice(sorted, func(i, j int) bool { return sorted[i] < sorted[j] })

	idx := int(math.Ceil(pct*float64(len(sorted)))) - 1
	if idx < 0 {
		idx = 0
	}
	if idx >= len(sorted) {
		idx = len(sorted) - 1
	}

	return uint32(sorted[idx])
}
