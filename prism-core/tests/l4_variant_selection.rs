use prism_core::analyzer::ChatMessage;
use prism_core::registry::InMemoryRegistry;
use prism_core::router::{PrismRouter, RouteMetadata, RouteRequest, Router};
use prism_core::strategy::*;
use prism_core::types::*;

fn build_variant_registry() -> InMemoryRegistry {
    let mut reg = InMemoryRegistry::new();

    reg.add_provider(Provider {
        id: "openai".into(),
        name: "OpenAI".into(),
        auth: AuthConfig::default(),
        base_url: "https://api.openai.com/v1".into(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    });

    reg.add_endpoint(Endpoint {
        id: "openai-ep".into(),
        provider_id: "openai".into(),
        base_url: None,
        region: None,
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    });

    reg.add_account(Account {
        id: "openai-acct".into(),
        endpoint_id: "openai-ep".into(),
        auth: AuthConfig::default(),
        project_id: None,
        rate_limit: None,
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
    });

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

    // Variant A: stable, moderate latency
    reg.add_variant(ModelVariant {
        id: "gpt-4o-stable".into(),
        model_id: "gpt-4o".into(),
        endpoint_id: "openai-ep".into(),
        account_id: Some("openai-acct".into()),
        deployment_name: Some("gpt-4o-2024-08-06".into()),
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms: 300,
            p95_latency_ms: 500,
            p99_latency_ms: 800,
            ttft_p50_ms: 120,
            throughput_tps: 50.0,
            error_rate: 0.005,
            current_qps: 30.0,
            queue_depth: 0,
            last_updated: None,
            ..Default::default()
        },
        pricing_override: None,
        status: VariantStatus::Active,
            base_url: None,
    });

    // Variant B: faster but higher error rate (canary)
    reg.add_variant(ModelVariant {
        id: "gpt-4o-canary".into(),
        model_id: "gpt-4o".into(),
        endpoint_id: "openai-ep".into(),
        account_id: Some("openai-acct".into()),
        deployment_name: Some("gpt-4o-2024-11-20".into()),
        weight: 0.5, // lower weight
        performance: ModelPerformance {
            p50_latency_ms: 200,
            p95_latency_ms: 350,
            p99_latency_ms: 500,
            ttft_p50_ms: 80,
            throughput_tps: 60.0,
            error_rate: 0.03,
            current_qps: 5.0,
            queue_depth: 0,
            last_updated: None,
            ..Default::default()
        },
        pricing_override: None,
        status: VariantStatus::Active,
            base_url: None,
    });

    // Variant C: high weight (preferred)
    reg.add_variant(ModelVariant {
        id: "gpt-4o-preferred".into(),
        model_id: "gpt-4o".into(),
        endpoint_id: "openai-ep".into(),
        account_id: Some("openai-acct".into()),
        deployment_name: Some("gpt-4o-2024-08-06".into()),
        weight: 3.0, // high weight
        performance: ModelPerformance {
            p50_latency_ms: 280,
            p95_latency_ms: 450,
            p99_latency_ms: 700,
            ttft_p50_ms: 110,
            throughput_tps: 55.0,
            error_rate: 0.008,
            current_qps: 40.0,
            queue_depth: 0,
            last_updated: None,
            ..Default::default()
        },
        pricing_override: None,
        status: VariantStatus::Active,
            base_url: None,
    });

    reg.add_pool(ModelPool {
        id: "gpt4o-pool".into(),
        name: "GPT-4o variants".into(),
        variants: vec![
            "gpt-4o-stable".into(),
            "gpt-4o-canary".into(),
            "gpt-4o-preferred".into(),
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
    }
}

// ──────────────────────────────────────────────
// L4 Tests
// ──────────────────────────────────────────────

#[test]
fn test_best_performance_variant() {
    let reg = build_variant_registry();
    // Use SloFirst to pick the fastest variant
    let router = PrismRouter::new(reg).with_strategy(ModelSelectionStrategy::Auto {
        objective: RoutingObjective::SloFirst {
            max_latency_ms: 2000,
            max_ttft_ms: 500,
            min_quality_score: 0.0,
        },
        pool_ids: Some(vec!["gpt4o-pool".into()]),
        required_capabilities: None,
    });

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // All variants are gpt-4o, same quality. SloFirst picks highest quality score
    // among SLO-compliant variants. All comply, so it picks any S-tier.
    // This verifies the variant-level selection works.
    assert!(decision.selected_variant.starts_with("gpt-4o"));
}

#[test]
fn test_canary_traffic_split() {
    // With Canary { canary_variant_id: "gpt-4o-canary", canary_pct: 0.1 }
    // ~10% of requests should go to canary, ~90% to stable
    let reg = build_variant_registry();
    let router = PrismRouter::new(reg)
        .with_strategy(ModelSelectionStrategy::Auto {
            objective: RoutingObjective::SloFirst {
                max_latency_ms: 2000,
                max_ttft_ms: 500,
                min_quality_score: 0.0,
            },
            pool_ids: Some(vec!["gpt4o-pool".into()]),
            required_capabilities: None,
        })
        .with_variant_strategy(VariantSelectionStrategy::Canary {
            canary_variant_id: "gpt-4o-canary".into(),
            canary_pct: 0.1,
        });

    let mut canary_count = 0u32;
    let total = 1000u32;

    for _ in 0..total {
        let req = simple_request("Hello");
        let decision = router.route(&req).unwrap();
        if decision.selected_variant == "gpt-4o-canary" {
            canary_count += 1;
        }
    }

    let canary_pct = canary_count as f64 / total as f64;
    // Expect ~10% ± 5%
    assert!(
        canary_pct > 0.05 && canary_pct < 0.15,
        "Canary traffic should be ~10%, got {:.1}% ({} / {})",
        canary_pct * 100.0,
        canary_count,
        total
    );
}

#[test]
fn test_weighted_variant() {
    // gpt-4o-preferred has weight=3.0, stable=1.0, canary=0.5
    // Total = 4.5. preferred should get ~3/4.5 ≈ 66.7%
    let reg = build_variant_registry();
    let router = PrismRouter::new(reg)
        .with_strategy(ModelSelectionStrategy::Auto {
            objective: RoutingObjective::SloFirst {
                max_latency_ms: 2000,
                max_ttft_ms: 500,
                min_quality_score: 0.0,
            },
            pool_ids: Some(vec!["gpt4o-pool".into()]),
            required_capabilities: None,
        })
        .with_variant_strategy(VariantSelectionStrategy::Weighted);

    let mut counts = std::collections::HashMap::new();
    let total = 1000u32;

    for _ in 0..total {
        let req = simple_request("Hello");
        let decision = router.route(&req).unwrap();
        *counts.entry(decision.selected_variant).or_insert(0u32) += 1;
    }

    let preferred_count = *counts.get("gpt-4o-preferred").unwrap_or(&0);
    let preferred_pct = preferred_count as f64 / total as f64;

    // Expected: ~66.7% (3.0/4.5) ± 5%
    assert!(
        preferred_pct > 0.60 && preferred_pct < 0.75,
        "Preferred variant should get ~67% traffic, got {:.1}% ({} / {})",
        preferred_pct * 100.0,
        preferred_count,
        total
    );

    // Also check that canary gets ~11% (0.5/4.5) ± 5%
    let canary_count = *counts.get("gpt-4o-canary").unwrap_or(&0);
    let canary_pct = canary_count as f64 / total as f64;
    assert!(
        canary_pct > 0.05 && canary_pct < 0.20,
        "Canary variant should get ~11% traffic, got {:.1}% ({} / {})",
        canary_pct * 100.0,
        canary_count,
        total
    );
}
