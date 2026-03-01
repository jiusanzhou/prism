use prism_core::analyzer::{ChatMessage, RuleBasedAnalyzer, RequestAnalyzer};
use prism_core::registry::{InMemoryRegistry, ModelRegistryTrait};
use prism_core::router::{PrismRouter, RouteMetadata, RouteRequest, Router};
use prism_core::strategy::*;
use prism_core::types::*;

// ──────────────────────────────────────────────
// Test helpers
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

fn make_model(
    id: &str,
    tier: QualityTier,
    specialties: Vec<TaskType>,
    pricing_input: f64,
    pricing_output: f64,
) -> Model {
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
            input_per_1m_tokens: pricing_input,
            output_per_1m_tokens: pricing_output,
            currency: "USD".to_string(),
        },
        specialties,
        limits: None,
        status: ModelStatus::Available,
    }
}

fn make_variant(
    id: &str,
    model_id: &str,
    endpoint_id: &str,
    account_id: &str,
    latency_p50: u32,
    latency_p99: u32,
) -> ModelVariant {
    ModelVariant {
        id: id.to_string(),
        model_id: model_id.to_string(),
        endpoint_id: endpoint_id.to_string(),
        account_id: Some(account_id.to_string()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms: latency_p50,
            p95_latency_ms: (latency_p50 as f32 * 1.5) as u32,
            p99_latency_ms: latency_p99,
            ttft_p50_ms: latency_p50 / 3,
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
    }
}

/// Build a registry with tier-1 (S), tier-2 (A), tier-3 (B) models.
fn build_test_registry() -> InMemoryRegistry {
    let mut reg = InMemoryRegistry::new();

    // Providers
    reg.add_provider(make_provider("openai"));
    reg.add_provider(make_provider("anthropic"));
    reg.add_provider(make_provider("deepseek"));

    // Endpoints
    reg.add_endpoint(make_endpoint("openai-ep", "openai"));
    reg.add_endpoint(make_endpoint("anthropic-ep", "anthropic"));
    reg.add_endpoint(make_endpoint("deepseek-ep", "deepseek"));

    // Accounts
    reg.add_account(make_account("openai-acct", "openai-ep"));
    reg.add_account(make_account("anthropic-acct", "anthropic-ep"));
    reg.add_account(make_account("deepseek-acct", "deepseek-ep"));

    // Models
    let mut gpt4o = make_model(
        "gpt-4o",
        QualityTier::S,
        vec![TaskType::Reasoning, TaskType::CodeGeneration, TaskType::Creative],
        2.5,
        10.0,
    );
    gpt4o.capabilities.supports_vision = true;

    let mut claude = make_model(
        "claude-sonnet",
        QualityTier::S,
        vec![TaskType::Reasoning, TaskType::CodeReview, TaskType::Creative],
        3.0,
        15.0,
    );
    claude.capabilities.supports_vision = true;
    claude.capabilities.supports_reasoning = true;

    let gpt4o_mini = make_model(
        "gpt-4o-mini",
        QualityTier::A,
        vec![TaskType::SimpleQA, TaskType::Translation, TaskType::Summarization],
        0.15,
        0.6,
    );

    let mut ds_v3 = make_model(
        "deepseek-v3",
        QualityTier::A,
        vec![TaskType::CodeGeneration, TaskType::Reasoning, TaskType::Math],
        0.27,
        1.1,
    );
    ds_v3.capabilities.max_context_window = 64000;

    let ds_lite = make_model(
        "deepseek-lite",
        QualityTier::B,
        vec![TaskType::SimpleQA, TaskType::Conversation],
        0.07,
        0.3,
    );

    reg.add_model(gpt4o);
    reg.add_model(claude);
    reg.add_model(gpt4o_mini);
    reg.add_model(ds_v3);
    reg.add_model(ds_lite);

    // Variants
    reg.add_variant(make_variant("gpt-4o@openai", "gpt-4o", "openai-ep", "openai-acct", 320, 1200));
    reg.add_variant(make_variant("claude@anthropic", "claude-sonnet", "anthropic-ep", "anthropic-acct", 400, 1500));
    reg.add_variant(make_variant("gpt-4o-mini@openai", "gpt-4o-mini", "openai-ep", "openai-acct", 150, 500));
    reg.add_variant(make_variant("ds-v3@deepseek", "deepseek-v3", "deepseek-ep", "deepseek-acct", 280, 900));
    reg.add_variant(make_variant("ds-lite@deepseek", "deepseek-lite", "deepseek-ep", "deepseek-acct", 80, 200));

    // Pools
    reg.add_pool(ModelPool {
        id: "tier-1".into(),
        name: "High quality".into(),
        variants: vec!["gpt-4o@openai".into(), "claude@anthropic".into()],
        selection_mode: SelectionMode::Scored,
        description: None,
    });
    reg.add_pool(ModelPool {
        id: "tier-2".into(),
        name: "Cost effective".into(),
        variants: vec!["gpt-4o-mini@openai".into(), "ds-v3@deepseek".into()],
        selection_mode: SelectionMode::Scored,
        description: None,
    });
    reg.add_pool(ModelPool {
        id: "tier-3".into(),
        name: "Budget".into(),
        variants: vec!["ds-lite@deepseek".into()],
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
// L1 Tests
// ──────────────────────────────────────────────

#[test]
fn test_simple_qa_routes_to_cheap_model() {
    let reg = build_test_registry();
    let router = PrismRouter::new(reg).with_strategy(ModelSelectionStrategy::Auto {
        objective: RoutingObjective::CostFirst {
            min_quality_score: 0.3,
            budget_per_request: None,
        },
        pool_ids: None,
        required_capabilities: None,
    });

    let req = simple_request("What is the capital of France?");
    let decision = router.route(&req).unwrap();

    // CostFirst with low quality floor → should pick cheapest (deepseek-lite)
    assert_eq!(decision.selected_model, "deepseek-lite");
}

#[test]
fn test_code_generation_routes_to_code_model() {
    let reg = build_test_registry();
    let router = PrismRouter::new(reg).with_strategy(ModelSelectionStrategy::Auto {
        objective: RoutingObjective::Balanced {
            cost_weight: 0.2,
            quality_weight: 0.6,
            latency_weight: 0.2,
        },
        pool_ids: None,
        required_capabilities: None,
    });

    let req = simple_request("Write a function to implement quicksort in Rust");
    let decision = router.route(&req).unwrap();

    // Code generation → should prefer models with CodeGeneration specialty
    // claude-sonnet (CodeReview, Reasoning) may also score well on code tasks
    let code_capable = ["gpt-4o", "deepseek-v3", "claude-sonnet"];
    assert!(
        code_capable.contains(&decision.selected_model.as_str()),
        "Expected code-capable model, got {}",
        decision.selected_model
    );
}

#[test]
fn test_reasoning_routes_to_tier1() {
    let reg = build_test_registry();
    let router = PrismRouter::new(reg).with_strategy(ModelSelectionStrategy::Auto {
        objective: RoutingObjective::QualityFirst { max_latency_ms: None },
        pool_ids: None,
        required_capabilities: None,
    });

    let req = simple_request("推理分析：为什么量子计算对密码学有重大影响？请逐步推理。");
    let decision = router.route(&req).unwrap();

    // Reasoning task + quality first → should pick claude (supports_reasoning + S tier)
    assert_eq!(decision.selected_model, "claude-sonnet");
}

#[test]
fn test_vision_filters_unsupported() {
    let reg = build_test_registry();
    let router = PrismRouter::new(reg);

    // "看这张图" triggers MultiModal → requires_vision
    let req = simple_request("看这张图片里有什么？");
    let decision = router.route(&req).unwrap();

    // Only gpt-4o and claude support vision
    let vision_models = ["gpt-4o", "claude-sonnet"];
    assert!(
        vision_models.contains(&decision.selected_model.as_str()),
        "Expected vision model, got {}",
        decision.selected_model
    );
}

#[test]
fn test_long_context_filters_small_window() {
    let mut reg = build_test_registry();

    // Add a model with very small context window
    let mut small_model = make_model("tiny-model", QualityTier::S, vec![], 0.01, 0.01);
    small_model.capabilities.max_context_window = 2000;
    reg.add_model(small_model);
    reg.add_variant(make_variant("tiny@openai", "tiny-model", "openai-ep", "openai-acct", 50, 100));

    let router = PrismRouter::new(reg);

    // Create a very long message (triggers Large context need)
    let long_content = "word ".repeat(40000);
    let req = simple_request(&long_content);
    let decision = router.route(&req).unwrap();

    // tiny-model (2K context) should be filtered out
    assert_ne!(decision.selected_model, "tiny-model");
}

#[test]
fn test_pinned_uses_specified_model() {
    let reg = build_test_registry();
    let router = PrismRouter::new(reg);

    let mut req = simple_request("Hello");
    req.strategy = Some(ModelSelectionStrategy::Pinned {
        model_id: "deepseek-v3".into(),
    });

    let decision = router.route(&req).unwrap();
    assert_eq!(decision.selected_model, "deepseek-v3");
}

#[test]
fn test_pinned_nonexistent_model_errors() {
    let reg = build_test_registry();
    let router = PrismRouter::new(reg);

    let mut req = simple_request("Hello");
    req.strategy = Some(ModelSelectionStrategy::Pinned {
        model_id: "nonexistent-model".into(),
    });

    let result = router.route(&req);
    assert!(result.is_err());
}

#[test]
fn test_balanced_weights_affect_selection() {
    let reg1 = build_test_registry();
    let reg2 = build_test_registry();

    // Heavy cost weight → should prefer cheaper model
    let router_cost = PrismRouter::new(reg1).with_strategy(ModelSelectionStrategy::Auto {
        objective: RoutingObjective::Balanced {
            cost_weight: 0.8,
            quality_weight: 0.1,
            latency_weight: 0.1,
        },
        pool_ids: None,
        required_capabilities: None,
    });

    // Heavy quality weight → should prefer S-tier model
    let router_quality = PrismRouter::new(reg2).with_strategy(ModelSelectionStrategy::Auto {
        objective: RoutingObjective::Balanced {
            cost_weight: 0.1,
            quality_weight: 0.8,
            latency_weight: 0.1,
        },
        pool_ids: None,
        required_capabilities: None,
    });

    let req = simple_request("Tell me about machine learning");
    let decision_cost = router_cost.route(&req).unwrap();
    let decision_quality = router_quality.route(&req).unwrap();

    // Cost-heavy should pick cheaper; quality-heavy should pick S-tier
    let cheap_models = ["deepseek-lite", "gpt-4o-mini", "deepseek-v3"];
    let quality_models = ["gpt-4o", "claude-sonnet"];

    assert!(
        cheap_models.contains(&decision_cost.selected_model.as_str()),
        "Cost-heavy selected: {}",
        decision_cost.selected_model
    );
    assert!(
        quality_models.contains(&decision_quality.selected_model.as_str()),
        "Quality-heavy selected: {}",
        decision_quality.selected_model
    );
}

#[test]
fn test_cost_first_selects_cheapest() {
    let reg = build_test_registry();
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

    // deepseek-lite is the cheapest (0.07 + 0.3)
    assert_eq!(decision.selected_model, "deepseek-lite");
}

#[test]
fn test_slo_first_filters_high_latency() {
    let reg = build_test_registry();
    let router = PrismRouter::new(reg).with_strategy(ModelSelectionStrategy::Auto {
        objective: RoutingObjective::SloFirst {
            max_latency_ms: 600, // filters out claude (p99=1500) and gpt-4o (p99=1200)
            max_ttft_ms: 500,
            min_quality_score: 0.0,
        },
        pool_ids: None,
        required_capabilities: None,
    });

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // gpt-4o (p99=1200) and claude (p99=1500) should be filtered out
    let low_latency_models = ["gpt-4o-mini", "deepseek-lite", "deepseek-v3"];
    assert!(
        low_latency_models.contains(&decision.selected_model.as_str()),
        "SLO selected high-latency model: {}",
        decision.selected_model
    );
}

#[test]
fn test_pool_scoping() {
    let reg = build_test_registry();
    let router = PrismRouter::new(reg).with_strategy(ModelSelectionStrategy::Auto {
        objective: RoutingObjective::QualityFirst { max_latency_ms: None },
        pool_ids: Some(vec!["tier-2".into()]),
        required_capabilities: None,
    });

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // Only tier-2 models should be candidates
    let tier2_models = ["gpt-4o-mini", "deepseek-v3"];
    assert!(
        tier2_models.contains(&decision.selected_model.as_str()),
        "Expected tier-2 model, got {}",
        decision.selected_model
    );
}

#[test]
fn test_decision_has_fallback_chain() {
    let reg = build_test_registry();
    let router = PrismRouter::new(reg);

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // Should have fallback candidates
    assert!(!decision.fallback_chain.is_empty(), "fallback chain should not be empty");
}

#[test]
fn test_decision_time_is_reasonable() {
    let reg = build_test_registry();
    let router = PrismRouter::new(reg);

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // Decision should take < 5ms (5000 us)
    assert!(
        decision.decision_time_us < 5000,
        "Decision took {}us, expected < 5000us",
        decision.decision_time_us
    );
}
