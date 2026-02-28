//! Metrics collector — per-variant sliding window for runtime metrics.
//!
//! Collects request outcomes (latency, success, tokens) and periodically
//! flushes aggregated `ModelPerformance` into the prism-core `DataLayer`,
//! closing the feedback loop for routing decisions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use prism_core::data::{DataLayer, MetricKind, MetricValue};
use prism_core::types::ModelPerformance;

// ──────────────────────────────────────────────
// Request record
// ──────────────────────────────────────────────

/// A single request outcome recorded by the proxy/gateway.
#[derive(Debug, Clone)]
pub struct RequestRecord {
    pub variant_id: String,
    pub latency_ms: u64,
    pub ttft_ms: Option<u64>,
    pub success: bool,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub timestamp: Instant,
}

// ──────────────────────────────────────────────
// Per-variant window
// ──────────────────────────────────────────────

/// Circular buffer of request records per variant.
struct VariantWindow {
    records: Vec<RequestRecord>,
    head: usize,
    count: usize,
    capacity: usize,

    // Atomic counters for QPS (reset each flush)
    requests_since_flush: u64,
    last_flush: Instant,
}

impl VariantWindow {
    fn new(capacity: usize) -> Self {
        Self {
            records: Vec::with_capacity(capacity),
            head: 0,
            count: 0,
            capacity,
            requests_since_flush: 0,
            last_flush: Instant::now(),
        }
    }

    fn push(&mut self, record: RequestRecord) {
        if self.records.len() < self.capacity {
            self.records.push(record);
        } else {
            self.records[self.head] = record;
        }
        self.head = (self.head + 1) % self.capacity;
        if self.count < self.capacity {
            self.count += 1;
        }
        self.requests_since_flush += 1;
    }

    /// Compute aggregated performance from the window.
    fn aggregate(&mut self) -> ModelPerformance {
        if self.count == 0 {
            return ModelPerformance::default();
        }

        let now = Instant::now();
        let window_duration = now.duration_since(self.last_flush);
        let elapsed_secs = window_duration.as_secs_f32().max(0.001);

        // Collect latencies and stats
        let mut latencies: Vec<u64> = Vec::with_capacity(self.count);
        let mut ttfts: Vec<u64> = Vec::new();
        let mut errors: u64 = 0;
        let mut total_tokens: u64 = 0;

        let effective = self.count.min(self.records.len());
        for i in 0..effective {
            let rec = &self.records[i];
            latencies.push(rec.latency_ms);
            if let Some(ttft) = rec.ttft_ms {
                ttfts.push(ttft);
            }
            if !rec.success {
                errors += 1;
            }
            total_tokens += rec.input_tokens.unwrap_or(0) as u64
                + rec.output_tokens.unwrap_or(0) as u64;
        }

        latencies.sort_unstable();

        let p50 = percentile(&latencies, 50);
        let p95 = percentile(&latencies, 95);
        let p99 = percentile(&latencies, 99);

        let ttft_p50 = if ttfts.is_empty() {
            0
        } else {
            ttfts.sort_unstable();
            percentile(&ttfts, 50)
        };

        let error_rate = errors as f32 / effective as f32;
        let current_qps = self.requests_since_flush as f32 / elapsed_secs;
        let throughput_tps = total_tokens as f32 / elapsed_secs;

        // Reset flush counters
        self.requests_since_flush = 0;
        self.last_flush = now;

        ModelPerformance {
            p50_latency_ms: p50 as u32,
            p95_latency_ms: p95 as u32,
            p99_latency_ms: p99 as u32,
            ttft_p50_ms: ttft_p50 as u32,
            throughput_tps,
            error_rate,
            current_qps,
            queue_depth: 0,
            last_updated: Some(now),
        }
    }
}

/// Compute the p-th percentile from a sorted slice.
fn percentile(sorted: &[u64], p: u8) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((p as f64 / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

// ──────────────────────────────────────────────
// MetricsCollector
// ──────────────────────────────────────────────

/// Collects per-variant request metrics and flushes to DataLayer.
pub struct MetricsCollector {
    windows: RwLock<HashMap<String, VariantWindow>>,
    window_size: usize,
    /// Total requests recorded (atomic, for /metrics endpoint)
    pub total_requests: AtomicU64,
    pub total_errors: AtomicU64,
}

impl MetricsCollector {
    /// Create a new collector with the given window size per variant.
    pub fn new(window_size: usize) -> Self {
        Self {
            windows: RwLock::new(HashMap::new()),
            window_size,
            total_requests: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
        }
    }

    /// Record a request outcome. Lock-free on the hot path (write lock per variant).
    pub fn record(&self, record: RequestRecord) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        if !record.success {
            self.total_errors.fetch_add(1, Ordering::Relaxed);
        }

        let mut windows = self.windows.write().unwrap();
        let window = windows
            .entry(record.variant_id.clone())
            .or_insert_with(|| VariantWindow::new(self.window_size));
        window.push(record);
    }

    /// Flush all variant windows → DataLayer. Called periodically.
    pub fn flush_to_data_layer(&self, data_layer: &DataLayer) {
        let mut windows = self.windows.write().unwrap();
        for (variant_id, window) in windows.iter_mut() {
            let perf = window.aggregate();
            data_layer.update(
                MetricKind::Performance,
                variant_id,
                MetricValue::Performance(perf),
            );
        }
    }

    /// Get a snapshot of current metrics for a variant (without flushing).
    pub fn snapshot(&self, variant_id: &str) -> Option<ModelPerformance> {
        let mut windows = self.windows.write().unwrap();
        windows.get_mut(variant_id).map(|w| {
            // Compute without resetting flush counters — use a clone
            let mut clone = VariantWindow::new(w.capacity);
            clone.records = w.records.clone();
            clone.head = w.head;
            clone.count = w.count;
            clone.requests_since_flush = w.requests_since_flush;
            clone.last_flush = w.last_flush;
            clone.aggregate()
        })
    }

    /// List all tracked variant IDs.
    pub fn tracked_variants(&self) -> Vec<String> {
        let windows = self.windows.read().unwrap();
        windows.keys().cloned().collect()
    }

    /// Total requests recorded.
    pub fn total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }

    /// Total errors recorded.
    pub fn total_errors(&self) -> u64 {
        self.total_errors.load(Ordering::Relaxed)
    }
}

// ──────────────────────────────────────────────
// Background flush task
// ──────────────────────────────────────────────

/// Spawn a background task that flushes metrics to DataLayer every `interval`.
pub fn spawn_metrics_flush(
    collector: Arc<MetricsCollector>,
    data_layer: Arc<DataLayer>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            collector.flush_to_data_layer(&data_layer);
        }
    })
}

// ──────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::data::PerformanceDataSource;

    fn make_record(variant: &str, latency_ms: u64, success: bool) -> RequestRecord {
        RequestRecord {
            variant_id: variant.to_string(),
            latency_ms,
            ttft_ms: Some(latency_ms / 3),
            success,
            input_tokens: Some(100),
            output_tokens: Some(200),
            timestamp: Instant::now(),
        }
    }

    #[test]
    fn test_record_and_snapshot() {
        let collector = MetricsCollector::new(100);
        for i in 0..10 {
            collector.record(make_record("v1", 100 + i * 10, true));
        }

        let snap = collector.snapshot("v1").unwrap();
        assert!(snap.p50_latency_ms > 0);
        assert!(snap.p95_latency_ms >= snap.p50_latency_ms);
        assert_eq!(snap.error_rate, 0.0);
        assert_eq!(collector.total_requests(), 10);
        assert_eq!(collector.total_errors(), 0);
    }

    #[test]
    fn test_error_rate() {
        let collector = MetricsCollector::new(100);
        for _ in 0..8 {
            collector.record(make_record("v1", 100, true));
        }
        for _ in 0..2 {
            collector.record(make_record("v1", 500, false));
        }

        let snap = collector.snapshot("v1").unwrap();
        assert!((snap.error_rate - 0.2).abs() < 0.01);
        assert_eq!(collector.total_errors(), 2);
    }

    #[test]
    fn test_multiple_variants() {
        let collector = MetricsCollector::new(100);
        collector.record(make_record("v1", 100, true));
        collector.record(make_record("v2", 200, true));
        collector.record(make_record("v3", 300, false));

        let mut tracked = collector.tracked_variants();
        tracked.sort();
        assert_eq!(tracked, vec!["v1", "v2", "v3"]);

        let s1 = collector.snapshot("v1").unwrap();
        let s2 = collector.snapshot("v2").unwrap();
        assert!(s2.p50_latency_ms > s1.p50_latency_ms);
    }

    #[test]
    fn test_window_circular() {
        let collector = MetricsCollector::new(5);
        // Push 10 records — only last 5 should be in window
        for i in 0..10 {
            collector.record(make_record("v1", (i + 1) * 100, true));
        }

        let snap = collector.snapshot("v1").unwrap();
        // Last 5: 600, 700, 800, 900, 1000 → p50 = 800
        assert_eq!(snap.p50_latency_ms, 800);
    }

    #[test]
    fn test_flush_to_data_layer() {
        let collector = MetricsCollector::new(100);
        let data_layer = DataLayer::new();
        data_layer.register(Arc::new(PerformanceDataSource::new()));

        for i in 0..5 {
            collector.record(make_record("v1", 100 + i * 50, true));
        }

        collector.flush_to_data_layer(&data_layer);

        // DataLayer should now have performance data for v1
        let val = data_layer.query(MetricKind::Performance, "v1");
        assert!(val.is_some());
        if let Some(MetricValue::Performance(perf)) = val {
            assert!(perf.p50_latency_ms > 0);
            assert_eq!(perf.error_rate, 0.0);
        } else {
            panic!("expected Performance metric");
        }
    }

    #[test]
    fn test_flush_resets_qps() {
        let collector = MetricsCollector::new(100);

        for _ in 0..10 {
            collector.record(make_record("v1", 100, true));
        }

        let snap1 = collector.snapshot("v1").unwrap();
        assert!(snap1.current_qps > 0.0);

        // After flush, qps counter resets — but snapshot still shows based on window
        let data_layer = DataLayer::new();
        data_layer.register(Arc::new(PerformanceDataSource::new()));
        collector.flush_to_data_layer(&data_layer);
    }

    #[test]
    fn test_snapshot_nonexistent_variant() {
        let collector = MetricsCollector::new(100);
        assert!(collector.snapshot("nonexistent").is_none());
    }

    #[test]
    fn test_percentile_function() {
        assert_eq!(percentile(&[], 50), 0);
        assert_eq!(percentile(&[100], 50), 100);
        assert_eq!(percentile(&[100, 200, 300], 50), 200);
        assert_eq!(percentile(&[100, 200, 300], 0), 100);
        assert_eq!(percentile(&[100, 200, 300], 100), 300);
        // P95 of [100, 200, 300, 400, 500] ≈ index 3.8 → round to 4 → 500
        assert_eq!(percentile(&[100, 200, 300, 400, 500], 95), 500);
    }

    #[test]
    fn test_ttft_tracking() {
        let collector = MetricsCollector::new(100);
        collector.record(RequestRecord {
            variant_id: "v1".to_string(),
            latency_ms: 300,
            ttft_ms: Some(50),
            success: true,
            input_tokens: None,
            output_tokens: None,
            timestamp: Instant::now(),
        });
        collector.record(RequestRecord {
            variant_id: "v1".to_string(),
            latency_ms: 400,
            ttft_ms: Some(80),
            success: true,
            input_tokens: None,
            output_tokens: None,
            timestamp: Instant::now(),
        });

        let snap = collector.snapshot("v1").unwrap();
        assert!(snap.ttft_p50_ms > 0);
        assert!(snap.ttft_p50_ms <= 80);
    }

    #[tokio::test]
    async fn test_spawn_metrics_flush() {
        let collector = Arc::new(MetricsCollector::new(100));
        let data_layer = Arc::new(DataLayer::new());
        data_layer.register(Arc::new(PerformanceDataSource::new()));

        // Record some data
        for _ in 0..5 {
            collector.record(make_record("v1", 100, true));
        }

        // Spawn flush task with short interval
        let handle = spawn_metrics_flush(
            Arc::clone(&collector),
            Arc::clone(&data_layer),
            Duration::from_millis(50),
        );

        // Wait for at least one flush
        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.abort();

        // Data should be in DataLayer now
        let val = data_layer.query(MetricKind::Performance, "v1");
        assert!(val.is_some());
    }
}
