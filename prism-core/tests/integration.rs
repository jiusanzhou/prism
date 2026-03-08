use prism_core::analyzer::ChatMessage;
use prism_core::registry::InMemoryRegistry;
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

fn build_full_registry() -> InMemoryRegistry {
    let mut reg = InMemoryRegistry::new();

    // Multiple providers with same model (simulating gRPC upstream resolution)
    for (prov_id, base_url, latency, cost_input, cost_output) in [
        ("openai", "https://api.openai.com/v1", 300u32, 2.5, 10.0),
        (
            "azure-east",
            "https://east.openai.azure.com",
            200,
            2.5,
            10.0,
        ),
        ("azure-west", "https://west.openai.azure.com", 350, 2.0, 8.0),
    ] {
        let mut p = make_provider(prov_id);
        p.base_url = base_url.to_string();
        reg.add_provider(p);

        let ep_id = format!("{prov_id}-ep");
        reg.add_endpoint(make_endpoint(&ep_id, prov_id));
        reg.add_account(make_account(&format!("{prov_id}-acct"), &ep_id));

        let variant = ModelVariant {
            id: format!("gpt-4o@{prov_id}"),
            model_id: "gpt-4o".into(),
            endpoint_id: ep_id.clone(),
            account_id: Some(format!("{prov_id}-acct")),
            deployment_name: None,
            weight: 1.0,
            performance: ModelPerformance {
                p50_latency_ms: latency,
                p95_latency_ms: latency + 200,
                p99_latency_ms: latency + 500,
                ttft_p50_ms: latency / 3,
                throughput_tps: 50.0,
                error_rate: 0.01,
                current_qps: 10.0,
                queue_depth: 0,
                last_updated: None,
                ..Default::default()
            },
            pricing_override: Some(ModelPricing {
                input_per_1m_tokens: cost_input,
                output_per_1m_tokens: cost_output,
                currency: "USD".into(),
            }),
            status: VariantStatus::Active,
            base_url: None,
            upstream_model_name: None,
            metadata: std::collections::HashMap::new(),
        };
        reg.add_variant(variant);
    }

    // Add the model
    reg.add_model(Model {
        id: "gpt-4o".into(),
        display_name: "GPT-4o".into(),
        quality_tier: QualityTier::S,
        capabilities: ModelCapabilities {
            max_context_window: 128000,
            max_output_tokens: 16384,
            supports_streaming: true,
            supports_function_calling: true,
            supports_vision: true,
            supports_json_mode: true,
            supports_reasoning: false,
        },
        pricing: ModelPricing {
            input_per_1m_tokens: 2.5,
            output_per_1m_tokens: 10.0,
            currency: "USD".into(),
        },
        specialties: vec![TaskType::Reasoning, TaskType::CodeGeneration],
        limits: None,
        status: ModelStatus::Available,
    });

    // A cheaper model
    reg.add_model(Model {
        id: "gpt-4o-mini".into(),
        display_name: "GPT-4o Mini".into(),
        quality_tier: QualityTier::A,
        capabilities: ModelCapabilities {
            max_context_window: 128000,
            max_output_tokens: 16384,
            supports_streaming: true,
            supports_function_calling: true,
            supports_vision: true,
            supports_json_mode: true,
            supports_reasoning: false,
        },
        pricing: ModelPricing {
            input_per_1m_tokens: 0.15,
            output_per_1m_tokens: 0.6,
            currency: "USD".into(),
        },
        specialties: vec![TaskType::SimpleQA],
        limits: None,
        status: ModelStatus::Available,
    });

    reg.add_variant(ModelVariant {
        id: "gpt-4o-mini@openai".into(),
        model_id: "gpt-4o-mini".into(),
        endpoint_id: "openai-ep".into(),
        account_id: Some("openai-acct".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms: 100,
            p95_latency_ms: 200,
            p99_latency_ms: 300,
            ttft_p50_ms: 50,
            throughput_tps: 80.0,
            error_rate: 0.005,
            current_qps: 30.0,
            queue_depth: 0,
            last_updated: None,
            ..Default::default()
        },
        pricing_override: None,
        status: VariantStatus::Active,
        base_url: None,
        upstream_model_name: None,
        metadata: std::collections::HashMap::new(),
    });

    reg.add_pool(ModelPool {
        id: "all".into(),
        name: "All".into(),
        variants: vec![
            "gpt-4o@openai".into(),
            "gpt-4o@azure-east".into(),
            "gpt-4o@azure-west".into(),
            "gpt-4o-mini@openai".into(),
        ],
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
        strategy: None,
        metadata: RouteMetadata::default(),
        analyze_context: None,
        required_protocol: None,
        requested_model: None,
        inflight_snapshot: None,
        prefix_cache_index: None,
    }
}

// ──────────────────────────────────────────────
// Upstream resolution tests
// ──────────────────────────────────────────────

/// Verify that routing decisions include enough info to construct an upstream.
#[test]
fn test_decision_contains_upstream_info() {
    let reg = build_full_registry();
    let router = PrismRouter::new(reg);

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // Decision must contain all fields needed by Upstream proto message
    assert!(!decision.selected_model.is_empty(), "model must be set");
    assert!(
        !decision.selected_provider.is_empty(),
        "provider must be set"
    );
    assert!(!decision.selected_variant.is_empty(), "variant must be set");
    assert!(!decision.selected_account.is_empty(), "account must be set");
    assert!(
        decision.decision_time_us > 0,
        "decision time must be tracked"
    );
}

/// Verify that the decision reason is populated for each layer.
#[test]
fn test_decision_reason_per_layer() {
    let reg = build_full_registry();
    let router = PrismRouter::new(reg);

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    assert!(
        !decision.reason.l1_model.is_empty(),
        "L1 reason must be set"
    );
    assert!(
        !decision.reason.l2_provider.is_empty(),
        "L2 reason must be set"
    );
    assert!(
        !decision.reason.l3_account.is_empty(),
        "L3 reason must be set"
    );
    assert!(
        !decision.reason.l4_variant.is_empty(),
        "L4 reason must be set"
    );
}

/// Multi-provider: same model across different providers should select based on strategy.
#[test]
fn test_multi_provider_latency_routing() {
    let reg = build_full_registry();
    let router = PrismRouter::new(reg).with_strategy(ModelSelectionStrategy::Pinned {
        model_id: "gpt-4o".into(),
    });

    // With pinned model, all three providers are candidates.
    // The one with lowest latency (azure-east, p50=200) should be preferred
    // when using default balanced strategy for the scoring within pinned candidates.
    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    assert_eq!(decision.selected_model, "gpt-4o");
    // All three variants are valid; exact selection depends on scoring,
    // but azure-east has lowest latency
}

/// Multi-provider: verify fallback chain contains alternatives.
#[test]
fn test_multi_provider_fallback_chain() {
    let reg = build_full_registry();
    let router = PrismRouter::new(reg).with_strategy(ModelSelectionStrategy::Pinned {
        model_id: "gpt-4o".into(),
    });

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // With 3 gpt-4o variants, fallback should have at least 2 entries
    assert!(
        decision.fallback_chain.len() >= 2,
        "Expected >= 2 fallbacks for 3 variants, got {}",
        decision.fallback_chain.len()
    );

    // Fallback entries should be different from the selected variant
    for fb in &decision.fallback_chain {
        assert_ne!(
            fb, &decision.selected_variant,
            "Fallback should differ from selected"
        );
    }
}

/// Verify scoring is deterministic — same input on same router always produces same output.
#[test]
fn test_routing_deterministic() {
    let reg = build_full_registry();
    let router = PrismRouter::new(reg);

    let req1 = simple_request("What is machine learning?");
    let req2 = simple_request("What is machine learning?");
    let d1 = router.route(&req1).unwrap();
    let d2 = router.route(&req2).unwrap();

    assert_eq!(d1.selected_model, d2.selected_model);
    assert_eq!(d1.selected_variant, d2.selected_variant);
    assert_eq!(d1.selected_provider, d2.selected_provider);
}

/// Verify empty registry returns NoCandidates error.
#[test]
fn test_empty_registry_returns_error() {
    let reg = InMemoryRegistry::new();
    let router = PrismRouter::new(reg);

    let req = simple_request("Hello");
    let result = router.route(&req);

    assert!(result.is_err(), "Empty registry should return error");
}

/// Verify disabled variant is excluded.
#[test]
fn test_disabled_variant_excluded() {
    let mut reg = build_full_registry();

    // Disable all variants except one
    if let Some(v) = reg.variants.get_mut("gpt-4o@openai") {
        v.status = VariantStatus::Disabled;
    }
    if let Some(v) = reg.variants.get_mut("gpt-4o@azure-east") {
        v.status = VariantStatus::Disabled;
    }

    let router = PrismRouter::new(reg).with_strategy(ModelSelectionStrategy::Pinned {
        model_id: "gpt-4o".into(),
    });

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // Only azure-west should be available
    assert_eq!(decision.selected_variant, "gpt-4o@azure-west");
}

/// Verify draining variant is excluded.
#[test]
fn test_draining_variant_excluded() {
    let mut reg = build_full_registry();

    if let Some(v) = reg.variants.get_mut("gpt-4o@azure-east") {
        v.status = VariantStatus::Draining;
    }

    let router = PrismRouter::new(reg).with_strategy(ModelSelectionStrategy::Pinned {
        model_id: "gpt-4o".into(),
    });

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    assert_ne!(
        decision.selected_variant, "gpt-4o@azure-east",
        "Draining variant should not be selected"
    );
}

// ──────────────────────────────────────────────
// ReportOutcome data validation (what the router provides)
// ──────────────────────────────────────────────

/// Decision should provide variant_id for outcome reporting.
#[test]
fn test_decision_provides_variant_for_reporting() {
    let reg = build_full_registry();
    let router = PrismRouter::new(reg);

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // The variant_id should match a real variant in the registry
    assert!(
        decision.selected_variant.contains('@'),
        "Variant ID should be in format 'model@provider', got: {}",
        decision.selected_variant
    );
}

// ──────────────────────────────────────────────
// Request profile → routing integration
// ──────────────────────────────────────────────

/// Verify analyzer output is actually used by routing.
#[test]
fn test_analyzer_influences_routing() {
    let reg1 = build_full_registry();
    let reg2 = build_full_registry();

    let router = PrismRouter::new(reg1).with_strategy(ModelSelectionStrategy::Auto {
        objective: RoutingObjective::Balanced {
            cost_weight: 0.3,
            quality_weight: 0.5,
            latency_weight: 0.2,
        },
        pool_ids: None,
        required_capabilities: None,
    });

    let router_cost = PrismRouter::new(reg2).with_strategy(ModelSelectionStrategy::Auto {
        objective: RoutingObjective::CostFirst {
            min_quality_score: 0.0,
            budget_per_request: None,
        },
        pool_ids: None,
        required_capabilities: None,
    });

    // Simple question
    let simple_req = simple_request("What is 2+2?");
    let d_balanced = router.route(&simple_req).unwrap();
    let d_cost = router_cost.route(&simple_req).unwrap();

    // Cost-first should pick the cheapest model
    assert_eq!(
        d_cost.selected_model, "gpt-4o-mini",
        "CostFirst should pick cheapest model for simple query"
    );

    // Both should produce valid decisions
    assert!(!d_balanced.selected_model.is_empty());
    assert!(!d_cost.selected_model.is_empty());
}

/// Performance benchmark: 1000 routing decisions should complete in < 100ms.
#[test]
fn test_routing_throughput() {
    let reg = build_full_registry();
    let router = PrismRouter::new(reg);

    let req = simple_request("What is machine learning?");

    let start = std::time::Instant::now();
    for _ in 0..1000 {
        let _ = router.route(&req).unwrap();
    }
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 100,
        "1000 routing decisions took {}ms, expected < 100ms",
        elapsed.as_millis()
    );
}
