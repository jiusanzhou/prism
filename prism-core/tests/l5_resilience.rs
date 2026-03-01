use prism_core::analyzer::ChatMessage;
use prism_core::registry::InMemoryRegistry;
use prism_core::router::{PrismRouter, RouteMetadata, RouteRequest, Router};
use prism_core::strategy::*;
use prism_core::types::*;

fn build_resilience_registry() -> InMemoryRegistry {
    let mut reg = InMemoryRegistry::new();

    // Primary provider
    reg.add_provider(Provider {
        id: "primary".into(),
        name: "Primary".into(),
        auth: AuthConfig::default(),
        base_url: "https://primary.example.com".into(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    });

    // Backup provider
    reg.add_provider(Provider {
        id: "backup".into(),
        name: "Backup".into(),
        auth: AuthConfig::default(),
        base_url: "https://backup.example.com".into(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
        protocols: vec![],
    });

    reg.add_endpoint(Endpoint {
        id: "primary-ep".into(),
        provider_id: "primary".into(),
        base_url: None,
        region: None,
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    });
    reg.add_endpoint(Endpoint {
        id: "backup-ep".into(),
        provider_id: "backup".into(),
        base_url: None,
        region: None,
        auth_override: None,
        rate_limit: None,
        priority: 1,
        status: HealthStatus::Healthy,
    });

    reg.add_account(Account {
        id: "primary-acct".into(),
        endpoint_id: "primary-ep".into(),
        auth: AuthConfig::default(),
        project_id: None,
        rate_limit: None,
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
    });
    reg.add_account(Account {
        id: "backup-acct".into(),
        endpoint_id: "backup-ep".into(),
        auth: AuthConfig::default(),
        project_id: None,
        rate_limit: None,
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
    });

    // Primary model (S-tier)
    reg.add_model(Model {
        id: "model-primary".into(),
        display_name: "Primary Model".into(),
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
        specialties: vec![],
        limits: None,
        status: ModelStatus::Available,
    });

    // Backup model (A-tier, cheaper)
    reg.add_model(Model {
        id: "model-backup".into(),
        display_name: "Backup Model".into(),
        quality_tier: QualityTier::A,
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
            input_per_1m_tokens: 0.5,
            output_per_1m_tokens: 2.0,
            currency: "USD".into(),
        },
        specialties: vec![],
        limits: None,
        status: ModelStatus::Available,
    });

    reg.add_variant(ModelVariant {
        id: "primary-variant".into(),
        model_id: "model-primary".into(),
        endpoint_id: "primary-ep".into(),
        account_id: Some("primary-acct".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms: 300,
            p95_latency_ms: 600,
            p99_latency_ms: 1000,
            ttft_p50_ms: 120,
            throughput_tps: 50.0,
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
        id: "backup-variant".into(),
        model_id: "model-backup".into(),
        endpoint_id: "backup-ep".into(),
        account_id: Some("backup-acct".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms: 200,
            p95_latency_ms: 400,
            p99_latency_ms: 600,
            ttft_p50_ms: 80,
            throughput_tps: 60.0,
            error_rate: 0.005,
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

    reg.add_pool(ModelPool {
        id: "all".into(),
        name: "All".into(),
        variants: vec!["primary-variant".into(), "backup-variant".into()],
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
// L5 Tests
// ──────────────────────────────────────────────

#[test]
fn test_fallback_chain_populated() {
    let reg = build_resilience_registry();
    let router = PrismRouter::new(reg);

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // Should have a fallback chain with the other variant
    assert!(
        !decision.fallback_chain.is_empty(),
        "Fallback chain should be populated"
    );
}

#[test]
fn test_retry_on_failure() {
    // When the primary variant returns 500, should retry according to RetryPolicy
    let reg = build_resilience_registry();
    let router = PrismRouter::new(reg).with_resilience_policy(ResiliencePolicy {
        retry: RetryPolicy {
            max_retries: 2,
            retry_on: vec![RetryCondition::HttpStatus(vec![500, 502, 503])],
            backoff: BackoffStrategy::Fixed { delay_ms: 100 },
            retry_scope: RetryScope::SameVariant,
        },
        fallback: FallbackPolicy::default(),
        circuit_breaker: CircuitBreakerPolicy {
            error_threshold_pct: 50.0,
            window_seconds: 60,
            min_requests: 100, // high threshold so CB doesn't interfere
            open_duration_seconds: 30,
            half_open_requests: 3,
        },
        timeout: TimeoutPolicy::default(),
        hedging: None,
    });

    // Attempt 1: should retry the same variant
    let action = router
        .on_failure("primary-variant", FailureType::HttpError(500), 1)
        .unwrap();
    match &action {
        RecoveryAction::Retry { variant_id, delay_ms } => {
            assert_eq!(variant_id, "primary-variant");
            assert_eq!(*delay_ms, 100);
        }
        other => panic!("Expected Retry, got {:?}", other),
    }

    // Attempt 2: still within max_retries, should retry
    let action = router
        .on_failure("primary-variant", FailureType::HttpError(500), 2)
        .unwrap();
    match &action {
        RecoveryAction::Retry { variant_id, .. } => {
            assert_eq!(variant_id, "primary-variant");
        }
        other => panic!("Expected Retry on attempt 2, got {:?}", other),
    }

    // Attempt 3: exceeds max_retries (2), should fallback
    let action = router
        .on_failure("primary-variant", FailureType::HttpError(500), 3)
        .unwrap();
    match &action {
        RecoveryAction::Fallback { variant_id } => {
            assert_eq!(variant_id, "backup-variant");
        }
        other => panic!("Expected Fallback after max retries, got {:?}", other),
    }
}

#[test]
fn test_retry_cross_provider() {
    // When retry_scope is CrossProvider and primary fails,
    // should retry on backup provider
    let reg = build_resilience_registry();
    let router = PrismRouter::new(reg).with_resilience_policy(ResiliencePolicy {
        retry: RetryPolicy {
            max_retries: 2,
            retry_on: vec![RetryCondition::HttpStatus(vec![500, 502, 503])],
            backoff: BackoffStrategy::Fixed { delay_ms: 50 },
            retry_scope: RetryScope::CrossProvider,
        },
        fallback: FallbackPolicy::default(),
        circuit_breaker: CircuitBreakerPolicy {
            error_threshold_pct: 50.0,
            window_seconds: 60,
            min_requests: 100,
            open_duration_seconds: 30,
            half_open_requests: 3,
        },
        timeout: TimeoutPolicy::default(),
        hedging: None,
    });

    // CrossProvider retry: should get a variant from a different provider
    let action = router
        .on_failure("primary-variant", FailureType::HttpError(500), 1)
        .unwrap();
    match &action {
        RecoveryAction::Retry { variant_id, .. } => {
            // Should be backup-variant (different provider)
            assert_eq!(
                variant_id, "backup-variant",
                "CrossProvider retry should select variant from different provider"
            );
        }
        other => panic!("Expected Retry with cross-provider variant, got {:?}", other),
    }
}

#[test]
fn test_fallback_to_backup_model() {
    // When primary model fails all retries, should fallback to model-backup
    let reg = build_resilience_registry();
    let router = PrismRouter::new(reg).with_resilience_policy(ResiliencePolicy {
        retry: RetryPolicy {
            max_retries: 1,
            retry_on: vec![RetryCondition::HttpStatus(vec![500])],
            backoff: BackoffStrategy::Fixed { delay_ms: 100 },
            retry_scope: RetryScope::SameVariant,
        },
        fallback: FallbackPolicy {
            chain: vec![
                FallbackTarget::Model("model-backup".into()),
            ],
            allow_model_downgrade: true,
            notify_client: true,
        },
        circuit_breaker: CircuitBreakerPolicy {
            error_threshold_pct: 50.0,
            window_seconds: 60,
            min_requests: 100,
            open_duration_seconds: 30,
            half_open_requests: 3,
        },
        timeout: TimeoutPolicy::default(),
        hedging: None,
    });

    // Attempt 1: retry same variant
    let action = router
        .on_failure("primary-variant", FailureType::HttpError(500), 1)
        .unwrap();
    assert!(
        matches!(&action, RecoveryAction::Retry { variant_id, .. } if variant_id == "primary-variant"),
        "Attempt 1 should retry, got {:?}",
        action
    );

    // Attempt 2: exceeds max_retries, should fallback to backup model
    let action = router
        .on_failure("primary-variant", FailureType::HttpError(500), 2)
        .unwrap();
    match &action {
        RecoveryAction::Fallback { variant_id } => {
            assert_eq!(
                variant_id, "backup-variant",
                "Should fallback to the variant of model-backup"
            );
        }
        other => panic!("Expected Fallback to backup model, got {:?}", other),
    }
}

#[test]
fn test_circuit_breaker_opens() {
    // After error_threshold_pct exceeded within window_seconds,
    // circuit should open and requests should go to fallback
    let reg = build_resilience_registry();
    let router = PrismRouter::new(reg).with_resilience_policy(ResiliencePolicy {
        retry: RetryPolicy {
            max_retries: 3,
            retry_on: vec![RetryCondition::HttpStatus(vec![500])],
            backoff: BackoffStrategy::Fixed { delay_ms: 100 },
            retry_scope: RetryScope::SameVariant,
        },
        fallback: FallbackPolicy::default(),
        circuit_breaker: CircuitBreakerPolicy {
            error_threshold_pct: 50.0,
            window_seconds: 60,
            min_requests: 5, // Circuit opens after 5 failures
            open_duration_seconds: 30,
            half_open_requests: 3,
        },
        timeout: TimeoutPolicy::default(),
        hedging: None,
    });

    // Simulate 4 failures (below threshold) — should still retry
    for attempt in 1..=4 {
        let action = router
            .on_failure("primary-variant", FailureType::HttpError(500), attempt)
            .unwrap();
        // Each call records a failure. By attempt 4 we have 4 failures recorded.
        // Since 4 < min_requests(5) and attempt <= max_retries(3) only holds for 1-3:
        if attempt <= 3 {
            assert!(
                matches!(&action, RecoveryAction::Retry { .. }),
                "Attempt {} should retry (CB not open yet), got {:?}",
                attempt,
                action
            );
        }
    }

    // Now we have 4 recorded failures. Record one more to hit min_requests=5.
    // The 5th on_failure call should trigger circuit breaker open → Fallback
    let action = router
        .on_failure("primary-variant", FailureType::HttpError(500), 1)
        .unwrap();
    match &action {
        RecoveryAction::Fallback { variant_id } => {
            assert_eq!(
                variant_id, "backup-variant",
                "Circuit breaker should fallback to backup"
            );
        }
        other => panic!(
            "Expected Fallback when circuit breaker is open, got {:?}",
            other
        ),
    }
}

#[test]
fn test_hedging_takes_fastest() {
    // With HedgingPolicy { max_parallel: 2, delay_ms: 200 },
    // plan_hedging should return 2 variant ids to send requests to in parallel
    let reg = build_resilience_registry();
    let router = PrismRouter::new(reg).with_resilience_policy(ResiliencePolicy {
        retry: RetryPolicy::default(),
        fallback: FallbackPolicy::default(),
        circuit_breaker: CircuitBreakerPolicy::default(),
        timeout: TimeoutPolicy::default(),
        hedging: Some(HedgingPolicy {
            max_parallel: 2,
            delay_ms: 200,
            cancel_remaining: true,
        }),
    });

    let req = simple_request("Hello");
    let hedging_variants = router.plan_hedging(&req).unwrap();

    // Should return 2 variants (both available)
    assert_eq!(
        hedging_variants.len(),
        2,
        "Hedging should return 2 variants, got {:?}",
        hedging_variants
    );

    // Variants should be distinct
    assert_ne!(
        hedging_variants[0], hedging_variants[1],
        "Hedging variants should be different"
    );

    // Both variants should be from our registry
    let known = ["primary-variant", "backup-variant"];
    for v in &hedging_variants {
        assert!(
            known.contains(&v.as_str()),
            "Unknown hedging variant: {}",
            v
        );
    }
}

#[test]
fn test_ttft_timeout_triggers_retry() {
    // When first token doesn't arrive within ttft_ms, should retry with another variant
    let reg = build_resilience_registry();
    let router = PrismRouter::new(reg).with_resilience_policy(ResiliencePolicy {
        retry: RetryPolicy {
            max_retries: 2,
            retry_on: vec![RetryCondition::Timeout],
            backoff: BackoffStrategy::Fixed { delay_ms: 50 },
            retry_scope: RetryScope::CrossProvider,
        },
        fallback: FallbackPolicy::default(),
        circuit_breaker: CircuitBreakerPolicy {
            error_threshold_pct: 50.0,
            window_seconds: 60,
            min_requests: 100,
            open_duration_seconds: 30,
            half_open_requests: 3,
        },
        timeout: TimeoutPolicy {
            connect_ms: 3000,
            request_ms: 30000,
            ttft_ms: Some(5000),
        },
        hedging: None,
    });

    // TTFTTimeout should trigger a retry with a different variant (cross-provider)
    let action = router
        .on_failure("primary-variant", FailureType::TTFTTimeout, 1)
        .unwrap();
    match &action {
        RecoveryAction::Retry { variant_id, delay_ms } => {
            assert_eq!(
                variant_id, "backup-variant",
                "TTFT timeout should retry with cross-provider variant"
            );
            assert_eq!(*delay_ms, 50);
        }
        other => panic!("Expected Retry on TTFT timeout, got {:?}", other),
    }

    // Second TTFT timeout: still within max_retries
    let action = router
        .on_failure("primary-variant", FailureType::TTFTTimeout, 2)
        .unwrap();
    assert!(
        matches!(&action, RecoveryAction::Retry { .. }),
        "Second TTFT timeout should still retry, got {:?}",
        action
    );

    // Third attempt: exceeds max_retries → fallback
    let action = router
        .on_failure("primary-variant", FailureType::TTFTTimeout, 3)
        .unwrap();
    assert!(
        matches!(&action, RecoveryAction::Fallback { .. }),
        "Third TTFT timeout should fallback, got {:?}",
        action
    );
}
