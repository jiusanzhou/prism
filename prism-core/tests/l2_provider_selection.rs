use prism_core::analyzer::ChatMessage;
use prism_core::registry::InMemoryRegistry;
use prism_core::router::{PrismRouter, RouteMetadata, RouteRequest, Router};
use prism_core::strategy::*;
use prism_core::types::*;

// ──────────────────────────────────────────────
// Helpers — same model on multiple providers
// ──────────────────────────────────────────────

fn build_multi_provider_registry() -> InMemoryRegistry {
    let mut reg = InMemoryRegistry::new();

    reg.add_provider(Provider {
        id: "provider-fast".into(),
        name: "Fast Provider".into(),
        auth: AuthConfig::default(),
        base_url: "https://fast.example.com".into(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    });
    reg.add_provider(Provider {
        id: "provider-cheap".into(),
        name: "Cheap Provider".into(),
        auth: AuthConfig::default(),
        base_url: "https://cheap.example.com".into(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    });
    reg.add_provider(Provider {
        id: "provider-down".into(),
        name: "Down Provider".into(),
        auth: AuthConfig::default(),
        base_url: "https://down.example.com".into(),
        rate_limit: None,
        status: HealthStatus::Down,
        metadata: Default::default(),
        protocols: vec![],
    });

    reg.add_endpoint(Endpoint {
        id: "fast-ep".into(),
        provider_id: "provider-fast".into(),
        base_url: None,
        region: Some("us-east-1".into()),
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    });
    reg.add_endpoint(Endpoint {
        id: "cheap-ep".into(),
        provider_id: "provider-cheap".into(),
        base_url: None,
        region: Some("eu-west-1".into()),
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    });
    reg.add_endpoint(Endpoint {
        id: "down-ep".into(),
        provider_id: "provider-down".into(),
        base_url: None,
        region: None,
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Down,
    });

    reg.add_account(Account {
        id: "fast-acct".into(),
        endpoint_id: "fast-ep".into(),
        auth: AuthConfig::default(),
        project_id: None,
        rate_limit: None,
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
    });
    reg.add_account(Account {
        id: "cheap-acct".into(),
        endpoint_id: "cheap-ep".into(),
        auth: AuthConfig::default(),
        project_id: None,
        rate_limit: None,
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
    });
    reg.add_account(Account {
        id: "down-acct".into(),
        endpoint_id: "down-ep".into(),
        auth: AuthConfig::default(),
        project_id: None,
        rate_limit: None,
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
    });

    let model = Model {
        id: "claude-sonnet".into(),
        display_name: "Claude Sonnet".into(),
        quality_tier: QualityTier::S,
        capabilities: ModelCapabilities {
            max_context_window: 200000,
            max_output_tokens: 16384,
            supports_streaming: true,
            supports_function_calling: true,
            supports_vision: true,
            supports_json_mode: true,
            supports_reasoning: true,
        },
        pricing: ModelPricing {
            input_per_1m_tokens: 3.0,
            output_per_1m_tokens: 15.0,
            currency: "USD".into(),
        },
        specialties: vec![TaskType::Reasoning, TaskType::Creative],
        limits: None,
        status: ModelStatus::Available,
    };
    reg.add_model(model);

    reg.add_variant(ModelVariant {
        id: "claude@fast".into(),
        model_id: "claude-sonnet".into(),
        endpoint_id: "fast-ep".into(),
        account_id: Some("fast-acct".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms: 200,
            p95_latency_ms: 400,
            p99_latency_ms: 600,
            ttft_p50_ms: 80,
            throughput_tps: 60.0,
            error_rate: 0.01,
            current_qps: 20.0,
            queue_depth: 0,
            last_updated: None,
            ..Default::default()
        },
        pricing_override: None,
        status: VariantStatus::Active,
        base_url: None,
        upstream_model_name: None,
    });

    reg.add_variant(ModelVariant {
        id: "claude@cheap".into(),
        model_id: "claude-sonnet".into(),
        endpoint_id: "cheap-ep".into(),
        account_id: Some("cheap-acct".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms: 500,
            p95_latency_ms: 900,
            p99_latency_ms: 1500,
            ttft_p50_ms: 200,
            throughput_tps: 30.0,
            error_rate: 0.02,
            current_qps: 5.0,
            queue_depth: 0,
            last_updated: None,
            ..Default::default()
        },
        pricing_override: Some(ModelPricing {
            input_per_1m_tokens: 1.5,
            output_per_1m_tokens: 7.5,
            currency: "USD".into(),
        }),
        status: VariantStatus::Active,
        base_url: None,
        upstream_model_name: None,
    });

    reg.add_variant(ModelVariant {
        id: "claude@down".into(),
        model_id: "claude-sonnet".into(),
        endpoint_id: "down-ep".into(),
        account_id: Some("down-acct".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance::default(),
        pricing_override: None,
        status: VariantStatus::Disabled,
        base_url: None,
        upstream_model_name: None,
    });

    reg.add_pool(ModelPool {
        id: "all".into(),
        name: "All".into(),
        variants: vec![
            "claude@fast".into(),
            "claude@cheap".into(),
            "claude@down".into(),
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
    }
}

// ──────────────────────────────────────────────
// L2 Tests
// ──────────────────────────────────────────────

#[test]
fn test_lowest_latency_provider() {
    let reg = build_multi_provider_registry();
    let router = PrismRouter::new(reg).with_strategy(ModelSelectionStrategy::Auto {
        objective: RoutingObjective::Balanced {
            cost_weight: 0.0,
            quality_weight: 0.1,
            latency_weight: 0.9,
        },
        pool_ids: None,
        required_capabilities: None,
    });

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();
    assert_eq!(decision.selected_variant, "claude@fast");
}

#[test]
fn test_lowest_cost_provider() {
    let reg = build_multi_provider_registry();
    let router = PrismRouter::new(reg).with_strategy(ModelSelectionStrategy::Auto {
        objective: RoutingObjective::CostFirst {
            min_quality_score: 0.0,
            budget_per_request: None,
        },
        pool_ids: None,
        required_capabilities: None,
    });

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();
    assert_eq!(decision.selected_variant, "claude@cheap");
}

#[test]
fn test_unavailable_provider_skipped() {
    let reg = build_multi_provider_registry();
    let router = PrismRouter::new(reg);

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();
    assert_ne!(decision.selected_variant, "claude@down");
}

#[test]
fn test_quota_aware_selection() {
    let mut reg = InMemoryRegistry::new();

    reg.add_provider(Provider {
        id: "provider-fast".into(),
        name: "Fast Provider".into(),
        auth: AuthConfig::default(),
        base_url: "https://fast.example.com".into(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    });
    reg.add_provider(Provider {
        id: "provider-cheap".into(),
        name: "Cheap Provider".into(),
        auth: AuthConfig::default(),
        base_url: "https://cheap.example.com".into(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    });

    reg.add_endpoint(Endpoint {
        id: "fast-ep".into(),
        provider_id: "provider-fast".into(),
        base_url: None,
        region: None,
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    });
    reg.add_endpoint(Endpoint {
        id: "cheap-ep".into(),
        provider_id: "provider-cheap".into(),
        base_url: None,
        region: None,
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    });

    // Fast account: rate limit 100 RPM
    reg.add_account(Account {
        id: "fast-acct".into(),
        endpoint_id: "fast-ep".into(),
        auth: AuthConfig::default(),
        project_id: None,
        rate_limit: Some(RateLimit {
            rpm: Some(100),
            tpm: None,
            concurrent: None,
        }),
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
    });
    // Cheap account: rate limit 100 RPM
    reg.add_account(Account {
        id: "cheap-acct".into(),
        endpoint_id: "cheap-ep".into(),
        auth: AuthConfig::default(),
        project_id: None,
        rate_limit: Some(RateLimit {
            rpm: Some(100),
            tpm: None,
            concurrent: None,
        }),
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
    });

    reg.add_model(Model {
        id: "claude-sonnet".into(),
        display_name: "Claude Sonnet".into(),
        quality_tier: QualityTier::S,
        capabilities: ModelCapabilities {
            max_context_window: 200000,
            max_output_tokens: 16384,
            supports_streaming: true,
            supports_function_calling: true,
            supports_vision: true,
            supports_json_mode: true,
            supports_reasoning: true,
        },
        pricing: ModelPricing {
            input_per_1m_tokens: 3.0,
            output_per_1m_tokens: 15.0,
            currency: "USD".into(),
        },
        specialties: vec![],
        limits: None,
        status: ModelStatus::Available,
    });

    // Fast variant: current_qps = 1.5 (~90% of 100 RPM = 1.667 RPS)
    reg.add_variant(ModelVariant {
        id: "claude@fast".into(),
        model_id: "claude-sonnet".into(),
        endpoint_id: "fast-ep".into(),
        account_id: Some("fast-acct".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms: 200,
            p95_latency_ms: 400,
            p99_latency_ms: 600,
            ttft_p50_ms: 80,
            throughput_tps: 60.0,
            error_rate: 0.01,
            current_qps: 1.5,
            queue_depth: 0,
            last_updated: None,
            ..Default::default()
        },
        pricing_override: None,
        status: VariantStatus::Active,
        base_url: None,
        upstream_model_name: None,
    });

    // Cheap variant: current_qps = 0.08 (~5% of limit)
    reg.add_variant(ModelVariant {
        id: "claude@cheap".into(),
        model_id: "claude-sonnet".into(),
        endpoint_id: "cheap-ep".into(),
        account_id: Some("cheap-acct".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms: 200,
            p95_latency_ms: 400,
            p99_latency_ms: 600,
            ttft_p50_ms: 80,
            throughput_tps: 60.0,
            error_rate: 0.01,
            current_qps: 0.08,
            queue_depth: 0,
            last_updated: None,
            ..Default::default()
        },
        pricing_override: None,
        status: VariantStatus::Active,
        base_url: None,
        upstream_model_name: None,
    });

    reg.add_pool(ModelPool {
        id: "all".into(),
        name: "All".into(),
        variants: vec!["claude@fast".into(), "claude@cheap".into()],
        selection_mode: SelectionMode::Scored,
        description: None,
    });

    let router = PrismRouter::new(reg);
    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // Quota-aware: cheap has 5% utilisation vs fast at 90% → prefer cheap
    assert_eq!(decision.selected_variant, "claude@cheap");
}

#[test]
fn test_geo_proximity_selection() {
    let reg = build_multi_provider_registry();
    let router = PrismRouter::new(reg);

    // Request with region = "us-east-1" → prefer provider-fast (same region)
    let req = RouteRequest {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "Hello".into(),
        }],
        strategy: None,
        metadata: RouteMetadata {
            region: Some("us-east-1".into()),
            ..Default::default()
        },
        analyze_context: None,
        required_protocol: None,
        requested_model: None,
    };
    let decision = router.route(&req).unwrap();
    assert_eq!(decision.selected_variant, "claude@fast");

    // Request with region = "eu-west-1" → prefer provider-cheap (same region)
    let req_eu = RouteRequest {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "Hello".into(),
        }],
        strategy: None,
        metadata: RouteMetadata {
            region: Some("eu-west-1".into()),
            ..Default::default()
        },
        analyze_context: None,
        required_protocol: None,
        requested_model: None,
    };
    let decision_eu = router.route(&req_eu).unwrap();
    assert_eq!(decision_eu.selected_variant, "claude@cheap");
}
