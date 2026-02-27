package state

import (
	"context"
	"testing"
	"time"
)

func TestMemoryStoreIncrementQPS(t *testing.T) {
	store := NewMemoryStore()
	ctx := context.Background()

	// Increment QPS multiple times
	for i := 0; i < 10; i++ {
		if err := store.IncrementQPS(ctx, "variant-1"); err != nil {
			t.Fatalf("IncrementQPS failed: %v", err)
		}
	}

	metrics, err := store.GetVariantMetrics(ctx, "variant-1")
	if err != nil {
		t.Fatalf("GetVariantMetrics failed: %v", err)
	}

	// 10 requests in qpsWindow (10s) = 1.0 QPS
	if metrics.CurrentQPS < 0.5 || metrics.CurrentQPS > 2.0 {
		t.Errorf("Expected QPS ~1.0, got %f", metrics.CurrentQPS)
	}
}

func TestMemoryStoreRecordLatency(t *testing.T) {
	store := NewMemoryStore()
	ctx := context.Background()

	// Record 100 samples: 1ms, 2ms, ..., 100ms
	for i := int64(1); i <= 100; i++ {
		if err := store.RecordLatency(ctx, "variant-1", i); err != nil {
			t.Fatalf("RecordLatency failed: %v", err)
		}
	}

	metrics, err := store.GetVariantMetrics(ctx, "variant-1")
	if err != nil {
		t.Fatalf("GetVariantMetrics failed: %v", err)
	}

	// P50 should be around 50ms
	if metrics.P50Latency < 45 || metrics.P50Latency > 55 {
		t.Errorf("Expected P50 ~50ms, got %d", metrics.P50Latency)
	}

	// P95 should be around 95ms
	if metrics.P95Latency < 90 || metrics.P95Latency > 100 {
		t.Errorf("Expected P95 ~95ms, got %d", metrics.P95Latency)
	}

	// P99 should be around 99ms
	if metrics.P99Latency < 95 || metrics.P99Latency > 100 {
		t.Errorf("Expected P99 ~99ms, got %d", metrics.P99Latency)
	}
}

func TestMemoryStoreRecordError(t *testing.T) {
	store := NewMemoryStore()
	ctx := context.Background()

	// Record 10 requests total
	for i := 0; i < 10; i++ {
		if err := store.IncrementQPS(ctx, "variant-1"); err != nil {
			t.Fatalf("IncrementQPS failed: %v", err)
		}
	}

	// Record 3 errors
	for i := 0; i < 3; i++ {
		if err := store.RecordError(ctx, "variant-1"); err != nil {
			t.Fatalf("RecordError failed: %v", err)
		}
	}

	metrics, err := store.GetVariantMetrics(ctx, "variant-1")
	if err != nil {
		t.Fatalf("GetVariantMetrics failed: %v", err)
	}

	// Error rate should be ~30%
	if metrics.ErrorRate < 0.2 || metrics.ErrorRate > 0.4 {
		t.Errorf("Expected error rate ~0.3, got %f", metrics.ErrorRate)
	}
}

func TestMemoryStoreGetAllMetrics(t *testing.T) {
	store := NewMemoryStore()
	ctx := context.Background()

	// Create data for multiple variants
	if err := store.IncrementQPS(ctx, "variant-1"); err != nil {
		t.Fatal(err)
	}
	if err := store.RecordLatency(ctx, "variant-1", 100); err != nil {
		t.Fatal(err)
	}

	if err := store.IncrementQPS(ctx, "variant-2"); err != nil {
		t.Fatal(err)
	}
	if err := store.RecordLatency(ctx, "variant-2", 200); err != nil {
		t.Fatal(err)
	}

	if err := store.IncrementQPS(ctx, "variant-3"); err != nil {
		t.Fatal(err)
	}

	allMetrics, err := store.GetAllMetrics(ctx)
	if err != nil {
		t.Fatalf("GetAllMetrics failed: %v", err)
	}

	if len(allMetrics) != 3 {
		t.Errorf("Expected 3 variants, got %d", len(allMetrics))
	}

	if _, ok := allMetrics["variant-1"]; !ok {
		t.Error("variant-1 not found in all metrics")
	}
	if _, ok := allMetrics["variant-2"]; !ok {
		t.Error("variant-2 not found in all metrics")
	}
	if _, ok := allMetrics["variant-3"]; !ok {
		t.Error("variant-3 not found in all metrics")
	}

	// Verify variant-1 latency
	if allMetrics["variant-1"].P50Latency != 100 {
		t.Errorf("Expected variant-1 P50=100, got %d", allMetrics["variant-1"].P50Latency)
	}
	// Verify variant-2 latency
	if allMetrics["variant-2"].P50Latency != 200 {
		t.Errorf("Expected variant-2 P50=200, got %d", allMetrics["variant-2"].P50Latency)
	}
}

func TestMemoryStoreLastUpdated(t *testing.T) {
	store := NewMemoryStore()
	ctx := context.Background()

	before := time.Now()
	_ = store.IncrementQPS(ctx, "variant-1")

	metrics, err := store.GetVariantMetrics(ctx, "variant-1")
	if err != nil {
		t.Fatal(err)
	}

	if metrics.LastUpdated.Before(before) {
		t.Error("LastUpdated should be after our reference time")
	}
}

func TestMemoryStoreClose(t *testing.T) {
	store := NewMemoryStore()
	if err := store.Close(); err != nil {
		t.Errorf("Close should return nil, got %v", err)
	}
}
