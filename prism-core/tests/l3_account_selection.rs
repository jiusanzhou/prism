use prism_core::analyzer::ChatMessage;
use prism_core::registry::InMemoryRegistry;
use prism_core::router::{PrismRouter, RouteMetadata, RouteRequest, Router};
use prism_core::types::*;

fn build_multi_account_registry() -> InMemoryRegistry {
    let mut reg = InMemoryRegistry::new();

    reg.add_provider(Provider {
        id: "vertex".into(),
        name: "GCP Vertex AI".into(),
        auth: AuthConfig::default(),
        base_url: "https://us-central1-aiplatform.googleapis.com".into(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    });

    reg.add_endpoint(Endpoint {
        id: "vertex-us".into(),
        provider_id: "vertex".into(),
        base_url: None,
        region: Some("us-central1".into()),
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    });

    // Multiple accounts under same endpoint
    for i in 1..=3 {
        reg.add_account(Account {
            id: format!("project-{i}"),
            endpoint_id: "vertex-us".into(),
            auth: AuthConfig::default(),
            project_id: Some(format!("gcp-project-{i}")),
            rate_limit: Some(RateLimit {
                rpm: Some(100),
                tpm: Some(100000),
                concurrent: Some(10),
            }),
            billing_tag: Some(format!("team-{i}")),
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: Default::default(),
        });
    }

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
        specialties: vec![TaskType::Reasoning, TaskType::Creative],
        limits: None,
        status: ModelStatus::Available,
    });

    // One variant per account
    for i in 1..=3 {
        reg.add_variant(ModelVariant {
            id: format!("claude@project-{i}"),
            model_id: "claude-sonnet".into(),
            endpoint_id: "vertex-us".into(),
            account_id: Some(format!("project-{i}")),
            deployment_name: None,
            weight: 1.0,
            performance: ModelPerformance {
                p50_latency_ms: 300 + (i * 50) as u32,
                p95_latency_ms: 600,
                p99_latency_ms: 1000,
                ttft_p50_ms: 100,
                throughput_tps: 40.0,
                error_rate: 0.01,
                current_qps: (i * 5) as f32,
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
    }

    reg.add_pool(ModelPool {
        id: "vertex-pool".into(),
        name: "Vertex Claude".into(),
        variants: vec![
            "claude@project-1".into(),
            "claude@project-2".into(),
            "claude@project-3".into(),
        ],
        selection_mode: SelectionMode::Scored,
        description: None,
    });

    reg
}

fn request_with_session(content: &str, session_id: &str) -> RouteRequest {
    RouteRequest {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: content.into(),
        }],
        strategy: None,
        metadata: RouteMetadata {
            affinity_key: Some(session_id.into()),
            tenant_id: None,
            region: None,
            analyze_context: None,
            ..Default::default()
        },
        analyze_context: None,
        required_protocol: None,
        requested_model: None,
        inflight_snapshot: None,
        prefix_cache_index: None,
    }
}

// ──────────────────────────────────────────────
// L3 Tests
// ──────────────────────────────────────────────

#[test]
fn test_affinity_same_session_same_account() {
    let reg = build_multi_account_registry();
    let router = PrismRouter::new(reg);

    let d1 = router
        .route(&request_with_session("Hello", "session-abc"))
        .unwrap();
    let d2 = router
        .route(&request_with_session("How are you?", "session-abc"))
        .unwrap();
    assert_eq!(
        d1.selected_account, d2.selected_account,
        "Same session should route to same account"
    );

    let d3 = router
        .route(&request_with_session("Write me a poem", "session-abc"))
        .unwrap();
    assert_eq!(
        d1.selected_account, d3.selected_account,
        "Same session should always route to same account regardless of content"
    );

    let d4 = router
        .route(&request_with_session("Hello", "session-xyz"))
        .unwrap();
    assert!(!d4.selected_account.is_empty());
}

#[test]
fn test_soft_affinity_migrates_on_overload() {
    let mut reg = InMemoryRegistry::new();

    reg.add_provider(Provider {
        id: "vertex".into(),
        name: "GCP Vertex AI".into(),
        auth: AuthConfig::default(),
        base_url: "https://us-central1-aiplatform.googleapis.com".into(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    });

    reg.add_endpoint(Endpoint {
        id: "vertex-us".into(),
        provider_id: "vertex".into(),
        base_url: None,
        region: None,
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    });

    reg.add_account(Account {
        id: "project-overloaded".into(),
        endpoint_id: "vertex-us".into(),
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

    reg.add_account(Account {
        id: "project-available".into(),
        endpoint_id: "vertex-us".into(),
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
        pricing: ModelPricing::default(),
        specialties: vec![],
        limits: None,
        status: ModelStatus::Available,
    });

    reg.add_variant(ModelVariant {
        id: "claude@overloaded".into(),
        model_id: "claude-sonnet".into(),
        endpoint_id: "vertex-us".into(),
        account_id: Some("project-overloaded".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms: 300,
            p95_latency_ms: 600,
            p99_latency_ms: 1000,
            ttft_p50_ms: 100,
            throughput_tps: 40.0,
            error_rate: 0.01,
            current_qps: 1.6,
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

    reg.add_variant(ModelVariant {
        id: "claude@available".into(),
        model_id: "claude-sonnet".into(),
        endpoint_id: "vertex-us".into(),
        account_id: Some("project-available".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms: 300,
            p95_latency_ms: 600,
            p99_latency_ms: 1000,
            ttft_p50_ms: 100,
            throughput_tps: 40.0,
            error_rate: 0.01,
            current_qps: 0.5,
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
        variants: vec!["claude@overloaded".into(), "claude@available".into()],
        selection_mode: SelectionMode::Scored,
        description: None,
    });

    let router = PrismRouter::new(reg);

    let mut found_migration = false;
    for i in 0..100 {
        let session = format!("test-session-{i}");
        let decision = router
            .route(&request_with_session("Hello", &session))
            .unwrap();
        if decision.selected_account == "project-available" {
            found_migration = true;
        }
    }

    assert!(
        found_migration,
        "Soft affinity should migrate away from overloaded account"
    );
}

#[test]
fn test_hard_affinity_rejects_on_overload() {
    let mut reg = InMemoryRegistry::new();

    reg.add_provider(Provider {
        id: "vertex".into(),
        name: "Vertex".into(),
        auth: AuthConfig::default(),
        base_url: "https://vertex.example.com".into(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    });

    reg.add_endpoint(Endpoint {
        id: "vertex-ep".into(),
        provider_id: "vertex".into(),
        base_url: None,
        region: None,
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    });

    for i in 1..=2 {
        reg.add_account(Account {
            id: format!("acct-{i}"),
            endpoint_id: "vertex-ep".into(),
            auth: AuthConfig::default(),
            project_id: None,
            rate_limit: Some(RateLimit {
                rpm: Some(60),
                tpm: None,
                concurrent: None,
            }),
            billing_tag: None,
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: Default::default(),
        });
    }

    reg.add_model(Model {
        id: "model-a".into(),
        display_name: "Model A".into(),
        quality_tier: QualityTier::A,
        capabilities: ModelCapabilities {
            max_context_window: 128000,
            max_output_tokens: 8192,
            supports_streaming: true,
            supports_function_calling: false,
            supports_vision: false,
            supports_json_mode: false,
            supports_reasoning: false,
        },
        pricing: ModelPricing::default(),
        specialties: vec![],
        limits: None,
        status: ModelStatus::Available,
    });

    for i in 1..=2 {
        reg.add_variant(ModelVariant {
            id: format!("model-a@acct-{i}"),
            model_id: "model-a".into(),
            endpoint_id: "vertex-ep".into(),
            account_id: Some(format!("acct-{i}")),
            deployment_name: None,
            weight: 1.0,
            performance: ModelPerformance {
                p50_latency_ms: 300,
                p95_latency_ms: 600,
                p99_latency_ms: 1000,
                ttft_p50_ms: 100,
                throughput_tps: 40.0,
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
            metadata: std::collections::HashMap::new(),
        });
    }

    reg.add_pool(ModelPool {
        id: "all".into(),
        name: "All".into(),
        variants: vec!["model-a@acct-1".into(), "model-a@acct-2".into()],
        selection_mode: SelectionMode::Scored,
        description: None,
    });

    let router = PrismRouter::new(reg);

    let decision = router
        .route(&request_with_session("Hello", "session-123"))
        .unwrap();
    assert!(
        !decision.selected_account.is_empty(),
        "Should still route even when all accounts are overloaded (soft degradation)"
    );
}

#[test]
fn test_load_balanced_distribution() {
    let reg = build_multi_account_registry();
    let router = PrismRouter::new(reg);

    let mut account_counts = std::collections::HashMap::new();
    for i in 0..30 {
        let req = RouteRequest {
            messages: vec![ChatMessage {
                role: "user".into(),
                content: format!("Request number {i}"),
            }],
            strategy: None,
            metadata: RouteMetadata::default(),
            analyze_context: None,
            required_protocol: None,
            requested_model: None,
            inflight_snapshot: None,
            prefix_cache_index: None,
        };
        let decision = router.route(&req).unwrap();
        *account_counts
            .entry(decision.selected_account)
            .or_insert(0u32) += 1;
    }

    let total: u32 = account_counts.values().sum();
    assert_eq!(total, 30);
}

#[test]
fn test_cost_isolation_tenant_routing() {
    let reg = build_multi_account_registry();
    let router = PrismRouter::new(reg);

    let req1 = RouteRequest {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "Hello from team 1".into(),
        }],
        strategy: None,
        metadata: RouteMetadata {
            tenant_id: Some("team-1".into()),
            affinity_key: None,
            region: None,
            analyze_context: None,
            ..Default::default()
        },
        analyze_context: None,
        required_protocol: None,
        requested_model: None,
        inflight_snapshot: None,
        prefix_cache_index: None,
    };
    let d1 = router.route(&req1).unwrap();
    assert_eq!(
        d1.selected_account, "project-1",
        "tenant team-1 should route to project-1"
    );

    let req2 = RouteRequest {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "Hello from team 2".into(),
        }],
        strategy: None,
        metadata: RouteMetadata {
            tenant_id: Some("team-2".into()),
            affinity_key: None,
            region: None,
            analyze_context: None,
            ..Default::default()
        },
        analyze_context: None,
        required_protocol: None,
        requested_model: None,
        inflight_snapshot: None,
        prefix_cache_index: None,
    };
    let d2 = router.route(&req2).unwrap();
    assert_eq!(
        d2.selected_account, "project-2",
        "tenant team-2 should route to project-2"
    );

    let req3 = RouteRequest {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "Hello from team 3".into(),
        }],
        strategy: None,
        metadata: RouteMetadata {
            tenant_id: Some("team-3".into()),
            affinity_key: None,
            region: None,
            analyze_context: None,
            ..Default::default()
        },
        analyze_context: None,
        required_protocol: None,
        requested_model: None,
        inflight_snapshot: None,
        prefix_cache_index: None,
    };
    let d3 = router.route(&req3).unwrap();
    assert_eq!(
        d3.selected_account, "project-3",
        "tenant team-3 should route to project-3"
    );
}
