use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

use prism_core::analyzer::ChatMessage;
use prism_core::registry::InMemoryRegistry;
use prism_core::router::{PrismRouter, RouteMetadata, RouteRequest, Router};
use prism_core::strategy::{ModelSelectionStrategy, RoutingObjective};
use prism_core::types::*;

// ── Helpers ──────────────────────────────────────

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
            supports_vision: tier == QualityTier::S,
            supports_json_mode: true,
            supports_reasoning: false,
        },
        pricing: ModelPricing {
            input_per_1m_tokens: match tier {
                QualityTier::S => 5.0,
                QualityTier::A => 2.5,
                QualityTier::B => 0.5,
                QualityTier::C => 0.1,
            },
            output_per_1m_tokens: match tier {
                QualityTier::S => 15.0,
                QualityTier::A => 10.0,
                QualityTier::B => 1.5,
                QualityTier::C => 0.3,
            },
            currency: "USD".into(),
        },
        specialties: vec![TaskType::Reasoning, TaskType::CodeGeneration],
        limits: None,
        status: ModelStatus::Available,
    }
}

fn make_variant(model_id: &str, provider_id: &str, latency: u32) -> ModelVariant {
    let ep_id = format!("{provider_id}-ep");
    ModelVariant {
        id: format!("{model_id}@{provider_id}"),
        model_id: model_id.to_string(),
        endpoint_id: ep_id,
        account_id: Some(format!("{provider_id}-acct")),
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
        pricing_override: None,
        status: VariantStatus::Active,
        base_url: None,
        upstream_model_name: None,
    }
}

/// Build a registry with N providers × M models = N*M variants
fn build_registry(num_providers: usize, num_models: usize) -> InMemoryRegistry {
    let mut reg = InMemoryRegistry::new();
    let tiers = [
        QualityTier::S,
        QualityTier::A,
        QualityTier::B,
        QualityTier::C,
    ];

    for i in 0..num_providers {
        let prov_id = format!("provider-{i}");
        reg.add_provider(make_provider(&prov_id));
        reg.add_endpoint(make_endpoint(&format!("{prov_id}-ep"), &prov_id));
        reg.add_account(make_account(
            &format!("{prov_id}-acct"),
            &format!("{prov_id}-ep"),
        ));
    }

    for j in 0..num_models {
        let model_id = format!("model-{j}");
        let tier = tiers[j % tiers.len()];
        reg.add_model(make_model(&model_id, tier));

        for i in 0..num_providers {
            let prov_id = format!("provider-{i}");
            let latency = 100 + (i * 50 + j * 30) as u32;
            reg.add_variant(make_variant(&model_id, &prov_id, latency));
        }
    }

    // Add a default pool with all variants
    let variant_ids: Vec<String> = (0..num_models)
        .flat_map(|j| (0..num_providers).map(move |i| format!("model-{j}@provider-{i}")))
        .collect();
    reg.add_pool(ModelPool {
        id: "default".into(),
        name: "Default Pool".into(),
        variants: variant_ids,
        selection_mode: SelectionMode::default(),
        description: None,
    });

    reg
}

fn build_router(num_providers: usize, num_models: usize) -> PrismRouter {
    PrismRouter::new(build_registry(num_providers, num_models))
}

fn simple_request() -> RouteRequest {
    RouteRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Hello, world!".to_string(),
        }],
        strategy: None,
        metadata: RouteMetadata::default(),
        analyze_context: None,
        required_protocol: None,
    }
}

fn complex_request() -> RouteRequest {
    RouteRequest {
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content:
                    "You are an expert software architect specializing in distributed systems."
                        .to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Design a microservices architecture for a real-time trading platform \
                    that handles 10 million orders per day. Include event sourcing, CQRS, \
                    and discuss trade-offs between consistency and availability. Consider \
                    the CAP theorem implications and suggest appropriate database choices \
                    for each service. Please provide code examples in Rust."
                    .to_string(),
            },
        ],
        strategy: None,
        metadata: RouteMetadata::default(),
        analyze_context: None,
        required_protocol: None,
    }
}

// ── Benchmarks ──────────────────────────────────

fn bench_routing_scales(c: &mut Criterion) {
    let mut group = c.benchmark_group("routing/scale");

    for (providers, models) in [(1, 1), (3, 3), (5, 5), (10, 10), (10, 20), (20, 20)] {
        let total = providers * models;
        let router = build_router(providers, models);

        group.bench_with_input(
            BenchmarkId::new("route", format!("{providers}p×{models}m={total}v")),
            &router,
            |b, router| {
                let req = simple_request();
                b.iter(|| black_box(router.route(&req).unwrap()));
            },
        );
    }

    group.finish();
}

fn bench_routing_strategies(c: &mut Criterion) {
    let mut group = c.benchmark_group("routing/strategy");

    let strategies: Vec<(&str, ModelSelectionStrategy)> = vec![
        ("auto_default", ModelSelectionStrategy::default()),
        (
            "cost_first",
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::CostFirst {
                    min_quality_score: 0.5,
                    budget_per_request: None,
                },
                pool_ids: None,
                required_capabilities: None,
            },
        ),
        (
            "quality_first",
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::QualityFirst {
                    max_latency_ms: None,
                },
                pool_ids: None,
                required_capabilities: None,
            },
        ),
        (
            "slo_first",
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::SloFirst {
                    max_latency_ms: 500,
                    max_ttft_ms: 200,
                    min_quality_score: 0.7,
                },
                pool_ids: None,
                required_capabilities: None,
            },
        ),
        (
            "pinned",
            ModelSelectionStrategy::Pinned {
                model_id: "model-0".to_string(),
            },
        ),
    ];

    for (name, strategy) in &strategies {
        let mut router = PrismRouter::new(build_registry(5, 5));
        router.default_strategy = strategy.clone();

        group.bench_with_input(
            BenchmarkId::new("simple_msg", name),
            &router,
            |b, router| {
                let req = simple_request();
                b.iter(|| black_box(router.route(&req).unwrap()));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("complex_msg", name),
            &router,
            |b, router| {
                let req = complex_request();
                b.iter(|| black_box(router.route(&req).unwrap()));
            },
        );
    }

    group.finish();
}

fn bench_analyzer(c: &mut Criterion) {
    let mut group = c.benchmark_group("analyzer");

    let router = build_router(3, 3);

    group.bench_function("short_msg/2_chars", |b| {
        let msgs = vec![ChatMessage {
            role: "user".to_string(),
            content: "Hi".to_string(),
        }];
        b.iter(|| black_box(router.analyzer.analyze(&msgs)));
    });

    group.bench_function("medium_msg/65_chars", |b| {
        let msgs = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You are a helpful assistant.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Explain the difference between TCP and UDP protocols in networking."
                    .to_string(),
            },
        ];
        b.iter(|| black_box(router.analyzer.analyze(&msgs)));
    });

    group.bench_function("long_code/2500_chars", |b| {
        let code = "fn main() {\n    println!(\"hello\");\n}\n".repeat(50);
        let msgs = vec![ChatMessage {
            role: "user".to_string(),
            content: format!("Review this code and fix bugs:\n```rust\n{code}\n```"),
        }];
        b.iter(|| black_box(router.analyzer.analyze(&msgs)));
    });

    group.bench_function("multi_turn/10_msgs", |b| {
        let msgs: Vec<ChatMessage> = (0..10)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content: format!(
                    "This is message number {i} in the conversation with moderate length content."
                ),
            })
            .collect();
        b.iter(|| black_box(router.analyzer.analyze(&msgs)));
    });

    group.finish();
}

fn bench_plugin_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("plugin_pipeline");

    for (providers, models) in [(3, 3), (5, 5), (10, 10), (20, 20)] {
        let total = providers * models;
        let router = build_router(providers, models);

        group.bench_with_input(
            BenchmarkId::new("full", format!("{total}_variants")),
            &router,
            |b, router| {
                let req = simple_request();
                b.iter(|| black_box(router.route(&req).unwrap()));
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_routing_scales,
    bench_routing_strategies,
    bench_analyzer,
    bench_plugin_pipeline,
);
criterion_main!(benches);
