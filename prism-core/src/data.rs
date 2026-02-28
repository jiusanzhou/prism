//! Dynamic data layer with pluggable DataSource registration.
//!
//! Each [`DataSource`] declares the [`MetricKind`]s it supports.
//! The [`DataLayer`] routes queries to the appropriate source by kind,
//! resolving conflicts via [`DataTier`] priority.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::types::{ModelPerformance, ModelPricing};

// ──────────────────────────────────────────────
// MetricKind
// ──────────────────────────────────────────────

/// Metric type — declared by each [`DataSource`] at registration time.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum MetricKind {
    /// Runtime performance metrics (latency / QPS / error rate / queue depth).
    /// key = variant_id
    Performance,
    /// Dynamic pricing overrides (real-time price changes / discounts).
    /// key = variant_id
    Pricing,
    /// Quota remaining (RPM / TPM remaining).
    /// key = account_id
    Quota,
    /// SLO status (availability targets / violation alerts).
    /// key = provider_id
    SLO,
    /// External signals (degradation notices / maintenance announcements).
    /// key = provider_id
    Signal,
}

// ──────────────────────────────────────────────
// MetricValue
// ──────────────────────────────────────────────

/// Carrier for metric values of each kind.
#[derive(Debug, Clone)]
pub enum MetricValue {
    Performance(ModelPerformance),
    Pricing(ModelPricing),
    Quota(QuotaStatus),
    SLO(SLOStatus),
    Signal(SignalInfo),
}

/// Quota remaining status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuotaStatus {
    pub rpm_remaining: Option<u32>,
    pub tpm_remaining: Option<u32>,
    pub concurrent_remaining: Option<u32>,
    /// Unix epoch seconds when the quota resets.
    pub reset_at: Option<u64>,
}

/// SLO status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SLOStatus {
    /// e.g. 0.999
    pub target_availability: f32,
    /// e.g. 0.995
    pub current_availability: f32,
    /// e.g. 5000
    pub target_p99_latency_ms: u32,
    /// Error budget remaining %.
    pub budget_remaining_pct: f32,
    /// Whether the SLO is currently being violated.
    pub in_violation: bool,
}

/// External signal information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignalInfo {
    pub status: ProviderSignalStatus,
    pub message: Option<String>,
    pub severity: SignalSeverity,
}

/// Provider signal status.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderSignalStatus {
    #[default]
    Normal,
    Degraded,
    Maintenance,
    Outage,
}

/// Signal severity level.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SignalSeverity {
    #[default]
    Info,
    Warning,
    Critical,
}

// ──────────────────────────────────────────────
// DataTier
// ──────────────────────────────────────────────

/// Data tier — determines update frequency and query priority.
///
/// Ordering: `NearRealtime < External < Static`.
/// Lower ordinal = higher priority for queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DataTier {
    /// Second-level updates, produced by this instance.
    NearRealtime,
    /// Minute-level, externally pushed or pulled.
    External,
    /// Written at config-load time, rarely changes.
    Static,
}

// ──────────────────────────────────────────────
// DataSource trait
// ──────────────────────────────────────────────

/// Data source trait — each implementation declares the metric kinds it supports.
///
/// # Distributed Deployment
///
/// In single-instance mode, use in-memory implementations (e.g. [`PerformanceDataSource`]).
/// For distributed deployments, replace with Redis-backed implementations:
/// - Writes include `instance_id` prefix to distinguish origins.
/// - Reads aggregate data across all instances:
///   - QPS: sum across instances
///   - Latency percentiles: weighted-average approximation
///   - Error rate: request-weighted average
///   - Quota: take minimum (most conservative)
pub trait DataSource: Send + Sync {
    /// Data source name (for logging / debugging).
    fn name(&self) -> &str;

    /// Data tier of this source.
    fn tier(&self) -> DataTier;

    /// Metric kinds supported by this data source.
    fn supported_metrics(&self) -> Vec<MetricKind>;

    /// Query a metric value.
    fn get(&self, kind: MetricKind, key: &str) -> Option<MetricValue>;

    /// Update a metric value.
    fn update(&self, kind: MetricKind, key: &str, value: MetricValue);

    /// Batch update. Default implementation delegates to [`update`](DataSource::update).
    fn update_batch(&self, updates: &[(MetricKind, String, MetricValue)]) {
        for (kind, key, value) in updates {
            self.update(*kind, key, value.clone());
        }
    }
}

// ──────────────────────────────────────────────
// DataLayer
// ──────────────────────────────────────────────

/// Data layer — dynamically registers [`DataSource`]s and routes queries by [`MetricKind`].
///
/// Multiple data sources may support the same `MetricKind`. Queries resolve by
/// [`DataTier`] priority (`NearRealtime > External > Static`). Within the same
/// tier, the source registered last takes precedence.
pub struct DataLayer {
    sources: RwLock<Vec<Arc<dyn DataSource>>>,
    /// MetricKind → indices of sources supporting that kind.
    index: RwLock<HashMap<MetricKind, Vec<usize>>>,
}

impl DataLayer {
    pub fn new() -> Self {
        Self {
            sources: RwLock::new(Vec::new()),
            index: RwLock::new(HashMap::new()),
        }
    }

    /// Register a data source. Automatically indexes by its [`supported_metrics`](DataSource::supported_metrics).
    pub fn register(&self, source: Arc<dyn DataSource>) {
        let mut sources = self.sources.write().unwrap();
        let idx = sources.len();
        let metrics = source.supported_metrics();
        sources.push(source);

        let mut index = self.index.write().unwrap();
        for kind in metrics {
            let entry = index.entry(kind).or_default();
            entry.push(idx);
        }
    }

    /// Query a metric value. When multiple sources support the same kind,
    /// selects by tier priority + registration order.
    pub fn query(&self, kind: MetricKind, key: &str) -> Option<MetricValue> {
        let sources = self.sources.read().unwrap();
        let index = self.index.read().unwrap();

        let indices = index.get(&kind)?;

        // Priority: NearRealtime > External > Static
        // DataTier ord: NearRealtime(0) < External(1) < Static(2)
        // So lower tier = higher priority.
        // Within the same tier, later registration (higher index) wins.
        let mut best: Option<(DataTier, usize, MetricValue)> = None;

        for &idx in indices {
            if let Some(value) = sources[idx].get(kind, key) {
                let tier = sources[idx].tier();
                match &best {
                    None => best = Some((tier, idx, value)),
                    Some((best_tier, best_idx, _)) => {
                        if tier < *best_tier || (tier == *best_tier && idx > *best_idx) {
                            best = Some((tier, idx, value));
                        }
                    }
                }
            }
        }

        best.map(|(_, _, v)| v)
    }

    /// Update a metric — writes to the highest-priority source that supports it.
    pub fn update(&self, kind: MetricKind, key: &str, value: MetricValue) {
        let sources = self.sources.read().unwrap();
        let index = self.index.read().unwrap();

        if let Some(indices) = index.get(&kind) {
            // Write to the first (earliest-registered) source for this kind.
            if let Some(&idx) = indices.first() {
                sources[idx].update(kind, key, value);
            }
        }
    }

    /// Check whether any registered source supports the given metric kind.
    pub fn has_source_for(&self, kind: MetricKind) -> bool {
        let index = self.index.read().unwrap();
        index.get(&kind).map_or(false, |v| !v.is_empty())
    }

    /// Names of all registered data sources.
    pub fn registered_sources(&self) -> Vec<String> {
        let sources = self.sources.read().unwrap();
        sources.iter().map(|s| s.name().to_string()).collect()
    }
}

impl Default for DataLayer {
    fn default() -> Self {
        Self::new()
    }
}

// ──────────────────────────────────────────────
// PerformanceDataSource
// ──────────────────────────────────────────────

/// In-memory performance data source — replaces the former `InMemoryPerformanceStore`.
pub struct PerformanceDataSource {
    data: RwLock<HashMap<String, ModelPerformance>>,
}

impl PerformanceDataSource {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for PerformanceDataSource {
    fn default() -> Self {
        Self::new()
    }
}

impl DataSource for PerformanceDataSource {
    fn name(&self) -> &str {
        "memory:performance"
    }

    fn tier(&self) -> DataTier {
        DataTier::NearRealtime
    }

    fn supported_metrics(&self) -> Vec<MetricKind> {
        vec![MetricKind::Performance]
    }

    fn get(&self, kind: MetricKind, key: &str) -> Option<MetricValue> {
        match kind {
            MetricKind::Performance => {
                let data = self.data.read().unwrap();
                data.get(key).cloned().map(MetricValue::Performance)
            }
            _ => None,
        }
    }

    fn update(&self, kind: MetricKind, key: &str, value: MetricValue) {
        if let (MetricKind::Performance, MetricValue::Performance(perf)) = (kind, value) {
            let mut data = self.data.write().unwrap();
            data.insert(key.to_string(), perf);
        }
    }

    fn update_batch(&self, updates: &[(MetricKind, String, MetricValue)]) {
        let mut data = self.data.write().unwrap();
        for (kind, key, value) in updates {
            if let (MetricKind::Performance, MetricValue::Performance(perf)) = (kind, value) {
                data.insert(key.clone(), perf.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perf_source_update_and_get() {
        let src = PerformanceDataSource::new();
        let perf = ModelPerformance {
            p50_latency_ms: 100,
            p95_latency_ms: 200,
            p99_latency_ms: 300,
            ttft_p50_ms: 50,
            throughput_tps: 10.0,
            error_rate: 0.01,
            current_qps: 5.0,
            queue_depth: 0,
            last_updated: None,
        };

        src.update(
            MetricKind::Performance,
            "v1",
            MetricValue::Performance(perf),
        );
        let got = src.get(MetricKind::Performance, "v1").unwrap();
        if let MetricValue::Performance(p) = got {
            assert_eq!(p.p50_latency_ms, 100);
            assert_eq!(p.current_qps, 5.0);
        } else {
            panic!("expected Performance ");
        }
    }

    #[test]
    fn test_perf_source_ignores_wrong_kind() {
        let src = PerformanceDataSource::new();
        let perf = ModelPerformance::default();
        // Trying to store as Pricing kind — should be ignored
        src.update(
            MetricKind::Pricing,
            "v1",
            MetricValue::Performance(perf),
        );
        assert!(src.get(MetricKind::Performance, "v1").is_none());
        assert!(src.get(MetricKind::Pricing, "v1").is_none());
    }

    #[test]
    fn test_perf_source_batch_update() {
        let src = PerformanceDataSource::new();
        let updates = vec![
            (
                MetricKind::Performance,
                "v1".to_string(),
                MetricValue::Performance(ModelPerformance {
                    p50_latency_ms: 100,
                    ..Default::default()
                }),
            ),
            (
                MetricKind::Performance,
                "v2".to_string(),
                MetricValue::Performance(ModelPerformance {
                    p50_latency_ms: 200,
                    ..Default::default()
                }),
            ),
        ];
        src.update_batch(&updates);

        if let Some(MetricValue::Performance(p)) = src.get(MetricKind::Performance, "v1") {
            assert_eq!(p.p50_latency_ms, 100);
        } else {
            panic!("v1 not found");
        }
        if let Some(MetricValue::Performance(p)) = src.get(MetricKind::Performance, "v2") {
            assert_eq!(p.p50_latency_ms, 200);
        } else {
            panic!("v2 not found");
        }
    }

    #[test]
    fn test_get_missing_returns_none() {
        let src = PerformanceDataSource::new();
        assert!(src.get(MetricKind::Performance, "nonexistent").is_none());
    }

    #[test]
    fn test_data_layer_register_and_query() {
        let layer = DataLayer::new();
        let src = Arc::new(PerformanceDataSource::new());
        layer.register(src);

        layer.update(
            MetricKind::Performance,
            "v1",
            MetricValue::Performance(ModelPerformance {
                p50_latency_ms: 42,
                ..Default::default()
            }),
        );

        let val = layer.query(MetricKind::Performance, "v1").unwrap();
        if let MetricValue::Performance(p) = val {
            assert_eq!(p.p50_latency_ms, 42);
        } else {
            panic!("expected Performance");
        }
    }

    #[test]
    fn test_data_layer_query_missing_kind() {
        let layer = DataLayer::new();
        let src = Arc::new(PerformanceDataSource::new());
        layer.register(src);
        // No Pricing source registered
        assert!(layer.query(MetricKind::Pricing, "v1").is_none());
    }

    #[test]
    fn test_data_layer_query_missing_key() {
        let layer = DataLayer::new();
        let src = Arc::new(PerformanceDataSource::new());
        layer.register(src);
        assert!(layer.query(MetricKind::Performance, "nonexistent").is_none());
    }

    #[test]
    fn test_has_source_for() {
        let layer = DataLayer::new();
        assert!(!layer.has_source_for(MetricKind::Performance));

        let src = Arc::new(PerformanceDataSource::new());
        layer.register(src);
        assert!(layer.has_source_for(MetricKind::Performance));
        assert!(!layer.has_source_for(MetricKind::Pricing));
    }

    #[test]
    fn test_registered_sources() {
        let layer = DataLayer::new();
        assert!(layer.registered_sources().is_empty());

        let src = Arc::new(PerformanceDataSource::new());
        layer.register(src);
        let names = layer.registered_sources();
        assert_eq!(names, vec!["memory:performance"]);
    }

    #[test]
    fn test_same_metric_priority_by_tier() {
        // Two sources support Performance, but with different tiers.
        // NearRealtime should win over External.
        struct ExternalPerfSource {
            data: RwLock<HashMap<String, ModelPerformance>>,
        }
        impl DataSource for ExternalPerfSource {
            fn name(&self) -> &str { "external:performance" }
            fn tier(&self) -> DataTier { DataTier::External }
            fn supported_metrics(&self) -> Vec<MetricKind> { vec![MetricKind::Performance] }
            fn get(&self, kind: MetricKind, key: &str) -> Option<MetricValue> {
                if kind == MetricKind::Performance {
                    self.data.read().unwrap().get(key).cloned().map(MetricValue::Performance)
                } else {
                    None
                }
            }
            fn update(&self, kind: MetricKind, key: &str, value: MetricValue) {
                if let (MetricKind::Performance, MetricValue::Performance(p)) = (kind, value) {
                    self.data.write().unwrap().insert(key.to_string(), p);
                }
            }
        }

        let layer = DataLayer::new();

        // Register External first
        let ext = Arc::new(ExternalPerfSource {
            data: RwLock::new(HashMap::new()),
        });
        ext.update(
            MetricKind::Performance,
            "v1",
            MetricValue::Performance(ModelPerformance {
                p50_latency_ms: 999,
                ..Default::default()
            }),
        );
        layer.register(ext);

        // Register NearRealtime second
        let nr = Arc::new(PerformanceDataSource::new());
        nr.update(
            MetricKind::Performance,
            "v1",
            MetricValue::Performance(ModelPerformance {
                p50_latency_ms: 42,
                ..Default::default()
            }),
        );
        layer.register(nr);

        // NearRealtime (tier=0) should win over External (tier=1)
        let val = layer.query(MetricKind::Performance, "v1").unwrap();
        if let MetricValue::Performance(p) = val {
            assert_eq!(p.p50_latency_ms, 42);
        } else {
            panic!("expected Performance");
        }
    }

    #[test]
    fn test_same_tier_last_registered_wins() {
        // Two NearRealtime sources for Performance — later one wins.
        struct NamedPerfSource {
            name: String,
            data: RwLock<HashMap<String, ModelPerformance>>,
        }
        impl DataSource for NamedPerfSource {
            fn name(&self) -> &str { &self.name }
            fn tier(&self) -> DataTier { DataTier::NearRealtime }
            fn supported_metrics(&self) -> Vec<MetricKind> { vec![MetricKind::Performance] }
            fn get(&self, kind: MetricKind, key: &str) -> Option<MetricValue> {
                if kind == MetricKind::Performance {
                    self.data.read().unwrap().get(key).cloned().map(MetricValue::Performance)
                } else {
                    None
                }
            }
            fn update(&self, kind: MetricKind, key: &str, value: MetricValue) {
                if let (MetricKind::Performance, MetricValue::Performance(p)) = (kind, value) {
                    self.data.write().unwrap().insert(key.to_string(), p);
                }
            }
        }

        let layer = DataLayer::new();

        let src1 = Arc::new(NamedPerfSource {
            name: "first".into(),
            data: RwLock::new(HashMap::new()),
        });
        src1.update(
            MetricKind::Performance,
            "v1",
            MetricValue::Performance(ModelPerformance {
                p50_latency_ms: 111,
                ..Default::default()
            }),
        );
        layer.register(src1);

        let src2 = Arc::new(NamedPerfSource {
            name: "second".into(),
            data: RwLock::new(HashMap::new()),
        });
        src2.update(
            MetricKind::Performance,
            "v1",
            MetricValue::Performance(ModelPerformance {
                p50_latency_ms: 222,
                ..Default::default()
            }),
        );
        layer.register(src2);

        let val = layer.query(MetricKind::Performance, "v1").unwrap();
        if let MetricValue::Performance(p) = val {
            assert_eq!(p.p50_latency_ms, 222, "later registered source should win");
        } else {
            panic!("expected Performance");
        }
    }

    #[test]
    fn test_update_routes_to_source() {
        let layer = DataLayer::new();
        let src = Arc::new(PerformanceDataSource::new());
        layer.register(src.clone());

        layer.update(
            MetricKind::Performance,
            "v1",
            MetricValue::Performance(ModelPerformance {
                p50_latency_ms: 77,
                ..Default::default()
            }),
        );

        // Verify the source received the update
        let val = src.get(MetricKind::Performance, "v1").unwrap();
        if let MetricValue::Performance(p) = val {
            assert_eq!(p.p50_latency_ms, 77);
        } else {
            panic!("expected Performance");
        }
    }
}
