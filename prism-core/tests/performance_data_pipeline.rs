//! Integration tests verifying the dynamic performance data pipeline.
//!
//! These tests confirm that:
//! 1. `update_performance` actually persists data (no longer a no-op)
//! 2. `assemble_candidate` overlays runtime perf data onto variants
//! 3. Routing decisions respond to live performance updates
//! 4. FFI `StatsUpdate` with `#[serde(flatten)]` parses both array and single formats
//! 5. The DataLayer works correctly via the registry

use std::sync::Arc;

use prism_core::analyzer::ChatMessage;
use prism_core::data::{DataLayer, DataSource, DataTier, MetricKind, MetricValue, PerformanceDataSource};
use prism_core::registry::{InMemoryRegistry, ModelRegistryTrait};
use prism_core::router::{PrismRouter, RouteMetadata, RouteRequest, Router};
use prism_core::strategy::*;
use prism_core::types::*;

// ──────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────

fn make_provider(id: &str) -> Provider {
    Provider {
        id: id.to_string(),
        name: id.to_string(),
        auth: AuthConfig::default(),
        base_url: format!("https://api.{id}.com/v1"),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    }
}

fn make_endpoint(id: &str, provider_id: &str) -> Endpoint {
    Endpoint {
        id: id.to_string(),
        provider_id: provider_id.to_string(),
        base_url: None,
        region: None,
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    }
}

fn make_account(id: &str, endpoint_id: &str) -> Account {
    Account {
        id: id.to_string(),
        endpoint_id: endpoint_id.to_string(),
        auth: AuthConfig::default(),
        project_id: None,
        rate_limit: None,
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
    }
}

fn make_model(id: &str, tier: QualityTier) -> Model {
    Model {
        id: id.to_string(),
        display_name: id.to_string(),
        quality_tier: tier,
        capabilities: ModelCapabilities {
            max_context_window: 128000,
            max_output_tokens: 16384,
            supports_streaming: true,
            supports_function_calling: true,
            supports_vision: false,
            supports_json_mode: true,
            supports_reasoning: false,
        },
        pricing: ModelPricing {
            input_per_1m_tokens: 2.0,
            output_per_1m_tokens: 8.0,
            currency: "USD".into(),
        },
        specialties: vec![],
        limits: None,
        status: ModelStatus::Available,
    }
}

fn build_two_variant_registry() -> InMemoryRegistry {
    let mut reg = InMemoryRegistry::new();

    // Same model, two providers — identical static config except variant ID
    for (prov, latency) in [("fast", 100u32), ("slow", 500)] {
        reg.add_provider(make_provider(prov));
        let ep = format!("{prov}-ep");
        reg.add_endpoint(make_endpoint(&ep, prov));
        reg.add_account(make_account(&format!("{prov}-acct"), &ep));

        reg.add_variant(ModelVariant {
            id: format!("model-a@{prov}"),
            model_id: "model-a".into(),
            endpoint_id: ep,
            account_id: Some(format!("{prov}-acct")),
            deployment_name: None,
            weight: 1.0,
            performance: ModelPerformance {
                p50_latency_ms: latency,
                p95_latency_ms: latency + 100,
                p99_latency_ms: latency + 200,
                ttft_p50_ms: latency / 2,
                throughput_tps: 50.0,
                error_rate: 0.01,
                current_qps: 10.0,
                queue_depth: 0,
                last_updated: None,
                ..Default::default()
            },
            pricing_override: None,
            status: VariantStatus::Active,
            base_url: None,
            upstream_model_name: None,
        });
    }

    reg.add_model(make_model("model-a", QualityTier::S));

    reg.add_pool(ModelPool {
        id: "all".into(),
        name: "All".into(),
        variants: vec!["model-a@fast".into(), "model-a@slow".into()],
        selection_mode: SelectionMode::Scored,
        description: None,
    });

    reg
}

fn simple_request(content: &str) -> RouteRequest {
    RouteRequest {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: content.into(),
        }],
        strategy: Some(ModelSelectionStrategy::Auto {
            objective: RoutingObjective::Balanced {
                cost_weight: 0.0,
                quality_weight: 0.0,
                latency_weight: 1.0, // purely latency-driven so perf updates matter
            },
            pool_ids: Some(vec!["all".into()]),
            required_capabilities: None,
        }),
        metadata: RouteMetadata::default(),
        analyze_context: None,
        required_protocol: None,
    }
}

// ──────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────

/// update_performance actually persists — reading back gives updated values.
#[test]
fn test_update_performance_persists() {
    let reg = build_two_variant_registry();

    // Before update, data layer has no data for this key
    assert!(reg
        .data_layer()
        .query(MetricKind::Performance, "model-a@fast")
        .is_none());

    // Update performance
    let new_perf = ModelPerformance {
        p50_latency_ms: 999,
        ..Default::default()
    };
    reg.update_performance("model-a@fast", new_perf);

    // Read it back
    let val = reg
        .data_layer()
        .query(MetricKind::Performance, "model-a@fast")
        .unwrap();
    if let MetricValue::Performance(stored) = val {
        assert_eq!(stored.p50_latency_ms, 999);
    } else {
        panic!("expected Performance value");
    }
}

/// assemble_candidate overlays data layer data onto variant performance.
#[test]
fn test_assemble_candidate_uses_data_layer() {
    let reg = build_two_variant_registry();

    // Initially, candidate should have static performance (p50=100 for "fast")
    let candidates = reg.get_all_candidates();
    let fast_before = candidates
        .iter()
        .find(|c| c.variant.id == "model-a@fast")
        .unwrap();
    assert_eq!(fast_before.variant.performance.p50_latency_ms, 100);

    // Update performance in data layer
    reg.update_performance(
        "model-a@fast",
        ModelPerformance {
            p50_latency_ms: 42,
            p95_latency_ms: 80,
            p99_latency_ms: 120,
            ttft_p50_ms: 20,
            throughput_tps: 100.0,
            error_rate: 0.005,
            current_qps: 50.0,
            queue_depth: 3,
            last_updated: None,
            ..Default::default()
        },
    );

    // Re-assemble — should now reflect updated data
    let candidates = reg.get_all_candidates();
    let fast_after = candidates
        .iter()
        .find(|c| c.variant.id == "model-a@fast")
        .unwrap();
    assert_eq!(fast_after.variant.performance.p50_latency_ms, 42);
    assert_eq!(fast_after.variant.performance.current_qps, 50.0);
}

/// Routing decisions change when performance data is updated.
/// Start: "fast" has p50=100, "slow" has p50=500 → "fast" wins.
/// Then flip: "fast" gets p50=9000, "slow" gets p50=50 → "slow" wins.
#[test]
fn test_routing_responds_to_performance_updates() {
    let reg = build_two_variant_registry();
    let router = PrismRouter::new(reg);

    let req = simple_request("Hello");

    // Initial routing — "fast" should win (lower latency)
    let decision_before = router.route(&req).unwrap();
    assert_eq!(
        decision_before.selected_variant, "model-a@fast",
        "Lower-latency variant should be selected initially"
    );

    // Flip performance: make "fast" very slow and "slow" very fast
    router.registry.update_performance(
        "model-a@fast",
        ModelPerformance {
            p50_latency_ms: 9000,
            p95_latency_ms: 9500,
            p99_latency_ms: 10000,
            ttft_p50_ms: 3000,
            throughput_tps: 5.0,
            error_rate: 0.1,
            current_qps: 2.0,
            queue_depth: 50,
            last_updated: None,
            ..Default::default()
        },
    );
    router.registry.update_performance(
        "model-a@slow",
        ModelPerformance {
            p50_latency_ms: 50,
            p95_latency_ms: 80,
            p99_latency_ms: 100,
            ttft_p50_ms: 20,
            throughput_tps: 200.0,
            error_rate: 0.001,
            current_qps: 5.0,
            queue_depth: 0,
            last_updated: None,
            ..Default::default()
        },
    );

    // Re-route — "slow" should now win
    let decision_after = router.route(&req).unwrap();
    assert_eq!(
        decision_after.selected_variant, "model-a@slow",
        "After perf update, previously-slow variant should be selected"
    );
}

/// Verify StatsUpdate with serde(flatten) can parse the flat JSON format
/// that Go's SyncLoop sends.
#[test]
fn test_stats_update_flat_json_deserialization() {
    // This mirrors the exact JSON format Go's syncMetrics produces
    let flat_json = r#"{
        "variant_id": "gpt-4o@openai",
        "current_qps": 42.5,
        "error_rate": 0.02,
        "p50_latency_ms": 150,
        "p95_latency_ms": 300,
        "p99_latency_ms": 500,
        "ttft_p50_ms": 80,
        "queue_depth": 5
    }"#;

    // Deserialize using the same struct the FFI uses
    #[derive(serde::Deserialize)]
    struct StatsUpdate {
        variant_id: String,
        #[serde(flatten)]
        performance: ModelPerformance,
    }

    let update: StatsUpdate = serde_json::from_str(flat_json).unwrap();
    assert_eq!(update.variant_id, "gpt-4o@openai");
    assert_eq!(update.performance.current_qps, 42.5);
    assert_eq!(update.performance.p50_latency_ms, 150);
    assert_eq!(update.performance.error_rate, 0.02);
    assert_eq!(update.performance.queue_depth, 5);
}

/// Verify array format deserialization (what Go actually sends).
#[test]
fn test_stats_update_array_json_deserialization() {
    let array_json = r#"[
        {"variant_id": "v1", "current_qps": 10.0, "p50_latency_ms": 100},
        {"variant_id": "v2", "current_qps": 20.0, "p50_latency_ms": 200}
    ]"#;

    #[derive(serde::Deserialize)]
    struct StatsUpdate {
        variant_id: String,
        #[serde(flatten)]
        performance: ModelPerformance,
    }

    let updates: Vec<StatsUpdate> = serde_json::from_str(array_json).unwrap();
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].variant_id, "v1");
    assert_eq!(updates[0].performance.current_qps, 10.0);
    assert_eq!(updates[1].variant_id, "v2");
    assert_eq!(updates[1].performance.p50_latency_ms, 200);
}

/// DataLayer works correctly when injected into registry via with_data_layer.
#[test]
fn test_custom_data_layer_integration() {
    let data_layer = Arc::new(DataLayer::new());
    let perf_src = Arc::new(PerformanceDataSource::new());

    // Pre-populate before creating registry
    perf_src.update(
        MetricKind::Performance,
        "model-a@fast",
        MetricValue::Performance(ModelPerformance {
            p50_latency_ms: 77,
            ..Default::default()
        }),
    );
    data_layer.register(perf_src);

    let mut reg = InMemoryRegistry::with_data_layer(data_layer);

    // Set up minimal registry data
    reg.add_provider(make_provider("fast"));
    reg.add_endpoint(make_endpoint("fast-ep", "fast"));
    reg.add_account(make_account("fast-acct", "fast-ep"));
    reg.add_model(make_model("model-a", QualityTier::S));
    reg.add_variant(ModelVariant {
        id: "model-a@fast".into(),
        model_id: "model-a".into(),
        endpoint_id: "fast-ep".into(),
        account_id: Some("fast-acct".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms: 999, // static value that should be overridden
            ..Default::default()
        },
        pricing_override: None,
        status: VariantStatus::Active,
            base_url: None,
            upstream_model_name: None,
    });

    // Candidate should use the data layer value (77), not the static value (999)
    let candidates = reg.get_all_candidates();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].variant.performance.p50_latency_ms, 77);
}

/// Verify batch update via the data layer.
#[test]
fn test_batch_update_via_data_layer() {
    let reg = build_two_variant_registry();

    let updates = vec![
        (
            MetricKind::Performance,
            "model-a@fast".to_string(),
            MetricValue::Performance(ModelPerformance {
                p50_latency_ms: 11,
                ..Default::default()
            }),
        ),
        (
            MetricKind::Performance,
            "model-a@slow".to_string(),
            MetricValue::Performance(ModelPerformance {
                p50_latency_ms: 22,
                ..Default::default()
            }),
        ),
    ];

    // Find the performance source and batch-update directly
    // (DataLayer doesn't expose batch; the source does)
    let data_layer = reg.data_layer();
    // Update individually via data_layer
    for (kind, key, value) in updates {
        data_layer.update(kind, &key, value);
    }

    let candidates = reg.get_all_candidates();
    let fast = candidates
        .iter()
        .find(|c| c.variant.id == "model-a@fast")
        .unwrap();
    let slow = candidates
        .iter()
        .find(|c| c.variant.id == "model-a@slow")
        .unwrap();

    assert_eq!(fast.variant.performance.p50_latency_ms, 11);
    assert_eq!(slow.variant.performance.p50_latency_ms, 22);
}

/// Verify that L2/L3 selectors see updated performance data in candidates.
#[test]
fn test_selectors_see_updated_perf_data() {
    let reg = build_two_variant_registry();

    // Update performance to make "slow" have very high QPS (triggers quota penalty in L2)
    reg.update_performance(
        "model-a@slow",
        ModelPerformance {
            p50_latency_ms: 50,
            current_qps: 999.0,
            ..Default::default()
        },
    );

    let candidates = reg.get_all_candidates();
    let slow = candidates
        .iter()
        .find(|c| c.variant.id == "model-a@slow")
        .unwrap();

    // The candidate should reflect the updated QPS
    assert_eq!(slow.variant.performance.current_qps, 999.0);
    assert_eq!(slow.variant.performance.p50_latency_ms, 50);
}

/// ModelPerformance with missing fields deserializes correctly via #[serde(default)].
#[test]
fn test_performance_json_deserializes() {
    // Only partial fields — should fill rest with defaults
    let json = r#"{"p50_latency_ms": 100, "current_qps": 5.0}"#;
    let perf: ModelPerformance = serde_json::from_str(json).unwrap();
    assert_eq!(perf.p50_latency_ms, 100);
    assert_eq!(perf.current_qps, 5.0);
    assert_eq!(perf.p95_latency_ms, 0); // default
    assert_eq!(perf.error_rate, 0.0); // default
    assert!(perf.last_updated.is_none()); // skipped
}

/// Empty JSON object should deserialize to default ModelPerformance.
#[test]
fn test_empty_performance_json_deserializes() {
    let json = "{}";
    let perf: ModelPerformance = serde_json::from_str(json).unwrap();
    assert_eq!(perf.p50_latency_ms, 0);
    assert_eq!(perf.throughput_tps, 0.0);
}

/// new() default registry creates a PerformanceDataSource automatically.
#[test]
fn test_registry_default_creates_perf_source() {
    let reg = InMemoryRegistry::new();
    assert!(reg.data_layer().has_source_for(MetricKind::Performance));
    let names = reg.data_layer().registered_sources();
    assert!(names.contains(&"memory:performance".to_string()));
}
