use prism_core::feedback::{FeedbackCollector, OutcomeReport};

#[test]
fn test_record_outcome() {
    let collector = FeedbackCollector::new(100);

    let report = OutcomeReport {
        variant_id: "gpt4@openai".to_string(),
        success: true,
        latency_ms: 200,
        prompt_tokens: Some(100),
        completion_tokens: Some(50),
        cost: Some(0.01),
        user_rating: Some(4),
    };

    collector.record(&report);

    let stats = collector.get_stats("gpt4@openai").unwrap();
    assert_eq!(stats.total_requests, 1);
    assert_eq!(stats.successful_requests, 1);
    assert_eq!(stats.total_latency_ms, 200);
    assert_eq!(stats.total_tokens, 150);
    assert_eq!(stats.user_ratings.len(), 1);
    assert_eq!(stats.user_ratings[0], 4);
}

#[test]
fn test_success_rate() {
    let collector = FeedbackCollector::new(100);

    // Record 8 successes and 2 failures
    for i in 0..10 {
        let report = OutcomeReport {
            variant_id: "v1".to_string(),
            success: i < 8,
            latency_ms: 100,
            prompt_tokens: Some(50),
            completion_tokens: Some(25),
            cost: None,
            user_rating: None,
        };
        collector.record(&report);
    }

    let stats = collector.get_stats("v1").unwrap();
    assert_eq!(stats.total_requests, 10);
    assert_eq!(stats.successful_requests, 8);
    let sr = stats.success_rate();
    assert!((sr - 0.8).abs() < 0.001, "expected 0.8, got {}", sr);
    assert_eq!(stats.error_count, 2);
}

#[test]
fn test_quality_adjustment_boost() {
    let collector = FeedbackCollector::new(100);

    // Record 20 perfect outcomes: 100% success, rating 5
    for _ in 0..20 {
        collector.record(&OutcomeReport {
            variant_id: "good-variant".to_string(),
            success: true,
            latency_ms: 100,
            prompt_tokens: Some(50),
            completion_tokens: Some(25),
            cost: None,
            user_rating: Some(5),
        });
    }

    let adj = collector.quality_adjustment("good-variant");
    assert!(adj > 1.0, "expected boost > 1.0, got {}", adj);
    assert!(adj <= 1.5, "expected boost <= 1.5, got {}", adj);
}

#[test]
fn test_quality_adjustment_penalize() {
    let collector = FeedbackCollector::new(100);

    // Record 20 bad outcomes: 30% success, rating 1
    for i in 0..20 {
        collector.record(&OutcomeReport {
            variant_id: "bad-variant".to_string(),
            success: i < 6, // 30% success
            latency_ms: 5000,
            prompt_tokens: Some(50),
            completion_tokens: Some(25),
            cost: None,
            user_rating: Some(1),
        });
    }

    let adj = collector.quality_adjustment("bad-variant");
    assert!(adj < 1.0, "expected penalty < 1.0, got {}", adj);
    assert!(adj >= 0.5, "expected penalty >= 0.5, got {}", adj);
}

#[test]
fn test_quality_adjustment_neutral_no_data() {
    let collector = FeedbackCollector::new(100);
    let adj = collector.quality_adjustment("unknown-variant");
    assert!(
        (adj - 1.0).abs() < 0.001,
        "expected neutral 1.0, got {}",
        adj
    );
}

#[test]
fn test_quality_adjustment_neutral_few_samples() {
    let collector = FeedbackCollector::new(100);

    // Only 3 samples — too few for adjustment
    for _ in 0..3 {
        collector.record(&OutcomeReport {
            variant_id: "few-samples".to_string(),
            success: false,
            latency_ms: 5000,
            prompt_tokens: None,
            completion_tokens: None,
            cost: None,
            user_rating: Some(1),
        });
    }

    let adj = collector.quality_adjustment("few-samples");
    assert!(
        (adj - 1.0).abs() < 0.001,
        "expected neutral 1.0 with few samples, got {}",
        adj
    );
}

#[test]
fn test_get_all_stats() {
    let collector = FeedbackCollector::new(100);

    collector.record(&OutcomeReport {
        variant_id: "v1".to_string(),
        success: true,
        latency_ms: 100,
        prompt_tokens: None,
        completion_tokens: None,
        cost: None,
        user_rating: None,
    });
    collector.record(&OutcomeReport {
        variant_id: "v2".to_string(),
        success: false,
        latency_ms: 200,
        prompt_tokens: None,
        completion_tokens: None,
        cost: None,
        user_rating: None,
    });

    let all = collector.get_all_stats();
    assert_eq!(all.len(), 2);
    assert!(all.contains_key("v1"));
    assert!(all.contains_key("v2"));
}

#[test]
fn test_windowed_stats() {
    // Window of 5 — old records should be overwritten
    let collector = FeedbackCollector::new(5);

    // Record 5 failures first
    for _ in 0..5 {
        collector.record(&OutcomeReport {
            variant_id: "windowed".to_string(),
            success: false,
            latency_ms: 1000,
            prompt_tokens: None,
            completion_tokens: None,
            cost: None,
            user_rating: None,
        });
    }

    let stats = collector.get_stats("windowed").unwrap();
    assert_eq!(stats.success_rate(), 0.0);

    // Now record 5 successes — should replace the failures
    for _ in 0..5 {
        collector.record(&OutcomeReport {
            variant_id: "windowed".to_string(),
            success: true,
            latency_ms: 100,
            prompt_tokens: None,
            completion_tokens: None,
            cost: None,
            user_rating: None,
        });
    }

    let stats = collector.get_stats("windowed").unwrap();
    assert_eq!(stats.total_requests, 5); // window size
    assert_eq!(stats.success_rate(), 1.0); // all successes now
}

#[test]
fn test_feedback_influences_routing() {
    use prism_core::registry::InMemoryRegistry;
    use prism_core::router::{PrismRouter, RouteMetadata, RouteRequest, Router};
    use prism_core::strategy::*;
    use prism_core::types::*;

    // Set up a registry with two variants of comparable quality
    let mut registry = InMemoryRegistry::new();

    let provider = Provider {
        id: "openai".to_string(),
        name: "OpenAI".to_string(),
        auth: AuthConfig::default(),
        base_url: "https://api.openai.com".to_string(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    };
    registry.add_provider(provider);

    let provider2 = Provider {
        id: "anthropic".to_string(),
        name: "Anthropic".to_string(),
        auth: AuthConfig::default(),
        base_url: "https://api.anthropic.com".to_string(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    };
    registry.add_provider(provider2);

    let ep1 = Endpoint {
        id: "openai-ep".to_string(),
        provider_id: "openai".to_string(),
        base_url: None,
        region: None,
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    };
    registry.add_endpoint(ep1);

    let ep2 = Endpoint {
        id: "anthropic-ep".to_string(),
        provider_id: "anthropic".to_string(),
        base_url: None,
        region: None,
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    };
    registry.add_endpoint(ep2);

    let acct1 = Account {
        id: "openai-acct".to_string(),
        endpoint_id: "openai-ep".to_string(),
        auth: AuthConfig::default(),
        project_id: None,
        rate_limit: None,
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
    };
    registry.add_account(acct1);

    let acct2 = Account {
        id: "anthropic-acct".to_string(),
        endpoint_id: "anthropic-ep".to_string(),
        auth: AuthConfig::default(),
        project_id: None,
        rate_limit: None,
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
    };
    registry.add_account(acct2);

    // Two models with same quality tier but different pricing
    // claude-sonnet is cheaper, so without feedback it would win on cost
    // We'll use the same pricing to make it a fair comparison
    let model1 = Model {
        id: "gpt-4o".to_string(),
        display_name: "GPT-4o".to_string(),
        quality_tier: QualityTier::S,
        capabilities: ModelCapabilities {
            max_context_window: 128000,
            max_output_tokens: 4096,
            supports_streaming: true,
            supports_function_calling: true,
            supports_vision: false,
            supports_json_mode: true,
            supports_reasoning: false,
        },
        pricing: ModelPricing {
            input_per_1m_tokens: 5.0,
            output_per_1m_tokens: 15.0,
            currency: "USD".to_string(),
        },
        specialties: vec![],
        limits: None,
        status: ModelStatus::Available,
    };
    registry.add_model(model1);

    let model2 = Model {
        id: "claude-sonnet".to_string(),
        display_name: "Claude Sonnet".to_string(),
        quality_tier: QualityTier::S,
        capabilities: ModelCapabilities {
            max_context_window: 200000,
            max_output_tokens: 4096,
            supports_streaming: true,
            supports_function_calling: true,
            supports_vision: false,
            supports_json_mode: true,
            supports_reasoning: false,
        },
        pricing: ModelPricing {
            input_per_1m_tokens: 5.0,
            output_per_1m_tokens: 15.0,
            currency: "USD".to_string(),
        },
        specialties: vec![],
        limits: None,
        status: ModelStatus::Available,
    };
    registry.add_model(model2);

    let v1 = ModelVariant {
        id: "gpt4@openai".to_string(),
        model_id: "gpt-4o".to_string(),
        endpoint_id: "openai-ep".to_string(),
        account_id: Some("openai-acct".to_string()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance::default(),
        pricing_override: None,
        status: VariantStatus::Active,
        base_url: None,
        upstream_model_name: None,
    metadata: std::collections::HashMap::new(),
    };
    registry.add_variant(v1);

    let v2 = ModelVariant {
        id: "claude@anthropic".to_string(),
        model_id: "claude-sonnet".to_string(),
        endpoint_id: "anthropic-ep".to_string(),
        account_id: Some("anthropic-acct".to_string()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance::default(),
        pricing_override: None,
        status: VariantStatus::Active,
        base_url: None,
        upstream_model_name: None,
    metadata: std::collections::HashMap::new(),
    };
    registry.add_variant(v2);

    let pool = ModelPool {
        id: "default".to_string(),
        name: "Default".to_string(),
        variants: vec!["gpt4@openai".to_string(), "claude@anthropic".to_string()],
        selection_mode: SelectionMode::Scored,
        description: None,
    };
    registry.add_pool(pool);

    let router = PrismRouter::new(registry);

    // Record many bad outcomes for gpt4@openai
    for _ in 0..20 {
        router.record_outcome(&prism_core::feedback::OutcomeReport {
            variant_id: "gpt4@openai".to_string(),
            success: false,
            latency_ms: 5000,
            prompt_tokens: Some(100),
            completion_tokens: Some(50),
            cost: None,
            user_rating: Some(1),
        });
    }

    // Record many good outcomes for claude@anthropic
    for _ in 0..20 {
        router.record_outcome(&prism_core::feedback::OutcomeReport {
            variant_id: "claude@anthropic".to_string(),
            success: true,
            latency_ms: 100,
            prompt_tokens: Some(100),
            completion_tokens: Some(50),
            cost: None,
            user_rating: Some(5),
        });
    }

    // Now route — claude should be preferred due to feedback
    let req = RouteRequest {
        messages: vec![prism_core::analyzer::ChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        }],
        strategy: Some(ModelSelectionStrategy::Auto {
            objective: RoutingObjective::default(),
            pool_ids: Some(vec!["default".to_string()]),
            required_capabilities: None,
        }),
        metadata: RouteMetadata::default(),
        analyze_context: None,
        required_protocol: None,
        requested_model: None,
    };

    let decision = router.route(&req).expect("routing should succeed");
    assert_eq!(
        decision.selected_variant, "claude@anthropic",
        "Expected claude to win over penalized gpt4; got {}",
        decision.selected_variant
    );
}
