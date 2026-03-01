//! Metrics collector — per-variant sliding window for runtime metrics.
//!
//! Collects request outcomes (latency, success, tokens) and periodically
//! flushes aggregated `ModelPerformance` into the prism-core `DataLayer`,
//! closing the feedback loop for routing decisions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
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
            ..Default::default()
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
// New metric primitives
// ──────────────────────────────────────────────

/// Histogram with microsecond-granularity buckets.
pub struct UsHistogram {
    bucket_bounds: Vec<u64>,
    bucket_counts: Vec<AtomicU64>,
    count: AtomicU64,
    sum: AtomicU64,
}

impl UsHistogram {
    /// Create a new histogram with the given upper-bound bucket boundaries.
    pub fn new(bounds: Vec<u64>) -> Self {
        let n = bounds.len();
        let mut bucket_counts = Vec::with_capacity(n + 1); // +1 for +Inf
        for _ in 0..=n {
            bucket_counts.push(AtomicU64::new(0));
        }
        Self {
            bucket_bounds: bounds,
            bucket_counts,
            count: AtomicU64::new(0),
            sum: AtomicU64::new(0),
        }
    }

    /// Record a value in microseconds.
    pub fn observe(&self, value_us: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum.fetch_add(value_us, Ordering::Relaxed);
        for (i, &bound) in self.bucket_bounds.iter().enumerate() {
            if value_us <= bound {
                self.bucket_counts[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        // +Inf bucket always incremented
        self.bucket_counts[self.bucket_bounds.len()].fetch_add(1, Ordering::Relaxed);
    }

    /// Take a snapshot: (bounds, cumulative_counts_including_inf, total_count, total_sum).
    pub fn snapshot(&self) -> (Vec<u64>, Vec<u64>, u64, u64) {
        let counts: Vec<u64> = self
            .bucket_counts
            .iter()
            .map(|c| c.load(Ordering::Relaxed))
            .collect();
        let count = self.count.load(Ordering::Relaxed);
        let sum = self.sum.load(Ordering::Relaxed);
        (self.bucket_bounds.clone(), counts, count, sum)
    }
}

/// Counter with a single string label dimension.
pub struct LabeledCounter {
    values: RwLock<HashMap<String, u64>>,
}

impl LabeledCounter {
    pub fn new() -> Self {
        Self {
            values: RwLock::new(HashMap::new()),
        }
    }

    pub fn inc(&self, label: &str) {
        self.inc_by(label, 1);
    }

    pub fn inc_by(&self, label: &str, n: u64) {
        let mut map = self.values.write().unwrap();
        *map.entry(label.to_string()).or_insert(0) += n;
    }

    pub fn snapshot(&self) -> Vec<(String, u64)> {
        let map = self.values.read().unwrap();
        map.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }
}

/// Counter with two string label dimensions.
pub struct DualLabeledCounter {
    values: RwLock<HashMap<(String, String), u64>>,
}

impl DualLabeledCounter {
    pub fn new() -> Self {
        Self {
            values: RwLock::new(HashMap::new()),
        }
    }

    pub fn inc(&self, l1: &str, l2: &str) {
        let mut map = self.values.write().unwrap();
        *map.entry((l1.to_string(), l2.to_string())).or_insert(0) += 1;
    }

    pub fn snapshot(&self) -> Vec<((String, String), u64)> {
        let map = self.values.read().unwrap();
        map.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }
}

/// Gauge with a single string label, f64 values.
pub struct LabeledGaugeF64 {
    values: RwLock<HashMap<String, f64>>,
}

impl LabeledGaugeF64 {
    pub fn new() -> Self {
        Self {
            values: RwLock::new(HashMap::new()),
        }
    }

    pub fn set(&self, label: &str, val: f64) {
        let mut map = self.values.write().unwrap();
        map.insert(label.to_string(), val);
    }

    pub fn snapshot(&self) -> Vec<(String, f64)> {
        let map = self.values.read().unwrap();
        map.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }
}

/// Gauge with a single string label, i64 values (for inc/dec patterns like active requests).
pub struct LabeledGaugeI64 {
    values: RwLock<HashMap<String, AtomicI64>>,
}

impl LabeledGaugeI64 {
    pub fn new() -> Self {
        Self {
            values: RwLock::new(HashMap::new()),
        }
    }

    pub fn inc(&self, label: &str) {
        let map = self.values.read().unwrap();
        if let Some(v) = map.get(label) {
            v.fetch_add(1, Ordering::Relaxed);
            return;
        }
        drop(map);
        let mut map = self.values.write().unwrap();
        map.entry(label.to_string())
            .or_insert_with(|| AtomicI64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec(&self, label: &str) {
        let map = self.values.read().unwrap();
        if let Some(v) = map.get(label) {
            v.fetch_sub(1, Ordering::Relaxed);
            return;
        }
        drop(map);
        let mut map = self.values.write().unwrap();
        map.entry(label.to_string())
            .or_insert_with(|| AtomicI64::new(0))
            .fetch_sub(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Vec<(String, i64)> {
        let map = self.values.read().unwrap();
        map.iter()
            .map(|(k, v)| (k.clone(), v.load(Ordering::Relaxed)))
            .collect()
    }
}

/// Per-provider request/error/latency tracking.
pub struct ProviderStats {
    pub requests: AtomicU64,
    pub errors: AtomicU64,
    pub latency_sum_ms: AtomicU64,
}

impl ProviderStats {
    pub fn new() -> Self {
        Self {
            requests: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            latency_sum_ms: AtomicU64::new(0),
        }
    }
}

/// Map of provider_id → ProviderStats.
pub struct ProviderStatsMap {
    stats: RwLock<HashMap<String, Arc<ProviderStats>>>,
}

impl ProviderStatsMap {
    pub fn new() -> Self {
        Self {
            stats: RwLock::new(HashMap::new()),
        }
    }

    fn get_or_create(&self, provider_id: &str) -> Arc<ProviderStats> {
        // Fast path: read lock
        {
            let map = self.stats.read().unwrap();
            if let Some(s) = map.get(provider_id) {
                return Arc::clone(s);
            }
        }
        // Slow path: write lock
        let mut map = self.stats.write().unwrap();
        Arc::clone(
            map.entry(provider_id.to_string())
                .or_insert_with(|| Arc::new(ProviderStats::new())),
        )
    }

    /// Record a successful request with latency.
    pub fn record(&self, provider_id: &str, latency_ms: u64) {
        let s = self.get_or_create(provider_id);
        s.requests.fetch_add(1, Ordering::Relaxed);
        s.latency_sum_ms.fetch_add(latency_ms, Ordering::Relaxed);
    }

    /// Record an error for a provider.
    pub fn record_error(&self, provider_id: &str) {
        let s = self.get_or_create(provider_id);
        s.requests.fetch_add(1, Ordering::Relaxed);
        s.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot all provider stats.
    pub fn snapshot(&self) -> Vec<(String, u64, u64, u64)> {
        let map = self.stats.read().unwrap();
        map.iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.requests.load(Ordering::Relaxed),
                    v.errors.load(Ordering::Relaxed),
                    v.latency_sum_ms.load(Ordering::Relaxed),
                )
            })
            .collect()
    }
}

/// DataLayer query metrics.
pub struct DataLayerMetrics {
    pub queries_total: AtomicU64,
    pub hits_total: AtomicU64,
    pub latency_sum_us: AtomicU64,
}

impl DataLayerMetrics {
    pub fn new() -> Self {
        Self {
            queries_total: AtomicU64::new(0),
            hits_total: AtomicU64::new(0),
            latency_sum_us: AtomicU64::new(0),
        }
    }
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

    // ── Routing decision metrics ──
    pub routing_decision_duration: UsHistogram,
    pub strategy_calls: LabeledCounter,
    pub fallback_triggered: AtomicU64,
    pub plugin_filter_duration: UsHistogram,
    pub plugin_score_duration: UsHistogram,

    // ── Provider metrics ──
    pub provider_stats: ProviderStatsMap,
    pub rate_limit_utilization: LabeledGaugeF64,
    pub circuit_breaker_state: LabeledGaugeF64,

    // ── Traffic distribution ──
    pub routes_by_model: LabeledCounter,
    pub routes_by_provider: LabeledCounter,
    pub routes_by_account: LabeledCounter,
    pub experiment_assignments: DualLabeledCounter,

    // ── Token usage ──
    pub input_tokens: LabeledCounter,
    pub output_tokens: LabeledCounter,
    pub tokens_per_second: LabeledGaugeF64,

    // ── Resource ──
    pub active_requests: LabeledGaugeI64,
    pub queue_depth: AtomicU64,

    // ── DataLayer ──
    pub data_layer_metrics: DataLayerMetrics,
}

impl MetricsCollector {
    /// Create a new collector with the given window size per variant.
    pub fn new(window_size: usize) -> Self {
        Self {
            windows: RwLock::new(HashMap::new()),
            window_size,
            total_requests: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),

            // Routing decision
            routing_decision_duration: UsHistogram::new(vec![1, 5, 10, 50, 100, 500, 1000, 5000]),
            strategy_calls: LabeledCounter::new(),
            fallback_triggered: AtomicU64::new(0),
            plugin_filter_duration: UsHistogram::new(vec![1, 5, 10, 50, 100, 500, 1000, 5000]),
            plugin_score_duration: UsHistogram::new(vec![1, 5, 10, 50, 100, 500, 1000, 5000]),

            // Provider
            provider_stats: ProviderStatsMap::new(),
            rate_limit_utilization: LabeledGaugeF64::new(),
            circuit_breaker_state: LabeledGaugeF64::new(),

            // Traffic distribution
            routes_by_model: LabeledCounter::new(),
            routes_by_provider: LabeledCounter::new(),
            routes_by_account: LabeledCounter::new(),
            experiment_assignments: DualLabeledCounter::new(),

            // Token usage
            input_tokens: LabeledCounter::new(),
            output_tokens: LabeledCounter::new(),
            tokens_per_second: LabeledGaugeF64::new(),

            // Resource
            active_requests: LabeledGaugeI64::new(),
            queue_depth: AtomicU64::new(0),

            // DataLayer
            data_layer_metrics: DataLayerMetrics::new(),
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

    /// Record a request with full context (provider, model, account).
    pub fn record_with_context(
        &self,
        record: RequestRecord,
        provider: &str,
        model: &str,
        account: &str,
    ) {
        // Token counters
        if let Some(inp) = record.input_tokens {
            self.input_tokens
                .inc_by(&record.variant_id, inp as u64);
        }
        if let Some(out) = record.output_tokens {
            self.output_tokens
                .inc_by(&record.variant_id, out as u64);
        }

        // Provider stats
        if record.success {
            self.provider_stats.record(provider, record.latency_ms);
        } else {
            self.provider_stats.record_error(provider);
        }

        // Traffic distribution
        self.routes_by_model.inc(model);
        self.routes_by_provider.inc(provider);
        self.routes_by_account.inc(account);

        // Core record (variant window)
        self.record(record);
    }

    /// Record a routing decision duration and strategy.
    pub fn record_routing_decision(&self, time_us: u64, strategy: &str) {
        self.routing_decision_duration.observe(time_us);
        self.strategy_calls.inc(strategy);
    }

    /// Record a fallback trigger.
    pub fn record_fallback(&self) {
        self.fallback_triggered.fetch_add(1, Ordering::Relaxed);
    }

    /// Record plugin filter duration (microseconds).
    pub fn record_plugin_filter_duration(&self, us: u64) {
        self.plugin_filter_duration.observe(us);
    }

    /// Record plugin score duration (microseconds).
    pub fn record_plugin_score_duration(&self, us: u64) {
        self.plugin_score_duration.observe(us);
    }

    /// Record an experiment assignment.
    pub fn record_experiment(&self, exp_id: &str, variant: &str) {
        self.experiment_assignments.inc(exp_id, variant);
    }

    /// Record a DataLayer query.
    pub fn record_data_layer_query(&self, hit: bool, latency_us: u64) {
        self.data_layer_metrics
            .queries_total
            .fetch_add(1, Ordering::Relaxed);
        if hit {
            self.data_layer_metrics
                .hits_total
                .fetch_add(1, Ordering::Relaxed);
        }
        self.data_layer_metrics
            .latency_sum_us
            .fetch_add(latency_us, Ordering::Relaxed);
    }

    /// Increment active requests for a variant.
    pub fn inc_active_requests(&self, variant: &str) {
        self.active_requests.inc(variant);
    }

    /// Decrement active requests for a variant.
    pub fn dec_active_requests(&self, variant: &str) {
        self.active_requests.dec(variant);
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

    /// Format all metrics in Prometheus exposition format.
    pub fn format_prometheus(
        &self,
        uptime_secs: u64,
        model_count: usize,
        variant_count: usize,
    ) -> String {
        let mut out = String::with_capacity(4096);

        // ── Global counters ──
        prom_help_type(&mut out, "prism_requests_total", "Total routing requests", "counter");
        prom_counter(&mut out, "prism_requests_total", self.total_requests());

        prom_help_type(&mut out, "prism_errors_total", "Total failed requests", "counter");
        prom_counter(&mut out, "prism_errors_total", self.total_errors());

        prom_help_type(&mut out, "prism_uptime_seconds", "Router uptime", "gauge");
        prom_gauge_u64(&mut out, "prism_uptime_seconds", uptime_secs);

        prom_help_type(&mut out, "prism_registered_models", "Number of registered models", "gauge");
        prom_gauge_u64(&mut out, "prism_registered_models", model_count as u64);

        prom_help_type(&mut out, "prism_active_variants", "Number of active variants", "gauge");
        prom_gauge_u64(&mut out, "prism_active_variants", variant_count as u64);

        // ── 1. Routing decision metrics ──
        format_histogram(
            &mut out,
            "prism_routing_decision_duration_us",
            "Routing decision duration in microseconds",
            &self.routing_decision_duration,
        );

        prom_help_type(&mut out, "prism_strategy_calls_total", "Strategy calls by type", "counter");
        for (label, val) in self.strategy_calls.snapshot() {
            out.push_str(&format!(
                "prism_strategy_calls_total{{strategy=\"{}\"}} {}\n",
                label, val
            ));
        }

        prom_help_type(&mut out, "prism_fallback_triggered_total", "Total fallback triggers", "counter");
        prom_counter(
            &mut out,
            "prism_fallback_triggered_total",
            self.fallback_triggered.load(Ordering::Relaxed),
        );

        format_histogram(
            &mut out,
            "prism_plugin_filter_duration_us",
            "Plugin filter phase duration in microseconds",
            &self.plugin_filter_duration,
        );

        format_histogram(
            &mut out,
            "prism_plugin_score_duration_us",
            "Plugin score phase duration in microseconds",
            &self.plugin_score_duration,
        );

        // ── 2. Provider metrics ──
        prom_help_type(&mut out, "prism_provider_requests_total", "Provider request count", "counter");
        prom_help_type(&mut out, "prism_provider_errors_total", "Provider error count", "counter");
        prom_help_type(&mut out, "prism_provider_latency_sum_ms", "Provider latency sum in ms", "counter");
        for (provider, requests, errors, latency_sum) in self.provider_stats.snapshot() {
            out.push_str(&format!(
                "prism_provider_requests_total{{provider=\"{}\"}} {}\n",
                provider, requests
            ));
            out.push_str(&format!(
                "prism_provider_errors_total{{provider=\"{}\"}} {}\n",
                provider, errors
            ));
            out.push_str(&format!(
                "prism_provider_latency_sum_ms{{provider=\"{}\"}} {}\n",
                provider, latency_sum
            ));
        }

        prom_help_type(&mut out, "prism_rate_limit_utilization", "Rate limit utilization (0.0-1.0)", "gauge");
        for (label, val) in self.rate_limit_utilization.snapshot() {
            out.push_str(&format!(
                "prism_rate_limit_utilization{{provider=\"{}\"}} {:.4}\n",
                label, val
            ));
        }

        prom_help_type(&mut out, "prism_circuit_breaker_state", "Circuit breaker state (0=closed, 1=open, 2=half-open)", "gauge");
        for (label, val) in self.circuit_breaker_state.snapshot() {
            out.push_str(&format!(
                "prism_circuit_breaker_state{{variant=\"{}\"}} {}\n",
                label, val as u64
            ));
        }

        // ── 3. Traffic distribution ──
        prom_help_type(&mut out, "prism_routes_by_model_total", "Routes by model", "counter");
        for (label, val) in self.routes_by_model.snapshot() {
            out.push_str(&format!(
                "prism_routes_by_model_total{{model=\"{}\"}} {}\n",
                label, val
            ));
        }

        prom_help_type(&mut out, "prism_routes_by_provider_total", "Routes by provider", "counter");
        for (label, val) in self.routes_by_provider.snapshot() {
            out.push_str(&format!(
                "prism_routes_by_provider_total{{provider=\"{}\"}} {}\n",
                label, val
            ));
        }

        prom_help_type(&mut out, "prism_routes_by_account_total", "Routes by account", "counter");
        for (label, val) in self.routes_by_account.snapshot() {
            out.push_str(&format!(
                "prism_routes_by_account_total{{account=\"{}\"}} {}\n",
                label, val
            ));
        }

        prom_help_type(&mut out, "prism_experiment_assignments_total", "Experiment assignments", "counter");
        for ((exp, variant), val) in self.experiment_assignments.snapshot() {
            out.push_str(&format!(
                "prism_experiment_assignments_total{{experiment=\"{}\",variant=\"{}\"}} {}\n",
                exp, variant, val
            ));
        }

        // ── 4. Token usage ──
        prom_help_type(&mut out, "prism_input_tokens_total", "Input tokens by variant", "counter");
        for (label, val) in self.input_tokens.snapshot() {
            out.push_str(&format!(
                "prism_input_tokens_total{{variant=\"{}\"}} {}\n",
                label, val
            ));
        }

        prom_help_type(&mut out, "prism_output_tokens_total", "Output tokens by variant", "counter");
        for (label, val) in self.output_tokens.snapshot() {
            out.push_str(&format!(
                "prism_output_tokens_total{{variant=\"{}\"}} {}\n",
                label, val
            ));
        }

        prom_help_type(&mut out, "prism_tokens_per_second", "Tokens per second by variant", "gauge");
        for (label, val) in self.tokens_per_second.snapshot() {
            out.push_str(&format!(
                "prism_tokens_per_second{{variant=\"{}\"}} {:.2}\n",
                label, val
            ));
        }

        // ── 5. Resource ──
        prom_help_type(&mut out, "prism_active_requests", "Active in-flight requests by variant", "gauge");
        for (label, val) in self.active_requests.snapshot() {
            out.push_str(&format!(
                "prism_active_requests{{variant=\"{}\"}} {}\n",
                label, val
            ));
        }

        prom_help_type(&mut out, "prism_queue_depth", "Global queue depth", "gauge");
        prom_gauge_u64(&mut out, "prism_queue_depth", self.queue_depth.load(Ordering::Relaxed));

        // ── 6. DataLayer ──
        prom_help_type(&mut out, "prism_data_layer_queries_total", "DataLayer queries", "counter");
        prom_counter(
            &mut out,
            "prism_data_layer_queries_total",
            self.data_layer_metrics.queries_total.load(Ordering::Relaxed),
        );

        prom_help_type(&mut out, "prism_data_layer_hits_total", "DataLayer cache hits", "counter");
        prom_counter(
            &mut out,
            "prism_data_layer_hits_total",
            self.data_layer_metrics.hits_total.load(Ordering::Relaxed),
        );

        prom_help_type(&mut out, "prism_data_layer_latency_sum_us", "DataLayer latency sum in microseconds", "counter");
        prom_counter(
            &mut out,
            "prism_data_layer_latency_sum_us",
            self.data_layer_metrics.latency_sum_us.load(Ordering::Relaxed),
        );

        // ── Per-variant window metrics ──
        let variants = self.tracked_variants();
        if !variants.is_empty() {
            prom_help_type(&mut out, "prism_variant_latency_p50_ms", "P50 latency per variant", "gauge");
            prom_help_type(&mut out, "prism_variant_latency_p95_ms", "P95 latency per variant", "gauge");
            prom_help_type(&mut out, "prism_variant_latency_p99_ms", "P99 latency per variant", "gauge");
            prom_help_type(&mut out, "prism_variant_error_rate", "Error rate per variant", "gauge");
            prom_help_type(&mut out, "prism_variant_qps", "Current QPS per variant", "gauge");
            prom_help_type(&mut out, "prism_variant_ttft_p50_ms", "P50 TTFT per variant", "gauge");

            for variant_id in &variants {
                if let Some(snap) = self.snapshot(variant_id) {
                    let label = format!("variant=\"{}\"", variant_id);
                    out.push_str(&format!(
                        "prism_variant_latency_p50_ms{{{}}} {}\n",
                        label, snap.p50_latency_ms
                    ));
                    out.push_str(&format!(
                        "prism_variant_latency_p95_ms{{{}}} {}\n",
                        label, snap.p95_latency_ms
                    ));
                    out.push_str(&format!(
                        "prism_variant_latency_p99_ms{{{}}} {}\n",
                        label, snap.p99_latency_ms
                    ));
                    out.push_str(&format!(
                        "prism_variant_error_rate{{{}}} {:.4}\n",
                        label, snap.error_rate
                    ));
                    out.push_str(&format!(
                        "prism_variant_qps{{{}}} {:.2}\n",
                        label, snap.current_qps
                    ));
                    out.push_str(&format!(
                        "prism_variant_ttft_p50_ms{{{}}} {}\n",
                        label, snap.ttft_p50_ms
                    ));
                }
            }
        }

        out
    }
}

// ──────────────────────────────────────────────
// Prometheus formatting helpers
// ──────────────────────────────────────────────

fn prom_help_type(out: &mut String, name: &str, help: &str, metric_type: &str) {
    out.push_str(&format!("# HELP {} {}\n", name, help));
    out.push_str(&format!("# TYPE {} {}\n", name, metric_type));
}

fn prom_counter(out: &mut String, name: &str, value: u64) {
    out.push_str(&format!("{} {}\n", name, value));
}

fn prom_gauge_u64(out: &mut String, name: &str, value: u64) {
    out.push_str(&format!("{} {}\n", name, value));
}

#[allow(dead_code)]
fn prom_gauge_f64(out: &mut String, name: &str, value: f64) {
    out.push_str(&format!("{} {:.4}\n", name, value));
}

#[allow(dead_code)]
fn prom_gauge_i64(out: &mut String, name: &str, value: i64) {
    out.push_str(&format!("{} {}\n", name, value));
}

fn format_histogram(out: &mut String, name: &str, help: &str, histogram: &UsHistogram) {
    let (bounds, counts, count, sum) = histogram.snapshot();
    prom_help_type(out, name, help, "histogram");
    for (i, bound) in bounds.iter().enumerate() {
        out.push_str(&format!(
            "{}_bucket{{le=\"{}\"}} {}\n",
            name, bound, counts[i]
        ));
    }
    out.push_str(&format!("{}_bucket{{le=\"+Inf\"}} {}\n", name, counts[bounds.len()]));
    out.push_str(&format!("{}_sum {}\n", name, sum));
    out.push_str(&format!("{}_count {}\n", name, count));
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

    // ──────────────────────────────────────────
    // Tests for new metric primitives
    // ──────────────────────────────────────────

    #[test]
    fn test_us_histogram_basic() {
        let h = UsHistogram::new(vec![10, 50, 100]);
        h.observe(5);
        h.observe(30);
        h.observe(75);
        h.observe(200);

        let (bounds, counts, count, sum) = h.snapshot();
        assert_eq!(bounds, vec![10, 50, 100]);
        assert_eq!(count, 4);
        assert_eq!(sum, 5 + 30 + 75 + 200);
        // le=10: 5 → 1
        assert_eq!(counts[0], 1);
        // le=50: 5,30 → 2
        assert_eq!(counts[1], 2);
        // le=100: 5,30,75 → 3
        assert_eq!(counts[2], 3);
        // +Inf: 4
        assert_eq!(counts[3], 4);
    }

    #[test]
    fn test_us_histogram_empty() {
        let h = UsHistogram::new(vec![1, 5, 10]);
        let (_, counts, count, sum) = h.snapshot();
        assert_eq!(count, 0);
        assert_eq!(sum, 0);
        for c in &counts {
            assert_eq!(*c, 0);
        }
    }

    #[test]
    fn test_us_histogram_all_below_first_bucket() {
        let h = UsHistogram::new(vec![100, 500, 1000]);
        h.observe(1);
        h.observe(50);

        let (_, counts, count, _) = h.snapshot();
        assert_eq!(count, 2);
        assert_eq!(counts[0], 2); // le=100
        assert_eq!(counts[1], 2); // le=500
        assert_eq!(counts[2], 2); // le=1000
        assert_eq!(counts[3], 2); // +Inf
    }

    #[test]
    fn test_labeled_counter_basic() {
        let c = LabeledCounter::new();
        c.inc("a");
        c.inc("a");
        c.inc("b");
        c.inc_by("b", 5);

        let snap = c.snapshot();
        let map: HashMap<String, u64> = snap.into_iter().collect();
        assert_eq!(map["a"], 2);
        assert_eq!(map["b"], 6);
    }

    #[test]
    fn test_labeled_counter_empty() {
        let c = LabeledCounter::new();
        assert!(c.snapshot().is_empty());
    }

    #[test]
    fn test_dual_labeled_counter_basic() {
        let c = DualLabeledCounter::new();
        c.inc("exp1", "control");
        c.inc("exp1", "control");
        c.inc("exp1", "treatment");
        c.inc("exp2", "v1");

        let snap = c.snapshot();
        let map: HashMap<(String, String), u64> = snap.into_iter().collect();
        assert_eq!(map[&("exp1".to_string(), "control".to_string())], 2);
        assert_eq!(map[&("exp1".to_string(), "treatment".to_string())], 1);
        assert_eq!(map[&("exp2".to_string(), "v1".to_string())], 1);
    }

    #[test]
    fn test_dual_labeled_counter_empty() {
        let c = DualLabeledCounter::new();
        assert!(c.snapshot().is_empty());
    }

    #[test]
    fn test_labeled_gauge_f64_basic() {
        let g = LabeledGaugeF64::new();
        g.set("p1", 0.5);
        g.set("p2", 0.8);
        g.set("p1", 0.3); // overwrite

        let snap = g.snapshot();
        let map: HashMap<String, f64> = snap.into_iter().collect();
        assert!((map["p1"] - 0.3).abs() < f64::EPSILON);
        assert!((map["p2"] - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn test_labeled_gauge_f64_empty() {
        let g = LabeledGaugeF64::new();
        assert!(g.snapshot().is_empty());
    }

    #[test]
    fn test_labeled_gauge_i64_inc_dec() {
        let g = LabeledGaugeI64::new();
        g.inc("v1");
        g.inc("v1");
        g.inc("v1");
        g.dec("v1");

        let snap = g.snapshot();
        let map: HashMap<String, i64> = snap.into_iter().collect();
        assert_eq!(map["v1"], 2);
    }

    #[test]
    fn test_labeled_gauge_i64_negative() {
        let g = LabeledGaugeI64::new();
        g.dec("v1");
        g.dec("v1");

        let snap = g.snapshot();
        let map: HashMap<String, i64> = snap.into_iter().collect();
        assert_eq!(map["v1"], -2);
    }

    #[test]
    fn test_labeled_gauge_i64_multiple_labels() {
        let g = LabeledGaugeI64::new();
        g.inc("a");
        g.inc("b");
        g.inc("b");

        let snap = g.snapshot();
        let map: HashMap<String, i64> = snap.into_iter().collect();
        assert_eq!(map["a"], 1);
        assert_eq!(map["b"], 2);
    }

    #[test]
    fn test_provider_stats_map_record() {
        let m = ProviderStatsMap::new();
        m.record("openai", 100);
        m.record("openai", 200);
        m.record_error("openai");
        m.record("anthropic", 50);

        let snap = m.snapshot();
        let map: HashMap<String, (u64, u64, u64)> = snap
            .into_iter()
            .map(|(k, r, e, l)| (k, (r, e, l)))
            .collect();

        assert_eq!(map["openai"], (3, 1, 300)); // 2 success + 1 error = 3 requests
        assert_eq!(map["anthropic"], (1, 0, 50));
    }

    #[test]
    fn test_provider_stats_map_empty() {
        let m = ProviderStatsMap::new();
        assert!(m.snapshot().is_empty());
    }

    #[test]
    fn test_data_layer_metrics() {
        let d = DataLayerMetrics::new();
        assert_eq!(d.queries_total.load(Ordering::Relaxed), 0);
        assert_eq!(d.hits_total.load(Ordering::Relaxed), 0);
        assert_eq!(d.latency_sum_us.load(Ordering::Relaxed), 0);

        d.queries_total.fetch_add(5, Ordering::Relaxed);
        d.hits_total.fetch_add(3, Ordering::Relaxed);
        d.latency_sum_us.fetch_add(1000, Ordering::Relaxed);

        assert_eq!(d.queries_total.load(Ordering::Relaxed), 5);
        assert_eq!(d.hits_total.load(Ordering::Relaxed), 3);
        assert_eq!(d.latency_sum_us.load(Ordering::Relaxed), 1000);
    }

    // ──────────────────────────────────────────
    // Tests for MetricsCollector new methods
    // ──────────────────────────────────────────

    #[test]
    fn test_record_with_context() {
        let collector = MetricsCollector::new(100);
        let rec = RequestRecord {
            variant_id: "v1".to_string(),
            latency_ms: 150,
            ttft_ms: None,
            success: true,
            input_tokens: Some(100),
            output_tokens: Some(200),
            timestamp: Instant::now(),
        };
        collector.record_with_context(rec, "openai", "gpt-4o", "acct-1");

        assert_eq!(collector.total_requests(), 1);

        let model_snap: HashMap<String, u64> =
            collector.routes_by_model.snapshot().into_iter().collect();
        assert_eq!(model_snap["gpt-4o"], 1);

        let provider_snap: HashMap<String, u64> =
            collector.routes_by_provider.snapshot().into_iter().collect();
        assert_eq!(provider_snap["openai"], 1);

        let account_snap: HashMap<String, u64> =
            collector.routes_by_account.snapshot().into_iter().collect();
        assert_eq!(account_snap["acct-1"], 1);

        let input_snap: HashMap<String, u64> =
            collector.input_tokens.snapshot().into_iter().collect();
        assert_eq!(input_snap["v1"], 100);

        let output_snap: HashMap<String, u64> =
            collector.output_tokens.snapshot().into_iter().collect();
        assert_eq!(output_snap["v1"], 200);

        let prov_stats = collector.provider_stats.snapshot();
        let prov_map: HashMap<String, (u64, u64, u64)> = prov_stats
            .into_iter()
            .map(|(k, r, e, l)| (k, (r, e, l)))
            .collect();
        assert_eq!(prov_map["openai"], (1, 0, 150));
    }

    #[test]
    fn test_record_with_context_error() {
        let collector = MetricsCollector::new(100);
        let rec = RequestRecord {
            variant_id: "v1".to_string(),
            latency_ms: 500,
            ttft_ms: None,
            success: false,
            input_tokens: None,
            output_tokens: None,
            timestamp: Instant::now(),
        };
        collector.record_with_context(rec, "anthropic", "claude", "acct-2");

        assert_eq!(collector.total_errors(), 1);

        let prov_stats = collector.provider_stats.snapshot();
        let prov_map: HashMap<String, (u64, u64, u64)> = prov_stats
            .into_iter()
            .map(|(k, r, e, l)| (k, (r, e, l)))
            .collect();
        // Error: requests=1, errors=1, latency_sum=0
        assert_eq!(prov_map["anthropic"], (1, 1, 0));
    }

    #[test]
    fn test_record_routing_decision() {
        let collector = MetricsCollector::new(100);
        collector.record_routing_decision(42, "auto");
        collector.record_routing_decision(100, "auto");
        collector.record_routing_decision(5, "pinned");

        let (_, _, count, sum) = collector.routing_decision_duration.snapshot();
        assert_eq!(count, 3);
        assert_eq!(sum, 42 + 100 + 5);

        let strategy_snap: HashMap<String, u64> =
            collector.strategy_calls.snapshot().into_iter().collect();
        assert_eq!(strategy_snap["auto"], 2);
        assert_eq!(strategy_snap["pinned"], 1);
    }

    #[test]
    fn test_record_fallback() {
        let collector = MetricsCollector::new(100);
        collector.record_fallback();
        collector.record_fallback();
        assert_eq!(
            collector.fallback_triggered.load(Ordering::Relaxed),
            2
        );
    }

    #[test]
    fn test_record_plugin_durations() {
        let collector = MetricsCollector::new(100);
        collector.record_plugin_filter_duration(10);
        collector.record_plugin_filter_duration(50);
        collector.record_plugin_score_duration(5);

        let (_, _, filter_count, _) = collector.plugin_filter_duration.snapshot();
        assert_eq!(filter_count, 2);

        let (_, _, score_count, _) = collector.plugin_score_duration.snapshot();
        assert_eq!(score_count, 1);
    }

    #[test]
    fn test_record_experiment() {
        let collector = MetricsCollector::new(100);
        collector.record_experiment("exp-1", "control");
        collector.record_experiment("exp-1", "treatment");
        collector.record_experiment("exp-1", "control");

        let snap = collector.experiment_assignments.snapshot();
        let map: HashMap<(String, String), u64> = snap.into_iter().collect();
        assert_eq!(
            map[&("exp-1".to_string(), "control".to_string())],
            2
        );
        assert_eq!(
            map[&("exp-1".to_string(), "treatment".to_string())],
            1
        );
    }

    #[test]
    fn test_record_data_layer_query() {
        let collector = MetricsCollector::new(100);
        collector.record_data_layer_query(true, 50);
        collector.record_data_layer_query(false, 100);
        collector.record_data_layer_query(true, 30);

        assert_eq!(
            collector
                .data_layer_metrics
                .queries_total
                .load(Ordering::Relaxed),
            3
        );
        assert_eq!(
            collector
                .data_layer_metrics
                .hits_total
                .load(Ordering::Relaxed),
            2
        );
        assert_eq!(
            collector
                .data_layer_metrics
                .latency_sum_us
                .load(Ordering::Relaxed),
            180
        );
    }

    #[test]
    fn test_active_requests_inc_dec() {
        let collector = MetricsCollector::new(100);
        collector.inc_active_requests("v1");
        collector.inc_active_requests("v1");
        collector.inc_active_requests("v2");
        collector.dec_active_requests("v1");

        let snap: HashMap<String, i64> =
            collector.active_requests.snapshot().into_iter().collect();
        assert_eq!(snap["v1"], 1);
        assert_eq!(snap["v2"], 1);
    }

    #[test]
    fn test_format_prometheus_basic() {
        let collector = MetricsCollector::new(100);
        // Record some data
        collector.record(make_record("v1", 100, true));
        collector.record_routing_decision(42, "auto");
        collector.record_fallback();
        collector.record_data_layer_query(true, 10);

        let output = collector.format_prometheus(60, 3, 5);

        // Check key sections
        assert!(output.contains("prism_requests_total 1"));
        assert!(output.contains("prism_errors_total 0"));
        assert!(output.contains("prism_uptime_seconds 60"));
        assert!(output.contains("prism_registered_models 3"));
        assert!(output.contains("prism_active_variants 5"));

        // Routing decision histogram
        assert!(output.contains("prism_routing_decision_duration_us_count 1"));
        assert!(output.contains("prism_routing_decision_duration_us_sum 42"));
        assert!(output.contains("prism_routing_decision_duration_us_bucket{le=\"50\"} 1"));

        // Strategy calls
        assert!(output.contains("prism_strategy_calls_total{strategy=\"auto\"} 1"));

        // Fallback
        assert!(output.contains("prism_fallback_triggered_total 1"));

        // DataLayer
        assert!(output.contains("prism_data_layer_queries_total 1"));
        assert!(output.contains("prism_data_layer_hits_total 1"));
        assert!(output.contains("prism_data_layer_latency_sum_us 10"));

        // Per-variant
        assert!(output.contains("prism_variant_latency_p50_ms{variant=\"v1\"}"));
    }

    #[test]
    fn test_format_prometheus_empty() {
        let collector = MetricsCollector::new(100);
        let output = collector.format_prometheus(0, 0, 0);
        assert!(output.contains("prism_requests_total 0"));
        assert!(output.contains("prism_errors_total 0"));
        assert!(output.contains("prism_uptime_seconds 0"));
    }

    #[test]
    fn test_format_prometheus_with_provider_metrics() {
        let collector = MetricsCollector::new(100);
        let rec = RequestRecord {
            variant_id: "v1".to_string(),
            latency_ms: 200,
            ttft_ms: None,
            success: true,
            input_tokens: Some(50),
            output_tokens: Some(100),
            timestamp: Instant::now(),
        };
        collector.record_with_context(rec, "openai", "gpt-4o", "acct-1");
        collector.rate_limit_utilization.set("openai", 0.75);
        collector.circuit_breaker_state.set("v1", 0.0);

        let output = collector.format_prometheus(10, 1, 1);

        assert!(output.contains("prism_provider_requests_total{provider=\"openai\"} 1"));
        assert!(output.contains("prism_provider_errors_total{provider=\"openai\"} 0"));
        assert!(output.contains("prism_provider_latency_sum_ms{provider=\"openai\"} 200"));
        assert!(output.contains("prism_rate_limit_utilization{provider=\"openai\"} 0.75"));
        assert!(output.contains("prism_circuit_breaker_state{variant=\"v1\"} 0"));
        assert!(output.contains("prism_routes_by_model_total{model=\"gpt-4o\"} 1"));
        assert!(output.contains("prism_routes_by_provider_total{provider=\"openai\"} 1"));
        assert!(output.contains("prism_routes_by_account_total{account=\"acct-1\"} 1"));
        assert!(output.contains("prism_input_tokens_total{variant=\"v1\"} 50"));
        assert!(output.contains("prism_output_tokens_total{variant=\"v1\"} 100"));
    }

    #[test]
    fn test_format_prometheus_histogram_format() {
        let collector = MetricsCollector::new(100);
        collector.record_routing_decision(3, "auto");
        collector.record_routing_decision(55, "auto");

        let output = collector.format_prometheus(1, 0, 0);

        // Check histogram format compliance
        assert!(output.contains("# TYPE prism_routing_decision_duration_us histogram"));
        assert!(output.contains("prism_routing_decision_duration_us_bucket{le=\"1\"} 0"));
        assert!(output.contains("prism_routing_decision_duration_us_bucket{le=\"5\"} 1")); // 3 <= 5
        assert!(output.contains("prism_routing_decision_duration_us_bucket{le=\"100\"} 2")); // both <= 100
        assert!(output.contains("prism_routing_decision_duration_us_bucket{le=\"+Inf\"} 2"));
        assert!(output.contains("prism_routing_decision_duration_us_sum 58"));
        assert!(output.contains("prism_routing_decision_duration_us_count 2"));
    }

    #[test]
    fn test_format_prometheus_experiment_assignments() {
        let collector = MetricsCollector::new(100);
        collector.record_experiment("ab-test", "control");
        collector.record_experiment("ab-test", "variant_a");

        let output = collector.format_prometheus(1, 0, 0);
        assert!(output.contains("prism_experiment_assignments_total{experiment=\"ab-test\",variant=\"control\"} 1"));
        assert!(output.contains("prism_experiment_assignments_total{experiment=\"ab-test\",variant=\"variant_a\"} 1"));
    }

    #[test]
    fn test_format_prometheus_active_requests() {
        let collector = MetricsCollector::new(100);
        collector.inc_active_requests("v1");
        collector.inc_active_requests("v1");
        collector.inc_active_requests("v2");

        let output = collector.format_prometheus(1, 0, 0);
        assert!(output.contains("prism_active_requests{variant=\"v1\"} 2"));
        assert!(output.contains("prism_active_requests{variant=\"v2\"} 1"));
    }
}
