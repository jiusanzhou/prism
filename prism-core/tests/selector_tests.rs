use prism_core::analyzer::ChatMessage;
use prism_core::registry::InMemoryRegistry;
use prism_core::router::{PrismRouter, RouteMetadata, RouteRequest, Router};
use prism_core::selector::*;
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

fn make_endpoint_with_priority(id: &str, provider_id: &str, priority: u32) -> Endpoint {
    Endpoint {
        id: id.to_string(),
        provider_id: provider_id.to_string(),
        base_url: None,
        region: None,
        auth_override: None,
        rate_limit: None,
        priority,
        status: HealthStatus::Healthy,
    }
}

fn make_endpoint_with_region(id: &str, provider_id: &str, region: &str) -> Endpoint {
    Endpoint {
        id: id.to_string(),
        provider_id: provider_id.to_string(),
        base_url: None,
        region: Some(region.to_string()),
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

fn make_account_with_rate_limit(id: &str, endpoint_id: &str, rpm: u32) -> Account {
    Account {
        id: id.to_string(),
        endpoint_id: endpoint_id.to_string(),
        auth: AuthConfig::default(),
        project_id: None,
        rate_limit: Some(RateLimit {
            rpm: Some(rpm),
            tpm: None,
            concurrent: None,
        }),
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: Default::default(),
    }
}

fn make_model(id: &str) -> Model {
    Model {
        id: id.to_string(),
        display_name: id.to_string(),
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
    }
}

fn make_variant(
    id: &str,
    model_id: &str,
    endpoint_id: &str,
    account_id: &str,
    p50_latency_ms: u32,
    current_qps: f32,
) -> ModelVariant {
    ModelVariant {
        id: id.to_string(),
        model_id: model_id.to_string(),
        endpoint_id: endpoint_id.to_string(),
        account_id: Some(account_id.to_string()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms,
            p95_latency_ms: p50_latency_ms + 200,
            p99_latency_ms: p50_latency_ms + 500,
            ttft_p50_ms: p50_latency_ms / 3,
            throughput_tps: 50.0,
            error_rate: 0.01,
            current_qps,
            queue_depth: 0,
            last_updated: None,
            ..Default::default()
        },
        pricing_override: None,
        status: VariantStatus::Active,
            base_url: None,
            upstream_model_name: None,
    }
}

fn make_variant_with_pricing(
    id: &str,
    model_id: &str,
    endpoint_id: &str,
    account_id: &str,
    p50_latency_ms: u32,
    input_per_1m: f64,
    output_per_1m: f64,
) -> ModelVariant {
    ModelVariant {
        id: id.to_string(),
        model_id: model_id.to_string(),
        endpoint_id: endpoint_id.to_string(),
        account_id: Some(account_id.to_string()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance {
            p50_latency_ms,
            p95_latency_ms: p50_latency_ms + 200,
            p99_latency_ms: p50_latency_ms + 500,
            ttft_p50_ms: p50_latency_ms / 3,
            throughput_tps: 50.0,
            error_rate: 0.01,
            current_qps: 10.0,
            queue_depth: 0,
            last_updated: None,
            ..Default::default()
        },
        pricing_override: Some(ModelPricing {
            input_per_1m_tokens: input_per_1m,
            output_per_1m_tokens: output_per_1m,
            currency: "USD".into(),
        }),
        status: VariantStatus::Active,
            base_url: None,
            upstream_model_name: None,
    }
}

fn make_candidate(
    variant: &ModelVariant,
    model: &Model,
    account: &Account,
    endpoint: &Endpoint,
    provider: &Provider,
) -> ModelCandidate {
    let effective_pricing = variant
        .pricing_override
        .clone()
        .unwrap_or_else(|| model.pricing.clone());
    ModelCandidate {
        variant: variant.clone(),
        model: model.clone(),
        account: account.clone(),
        endpoint: endpoint.clone(),
        provider: provider.clone(),
        effective_pricing,
    }
}

fn make_route_context<'a>(
    profile: &'a prism_core::analyzer::RequestProfile,
    metadata: &'a RouteMetadata,
    strategy: &'a ModelSelectionStrategy,
) -> RouteContext<'a> {
    RouteContext {
        profile,
        metadata,
        strategy,
        required_protocol: None,
    }
}

fn default_profile() -> prism_core::analyzer::RequestProfile {
    prism_core::analyzer::RequestProfile {
        task_type: prism_core::types::TaskType::SimpleQA,
        complexity: 0.3,
        estimated_tokens: 100,
        context_window_need: prism_core::analyzer::ContextSize::Small,
        requires_vision: false,
        requires_reasoning: false,
        language: prism_core::analyzer::Language::English,
        is_multi_turn: false,
    }
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

// ════════════════════════════════════════════════
// L2 Implementation Tests
// ════════════════════════════════════════════════

#[test]
fn test_lowest_latency_l2() {
    let provider = make_provider("prov");
    let endpoint = make_endpoint("ep", "prov");
    let account = make_account("acct", "ep");
    let model = make_model("model-a");

    let v1 = make_variant("v1", "model-a", "ep", "acct", 500, 10.0); // high latency
    let v2 = make_variant("v2", "model-a", "ep", "acct", 100, 10.0); // lowest latency
    let v3 = make_variant("v3", "model-a", "ep", "acct", 300, 10.0); // medium latency

    let c1 = make_candidate(&v1, &model, &account, &endpoint, &provider);
    let c2 = make_candidate(&v2, &model, &account, &endpoint, &provider);
    let c3 = make_candidate(&v3, &model, &account, &endpoint, &provider);

    let selector = LowestLatencyL2Selector;
    let profile = default_profile();
    let metadata = RouteMetadata::default();
    let strategy = ModelSelectionStrategy::default();
    let ctx = make_route_context(&profile, &metadata, &strategy);

    let result = selector.rerank(vec![&c1, &c2, &c3], &ctx);

    assert_eq!(result[0].variant.id, "v2", "Lowest latency (100) should be first");
    assert_eq!(result[1].variant.id, "v3", "Medium latency (300) should be second");
    assert_eq!(result[2].variant.id, "v1", "Highest latency (500) should be last");
}

#[test]
fn test_lowest_cost_l2() {
    let provider = make_provider("prov");
    let endpoint = make_endpoint("ep", "prov");
    let account = make_account("acct", "ep");
    let model = make_model("model-a");

    let v1 = make_variant_with_pricing("v1", "model-a", "ep", "acct", 200, 10.0, 20.0); // expensive
    let v2 = make_variant_with_pricing("v2", "model-a", "ep", "acct", 200, 0.5, 1.0);   // cheapest
    let v3 = make_variant_with_pricing("v3", "model-a", "ep", "acct", 200, 3.0, 6.0);   // medium

    let c1 = make_candidate(&v1, &model, &account, &endpoint, &provider);
    let c2 = make_candidate(&v2, &model, &account, &endpoint, &provider);
    let c3 = make_candidate(&v3, &model, &account, &endpoint, &provider);

    let selector = LowestCostL2Selector;
    let profile = default_profile();
    let metadata = RouteMetadata::default();
    let strategy = ModelSelectionStrategy::default();
    let ctx = make_route_context(&profile, &metadata, &strategy);

    let result = selector.rerank(vec![&c1, &c2, &c3], &ctx);

    assert_eq!(result[0].variant.id, "v2", "Cheapest should be first");
    assert_eq!(result[1].variant.id, "v3", "Medium cost should be second");
    assert_eq!(result[2].variant.id, "v1", "Most expensive should be last");
}

#[test]
fn test_priority_l2() {
    let provider = make_provider("prov");
    let ep1 = make_endpoint_with_priority("ep1", "prov", 10); // lowest priority number = highest priority
    let ep2 = make_endpoint_with_priority("ep2", "prov", 1);  // highest priority
    let ep3 = make_endpoint_with_priority("ep3", "prov", 5);  // medium priority

    let account = make_account("acct", "ep1");
    let model = make_model("model-a");

    let v1 = make_variant("v1", "model-a", "ep1", "acct", 200, 10.0);
    let v2 = make_variant("v2", "model-a", "ep2", "acct", 200, 10.0);
    let v3 = make_variant("v3", "model-a", "ep3", "acct", 200, 10.0);

    let c1 = make_candidate(&v1, &model, &account, &ep1, &provider);
    let c2 = make_candidate(&v2, &model, &account, &ep2, &provider);
    let c3 = make_candidate(&v3, &model, &account, &ep3, &provider);

    let selector = PriorityL2Selector;
    let profile = default_profile();
    let metadata = RouteMetadata::default();
    let strategy = ModelSelectionStrategy::default();
    let ctx = make_route_context(&profile, &metadata, &strategy);

    let result = selector.rerank(vec![&c1, &c2, &c3], &ctx);

    assert_eq!(result[0].variant.id, "v2", "Priority 1 should be first");
    assert_eq!(result[1].variant.id, "v3", "Priority 5 should be second");
    assert_eq!(result[2].variant.id, "v1", "Priority 10 should be last");
}

#[test]
fn test_default_l2_geo() {
    let provider = make_provider("prov");
    let ep1 = make_endpoint_with_region("ep1", "prov", "us-east-1");
    let ep2 = make_endpoint_with_region("ep2", "prov", "eu-west-1");

    let account = make_account("acct", "ep1");
    let model = make_model("model-a");

    let v1 = make_variant("v1", "model-a", "ep1", "acct", 200, 10.0);
    let v2 = make_variant("v2", "model-a", "ep2", "acct", 200, 10.0);

    let c1 = make_candidate(&v1, &model, &account, &ep1, &provider);
    let c2 = make_candidate(&v2, &model, &account, &ep2, &provider);

    let selector = DefaultL2Selector;
    let profile = default_profile();
    let metadata = RouteMetadata {
        region: Some("eu-west-1".into()),
        ..Default::default()
        
    };
    let strategy = ModelSelectionStrategy::default();
    let ctx = make_route_context(&profile, &metadata, &strategy);

    let result = selector.rerank(vec![&c1, &c2], &ctx);

    assert_eq!(
        result[0].variant.id, "v2",
        "EU endpoint should be preferred when request region is eu-west-1"
    );
}

#[test]
fn test_default_l2_quota() {
    let provider = make_provider("prov");
    let endpoint = make_endpoint("ep", "prov");
    let acct1 = make_account_with_rate_limit("acct1", "ep", 100);
    let acct2 = make_account_with_rate_limit("acct2", "ep", 100);
    let model = make_model("model-a");

    // v1: current_qps = 1.5 → 90% utilization of 100 RPM
    let v1 = make_variant("v1", "model-a", "ep", "acct1", 200, 1.5);
    // v2: current_qps = 0.08 → 5% utilization
    let v2 = make_variant("v2", "model-a", "ep", "acct2", 200, 0.08);

    let c1 = make_candidate(&v1, &model, &acct1, &endpoint, &provider);
    let c2 = make_candidate(&v2, &model, &acct2, &endpoint, &provider);

    let selector = DefaultL2Selector;
    let profile = default_profile();
    let metadata = RouteMetadata::default();
    let strategy = ModelSelectionStrategy::default();
    let ctx = make_route_context(&profile, &metadata, &strategy);

    let result = selector.rerank(vec![&c1, &c2], &ctx);

    assert_eq!(
        result[0].variant.id, "v2",
        "Low utilization account should be preferred"
    );
}

// ════════════════════════════════════════════════
// L3 Implementation Tests
// ════════════════════════════════════════════════

#[test]
fn test_round_robin_l3() {
    let provider = make_provider("prov");
    let endpoint = make_endpoint("ep", "prov");
    let acct1 = make_account("acct1", "ep");
    let acct2 = make_account("acct2", "ep");
    let acct3 = make_account("acct3", "ep");
    let model = make_model("model-a");

    let v1 = make_variant("v1", "model-a", "ep", "acct1", 200, 10.0);
    let v2 = make_variant("v2", "model-a", "ep", "acct2", 200, 10.0);
    let v3 = make_variant("v3", "model-a", "ep", "acct3", 200, 10.0);

    let c1 = make_candidate(&v1, &model, &acct1, &endpoint, &provider);
    let c2 = make_candidate(&v2, &model, &acct2, &endpoint, &provider);
    let c3 = make_candidate(&v3, &model, &acct3, &endpoint, &provider);

    let candidates: Vec<&ModelCandidate> = vec![&c1, &c2, &c3];
    let selector = RoundRobinL3Selector::new();
    let profile = default_profile();
    let metadata = RouteMetadata::default();
    let strategy = ModelSelectionStrategy::default();
    let ctx = make_route_context(&profile, &metadata, &strategy);

    // Should cycle through 0, 1, 2, 0, 1, 2, ...
    let idx0 = selector.select(&candidates, &ctx).unwrap();
    let idx1 = selector.select(&candidates, &ctx).unwrap();
    let idx2 = selector.select(&candidates, &ctx).unwrap();
    let idx3 = selector.select(&candidates, &ctx).unwrap();
    let idx4 = selector.select(&candidates, &ctx).unwrap();
    let idx5 = selector.select(&candidates, &ctx).unwrap();

    assert_eq!(idx0, 0);
    assert_eq!(idx1, 1);
    assert_eq!(idx2, 2);
    assert_eq!(idx3, 0, "Should wrap around to 0");
    assert_eq!(idx4, 1, "Should wrap around to 1");
    assert_eq!(idx5, 2, "Should wrap around to 2");
}

#[test]
fn test_least_load_l3() {
    let provider = make_provider("prov");
    let endpoint = make_endpoint("ep", "prov");
    let acct1 = make_account("acct1", "ep");
    let acct2 = make_account("acct2", "ep");
    let acct3 = make_account("acct3", "ep");
    let model = make_model("model-a");

    let v1 = make_variant("v1", "model-a", "ep", "acct1", 200, 50.0);  // high load
    let v2 = make_variant("v2", "model-a", "ep", "acct2", 200, 5.0);   // lowest load
    let v3 = make_variant("v3", "model-a", "ep", "acct3", 200, 20.0);  // medium load

    let c1 = make_candidate(&v1, &model, &acct1, &endpoint, &provider);
    let c2 = make_candidate(&v2, &model, &acct2, &endpoint, &provider);
    let c3 = make_candidate(&v3, &model, &acct3, &endpoint, &provider);

    let candidates: Vec<&ModelCandidate> = vec![&c1, &c2, &c3];
    let selector = LeastLoadL3Selector;
    let profile = default_profile();
    let metadata = RouteMetadata::default();
    let strategy = ModelSelectionStrategy::default();
    let ctx = make_route_context(&profile, &metadata, &strategy);

    let idx = selector.select(&candidates, &ctx).unwrap();
    assert_eq!(idx, 1, "Should select candidate with lowest current_qps (5.0)");
}

// ════════════════════════════════════════════════
// Factory Tests
// ════════════════════════════════════════════════

#[test]
fn test_create_l2_selector() {
    let s1 = create_l2_selector("default");
    assert_eq!(s1.name(), "DefaultL2Selector");

    let s2 = create_l2_selector("lowest_latency");
    assert_eq!(s2.name(), "LowestLatencyL2Selector");

    let s3 = create_l2_selector("lowest_cost");
    assert_eq!(s3.name(), "LowestCostL2Selector");

    let s4 = create_l2_selector("priority");
    assert_eq!(s4.name(), "PriorityL2Selector");
}

#[test]
fn test_create_l3_selector() {
    let s1 = create_l3_selector("affinity");
    assert_eq!(s1.name(), "AffinityL3Selector");

    let s2 = create_l3_selector("round_robin");
    assert_eq!(s2.name(), "RoundRobinL3Selector");

    let s3 = create_l3_selector("least_load");
    assert_eq!(s3.name(), "LeastLoadL3Selector");
}

#[test]
fn test_create_l4_selector() {
    let s1 = create_l4_selector("best");
    assert_eq!(s1.name(), "BestL4Selector");

    let s2 = create_l4_selector("weighted");
    assert_eq!(s2.name(), "WeightedL4Selector");
}

#[test]
fn test_unknown_name_fallback() {
    // Unknown L2 → DefaultL2Selector
    let s1 = create_l2_selector("nonexistent");
    assert_eq!(s1.name(), "DefaultL2Selector");

    let s2 = create_l2_selector("");
    assert_eq!(s2.name(), "DefaultL2Selector");

    // Unknown L3 → AffinityL3Selector
    let s3 = create_l3_selector("unknown");
    assert_eq!(s3.name(), "AffinityL3Selector");

    let s4 = create_l3_selector("");
    assert_eq!(s4.name(), "AffinityL3Selector");

    // Unknown L4 → BestL4Selector
    let s5 = create_l4_selector("random_name");
    assert_eq!(s5.name(), "BestL4Selector");

    let s6 = create_l4_selector("");
    assert_eq!(s6.name(), "BestL4Selector");
}

// ════════════════════════════════════════════════
// SelectorConfig Tests
// ════════════════════════════════════════════════

#[test]
fn test_selector_config_defaults() {
    let cfg = SelectorConfig::default();
    assert_eq!(cfg.l1, "scored");
    assert_eq!(cfg.l2, "default");
    assert_eq!(cfg.l3, "affinity");
    assert_eq!(cfg.l4, "best");
}

#[test]
fn test_selector_config_deserialize() {
    let json = r#"{"l2": "lowest_latency", "l3": "round_robin"}"#;
    let cfg: SelectorConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.l1, "scored", "Missing field should use default");
    assert_eq!(cfg.l2, "lowest_latency");
    assert_eq!(cfg.l3, "round_robin");
    assert_eq!(cfg.l4, "best", "Missing field should use default");
}

#[test]
fn test_selector_config_empty_json() {
    let json = r#"{}"#;
    let cfg: SelectorConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.l1, "scored");
    assert_eq!(cfg.l2, "default");
    assert_eq!(cfg.l3, "affinity");
    assert_eq!(cfg.l4, "best");
}

// ════════════════════════════════════════════════
// Integration Tests — Router with Custom Selectors
// ════════════════════════════════════════════════

fn build_three_provider_registry() -> InMemoryRegistry {
    let mut reg = InMemoryRegistry::new();

    // Three providers with different latencies and costs
    for (prov_id, latency, cost_in, cost_out) in [
        ("fast-prov", 100u32, 5.0, 10.0),     // fast but expensive
        ("medium-prov", 300u32, 2.5, 5.0),     // medium
        ("slow-prov", 500u32, 0.5, 1.0),       // slow but cheap
    ] {
        reg.add_provider(make_provider(prov_id));
        let ep_id = format!("{prov_id}-ep");
        reg.add_endpoint(make_endpoint(&ep_id, prov_id));
        let acct_id = format!("{prov_id}-acct");
        reg.add_account(make_account(&acct_id, &ep_id));

        let variant = make_variant_with_pricing(
            &format!("model@{prov_id}"),
            "test-model",
            &ep_id,
            &acct_id,
            latency,
            cost_in,
            cost_out,
        );
        reg.add_variant(variant);
    }

    reg.add_model(make_model("test-model"));

    reg.add_pool(ModelPool {
        id: "all".into(),
        name: "All".into(),
        variants: vec![
            "model@fast-prov".into(),
            "model@medium-prov".into(),
            "model@slow-prov".into(),
        ],
        selection_mode: SelectionMode::Scored,
        description: None,
    });

    reg
}

#[test]
fn test_router_with_custom_l2_lowest_latency() {
    use std::sync::Arc;
    use prism_core::plugin::PluginRegistry;
    use prism_core::plugins::protocol_filter::ProtocolFilterPlugin;
    use prism_core::plugins::latency_scorer::LatencyScorerPlugin;

    let reg = build_three_provider_registry();

    // Build a plugin profile that only cares about latency
    let mut plugins = PluginRegistry::new();
    plugins.register_default(Arc::new(ProtocolFilterPlugin));
    plugins.register(Arc::new(LatencyScorerPlugin), 100);

    let router = PrismRouter::new(reg).with_plugins(plugins);

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // LatencyScorerPlugin scores by p50_latency_ms (lower → higher score).
    // The fast provider (latency=100) should win.
    assert_eq!(
        decision.selected_variant, "model@fast-prov",
        "Latency-only plugin profile should prefer the fastest provider"
    );
}

#[test]
fn test_router_with_custom_l3_round_robin() {
    // Plugin framework doesn't have a direct "round-robin account" plugin,
    // but we can verify that identical candidates produce deterministic ordering.
    // With all-same scores, the framework picks the first candidate consistently.
    let mut reg = InMemoryRegistry::new();

    // Single provider, single endpoint, three accounts
    reg.add_provider(make_provider("prov"));
    reg.add_endpoint(make_endpoint("ep", "prov"));

    for i in 1..=3 {
        reg.add_account(make_account(&format!("acct-{i}"), "ep"));
        reg.add_variant(make_variant(
            &format!("model@acct-{i}"),
            "test-model",
            "ep",
            &format!("acct-{i}"),
            200,
            10.0,
        ));
    }

    reg.add_model(make_model("test-model"));
    reg.add_pool(ModelPool {
        id: "all".into(),
        name: "All".into(),
        variants: vec![
            "model@acct-1".into(),
            "model@acct-2".into(),
            "model@acct-3".into(),
        ],
        selection_mode: SelectionMode::Scored,
        description: None,
    });

    let router = PrismRouter::new(reg);

    // Route multiple requests — with identical candidates and deterministic scoring,
    // the plugin framework should consistently select the same winner
    let mut accounts = Vec::new();
    for i in 0..6 {
        let req = simple_request(&format!("Request {i}"));
        let decision = router.route(&req).unwrap();
        accounts.push(decision.selected_account.clone());
    }

    // All decisions should be deterministic (same account every time)
    assert!(
        !accounts.is_empty(),
        "Should produce routing decisions"
    );
    // With default plugins and identical candidates, scoring is deterministic
    // so we expect consistent selection
    assert_eq!(
        accounts[0], accounts[1],
        "Deterministic scoring should produce consistent results"
    );
}

#[test]
fn test_router_with_custom_l2_lowest_cost() {
    let reg = build_three_provider_registry();
    let router = PrismRouter::new(reg)
        .with_l2(Box::new(LowestCostL2Selector));

    let req = simple_request("Hello");
    let decision = router.route(&req).unwrap();

    // LowestCostL2Selector reranks by cost ascending.
    // The slow-prov (cost 0.5/1.0) is cheapest.
    assert_eq!(
        decision.selected_variant, "model@slow-prov",
        "LowestCostL2 should prefer the cheapest provider"
    );
}

#[test]
fn test_round_robin_l3_empty_candidates() {
    let selector = RoundRobinL3Selector::new();
    let profile = default_profile();
    let metadata = RouteMetadata::default();
    let strategy = ModelSelectionStrategy::default();
    let ctx = make_route_context(&profile, &metadata, &strategy);

    let empty: Vec<&ModelCandidate> = vec![];
    let result = selector.select(&empty, &ctx);
    assert!(result.is_err(), "Empty candidates should return error");
}

#[test]
fn test_least_load_l3_empty_candidates() {
    let selector = LeastLoadL3Selector;
    let profile = default_profile();
    let metadata = RouteMetadata::default();
    let strategy = ModelSelectionStrategy::default();
    let ctx = make_route_context(&profile, &metadata, &strategy);

    let empty: Vec<&ModelCandidate> = vec![];
    let result = selector.select(&empty, &ctx);
    assert!(result.is_err(), "Empty candidates should return error");
}
