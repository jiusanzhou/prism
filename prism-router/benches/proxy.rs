use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::time::Instant;

use prism_core::registry::InMemoryRegistry;
use prism_core::router::PrismRouter;
use prism_core::types::*;

use prism_router::codec::chain::ProxyPluginChain;
use prism_router::codec::registry::CodecRegistry;
use prism_router::codec::{
    Action, ProxyContext as CodecProxyContext, ProxyPlugin as CodecProxyPlugin,
};
use prism_router::metrics::{MetricsCollector, RequestLabels, RequestRecord};
use prism_router::ratelimit::{RateLimitOverride, RateLimitRule, RateLimiter};
use prism_router::state::AppState;

// ════════════════════════════════════════════════
// Helpers
// ════════════════════════════════════════════════

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
            input_per_1m_tokens: 2.5,
            output_per_1m_tokens: 10.0,
            currency: "USD".into(),
        },
        specialties: vec![],
        limits: None,
        status: ModelStatus::Available,
    }
}

fn make_variant(model_id: &str, provider_id: &str) -> ModelVariant {
    let ep_id = format!("{provider_id}-ep");
    ModelVariant {
        id: format!("{model_id}@{provider_id}"),
        model_id: model_id.to_string(),
        endpoint_id: ep_id,
        account_id: Some(format!("{provider_id}-acct")),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance::default(),
        pricing_override: None,
        status: VariantStatus::Active,
        base_url: None,
        upstream_model_name: None,
    }
}

fn build_registry(n_providers: usize, n_models: usize) -> InMemoryRegistry {
    let mut reg = InMemoryRegistry::new();
    for i in 0..n_providers {
        let pid = format!("provider-{i}");
        reg.add_provider(make_provider(&pid));
        reg.add_endpoint(Endpoint {
            id: format!("{pid}-ep"),
            provider_id: pid.clone(),
            base_url: None,
            region: None,
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        });
        reg.add_account(Account {
            id: format!("{pid}-acct"),
            endpoint_id: format!("{pid}-ep"),
            auth: AuthConfig::default(),
            project_id: None,
            rate_limit: None,
            billing_tag: None,
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: Default::default(),
        });
    }
    let tiers = [
        QualityTier::S,
        QualityTier::A,
        QualityTier::B,
        QualityTier::C,
    ];
    let mut variant_ids = Vec::new();
    for j in 0..n_models {
        let mid = format!("model-{j}");
        reg.add_model(make_model(&mid, tiers[j % tiers.len()]));
        for i in 0..n_providers {
            let pid = format!("provider-{i}");
            reg.add_variant(make_variant(&mid, &pid));
            variant_ids.push(format!("{mid}@{pid}"));
        }
    }
    reg.add_pool(ModelPool {
        id: "default".into(),
        name: "Default Pool".into(),
        variants: variant_ids,
        selection_mode: SelectionMode::default(),
        description: None,
    });
    reg
}

fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// ════════════════════════════════════════════════
// Mock codec plugin for benchmark
// ════════════════════════════════════════════════

struct BenchCodecPlugin {
    name: String,
}

impl BenchCodecPlugin {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

impl CodecProxyPlugin for BenchCodecPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_request_headers(&self, ctx: &mut CodecProxyContext) -> Action {
        ctx.request_headers
            .insert("x-converted".to_string(), "true".to_string());
        Action::Continue
    }

    fn on_request_body(
        &self,
        _ctx: &mut CodecProxyContext,
        body: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        // Simulate JSON body transformation: parse + rebuild
        if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&body) {
            json["_converted"] = serde_json::Value::Bool(true);
            Ok(serde_json::to_vec(&json).unwrap_or(body))
        } else {
            Ok(body)
        }
    }

    fn on_response_headers(&self, _ctx: &mut CodecProxyContext) -> Action {
        Action::Continue
    }

    fn on_response_body(
        &self,
        _ctx: &mut CodecProxyContext,
        body: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&body) {
            json["_converted_back"] = serde_json::Value::Bool(true);
            Ok(serde_json::to_vec(&json).unwrap_or(body))
        } else {
            Ok(body)
        }
    }

    fn on_response_stream_chunk(
        &self,
        _ctx: &mut CodecProxyContext,
        chunk: &str,
    ) -> Result<Option<String>, String> {
        Ok(Some(chunk.to_string()))
    }
}

// ════════════════════════════════════════════════
// 1. Rate Limiter Benchmarks
// ════════════════════════════════════════════════

fn bench_ratelimit(c: &mut Criterion) {
    let mut group = c.benchmark_group("ratelimit");

    // Single global rule
    group.bench_function("global/single_rule", |b| {
        let limiter = RateLimiter::new(vec![RateLimitRule {
            name: "global".to_string(),
            key_headers: vec![],
            window_secs: 60,
            max_requests: Some(1_000_000),
            max_tokens: None,
            quotas: vec![],
            overrides: vec![],
            enabled: true,
            status_code: 429,
            message: "Rate limit exceeded".to_string(),
        }]);
        b.iter(|| black_box(limiter.check(&headers(&[]))));
    });

    // Per-model keyed rule
    group.bench_function("keyed/per_model", |b| {
        let limiter = RateLimiter::new(vec![RateLimitRule {
            name: "per-model".to_string(),
            key_headers: vec!["x-model".to_string()],
            window_secs: 60,
            max_requests: Some(1_000_000),
            max_tokens: None,
            quotas: vec![],
            overrides: vec![],
            enabled: true,
            status_code: 429,
            message: "Rate limit exceeded".to_string(),
        }]);
        let h = headers(&[("x-model", "gpt-4o")]);
        b.iter(|| black_box(limiter.check(&h)));
    });

    // Multi-key rule
    group.bench_function("keyed/model_app_region", |b| {
        let limiter = RateLimiter::new(vec![RateLimitRule {
            name: "multi-key".to_string(),
            key_headers: vec![
                "x-model".to_string(),
                "x-app-id".to_string(),
                "x-region".to_string(),
            ],
            window_secs: 60,
            max_requests: Some(1_000_000),
            max_tokens: None,
            quotas: vec![],
            overrides: vec![],
            enabled: true,
            status_code: 429,
            message: "Rate limit exceeded".to_string(),
        }]);
        let h = headers(&[
            ("x-model", "gpt-4o"),
            ("x-app-id", "app-1"),
            ("x-region", "us-east"),
        ]);
        b.iter(|| black_box(limiter.check(&h)));
    });

    // Multiple rules checked sequentially
    group.bench_function("multi_rules/5_rules", |b| {
        let rules: Vec<RateLimitRule> = (0..5)
            .map(|i| RateLimitRule {
                name: format!("rule-{i}"),
                key_headers: vec!["x-model".to_string()],
                window_secs: 60,
                max_requests: Some(1_000_000),
                max_tokens: None,
                quotas: vec![],
                overrides: vec![],
                enabled: true,
                status_code: 429,
                message: "Rate limit exceeded".to_string(),
            })
            .collect();
        let limiter = RateLimiter::new(rules);
        let h = headers(&[("x-model", "gpt-4o")]);
        b.iter(|| black_box(limiter.check(&h)));
    });

    // Override resolution (most-specific match)
    group.bench_function("overrides/5_overrides", |b| {
        let limiter = RateLimiter::new(vec![RateLimitRule {
            name: "with-overrides".to_string(),
            key_headers: vec!["x-model".to_string(), "x-app-id".to_string()],
            window_secs: 60,
            max_requests: Some(1_000_000),
            max_tokens: None,
            quotas: vec![],
            overrides: vec![
                RateLimitOverride {
                    match_headers: [("x-model".to_string(), "gpt-4".to_string())].into(),
                    max_requests: Some(500),
                    max_tokens: None,
                    quotas: vec![],
                },
                RateLimitOverride {
                    match_headers: [("x-model".to_string(), "claude-3".to_string())].into(),
                    max_requests: Some(300),
                    max_tokens: None,
                    quotas: vec![],
                },
                RateLimitOverride {
                    match_headers: [
                        ("x-model".to_string(), "gpt-4".to_string()),
                        ("x-app-id".to_string(), "premium-app".to_string()),
                    ]
                    .into(),
                    max_requests: Some(2000),
                    max_tokens: None,
                    quotas: vec![],
                },
                RateLimitOverride {
                    match_headers: [("x-app-id".to_string(), "premium-app".to_string())].into(),
                    max_requests: Some(1500),
                    max_tokens: None,
                    quotas: vec![],
                },
                RateLimitOverride {
                    match_headers: [("x-model".to_string(), "llama-3".to_string())].into(),
                    max_requests: Some(5000),
                    max_tokens: None,
                    quotas: vec![],
                },
            ],
            enabled: true,
            status_code: 429,
            message: "Rate limit exceeded".to_string(),
        }]);
        let h = headers(&[("x-model", "gpt-4"), ("x-app-id", "premium-app")]);
        b.iter(|| black_box(limiter.check(&h)));
    });

    // Concurrent key cardinality (many distinct keys in DashMap)
    group.bench_function("cardinality/1000_keys_then_check", |b| {
        let limiter = RateLimiter::new(vec![RateLimitRule {
            name: "per-user".to_string(),
            key_headers: vec!["x-user-id".to_string()],
            window_secs: 60,
            max_requests: Some(1_000_000),
            max_tokens: None,
            quotas: vec![],
            overrides: vec![],
            enabled: true,
            status_code: 429,
            message: "Rate limit exceeded".to_string(),
        }]);
        // Pre-populate 1000 distinct keys
        for i in 0..1000 {
            let h = headers(&[("x-user-id", &format!("user-{i}"))]);
            let _ = limiter.check(&h);
        }
        // Benchmark checking an existing key
        let h = headers(&[("x-user-id", "user-500")]);
        b.iter(|| black_box(limiter.check(&h)));
    });

    group.finish();
}

// ════════════════════════════════════════════════
// 2. Codec Plugin Chain Benchmarks
// ════════════════════════════════════════════════

fn bench_codec_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("codec");

    let small_body = br#"{"model":"gpt-4","messages":[{"role":"user","content":"hi"}]}"#.to_vec();
    let medium_body = serde_json::to_vec(&serde_json::json!({
        "model": "gpt-4",
        "messages": (0..20).map(|i| serde_json::json!({
            "role": if i % 2 == 0 { "user" } else { "assistant" },
            "content": format!("Message number {i} with some moderate content to simulate a real conversation turn.")
        })).collect::<Vec<_>>(),
        "stream": true,
        "temperature": 0.7
    }))
    .unwrap();
    let large_body = serde_json::to_vec(&serde_json::json!({
        "model": "gpt-4",
        "messages": [{
            "role": "user",
            "content": "x".repeat(10000)
        }]
    }))
    .unwrap();

    // Request headers through chain
    for n_plugins in [1, 3, 5] {
        group.bench_with_input(
            BenchmarkId::new("request_headers", format!("{n_plugins}_plugins")),
            &n_plugins,
            |b, &n| {
                let mut chain = ProxyPluginChain::new();
                for i in 0..n {
                    chain.add(Box::new(BenchCodecPlugin::new(&format!("plugin-{i}"))));
                }
                b.iter(|| {
                    let mut ctx = CodecProxyContext::new();
                    black_box(chain.process_request_headers(&mut ctx))
                });
            },
        );
    }

    // Request body transformation
    for (label, body) in [
        ("small/60B", &small_body),
        ("medium/2KB", &medium_body),
        ("large/10KB", &large_body),
    ] {
        group.bench_with_input(BenchmarkId::new("request_body", label), body, |b, body| {
            let mut chain = ProxyPluginChain::new();
            chain.add(Box::new(BenchCodecPlugin::new("converter")));
            let mut ctx = CodecProxyContext::new();
            b.iter(|| black_box(chain.process_request_body(&mut ctx, body.clone())));
        });
    }

    // Response body transformation
    for (label, body) in [
        ("small/60B", &small_body),
        ("medium/2KB", &medium_body),
        ("large/10KB", &large_body),
    ] {
        group.bench_with_input(BenchmarkId::new("response_body", label), body, |b, body| {
            let mut chain = ProxyPluginChain::new();
            chain.add(Box::new(BenchCodecPlugin::new("converter")));
            let mut ctx = CodecProxyContext::new();
            b.iter(|| black_box(chain.process_response_body(&mut ctx, body.clone())));
        });
    }

    // SSE stream chunk processing
    let sse_chunk = r#"data: {"id":"chatcmpl-abc","object":"chat.completion.chunk","choices":[{"delta":{"content":"Hello"},"index":0}]}"#;
    group.bench_function("stream_chunk/1_plugin", |b| {
        let mut chain = ProxyPluginChain::new();
        chain.add(Box::new(BenchCodecPlugin::new("converter")));
        let mut ctx = CodecProxyContext::new();
        b.iter(|| black_box(chain.process_response_stream_chunk(&mut ctx, sse_chunk)));
    });

    group.bench_function("stream_chunk/3_plugins", |b| {
        let mut chain = ProxyPluginChain::new();
        for i in 0..3 {
            chain.add(Box::new(BenchCodecPlugin::new(&format!("p{i}"))));
        }
        let mut ctx = CodecProxyContext::new();
        b.iter(|| black_box(chain.process_response_stream_chunk(&mut ctx, sse_chunk)));
    });

    // Registry lookup
    group.bench_function("registry_lookup/protocol_pair", |b| {
        let mut registry = CodecRegistry::new();
        for (inp, out) in [
            ("openai", "anthropic"),
            ("anthropic", "openai"),
            ("openai", "gemini"),
        ] {
            let mut chain = ProxyPluginChain::new();
            chain.add(Box::new(BenchCodecPlugin::new("conv")));
            registry.register_protocol_pair(inp, out, chain);
        }
        b.iter(|| black_box(registry.get_chain("some-provider", "openai", "anthropic")));
    });

    group.bench_function("registry_lookup/provider_specific", |b| {
        let mut registry = CodecRegistry::new();
        for i in 0..10 {
            let mut chain = ProxyPluginChain::new();
            chain.add(Box::new(BenchCodecPlugin::new("conv")));
            registry.register_provider(&format!("provider-{i}"), chain);
        }
        b.iter(|| black_box(registry.get_chain("provider-5", "openai", "anthropic")));
    });

    group.finish();
}

// ════════════════════════════════════════════════
// 3. Config Hot-Reload (arc-swap) Benchmarks
// ════════════════════════════════════════════════

fn bench_config_hotreload(c: &mut Criterion) {
    let mut group = c.benchmark_group("hotreload");

    // ArcSwap load (the hot path — every request does this)
    group.bench_function("arc_swap_load", |b| {
        let registry = build_registry(5, 5);
        let router = PrismRouter::new(registry);
        let state = AppState::new(router, std::path::PathBuf::from("test.yaml"));
        b.iter(|| {
            let guard = state.router();
            black_box(guard.registry.models.len())
        });
    });

    // ArcSwap store (the reload path — happens on config change)
    group.bench_function("arc_swap_store", |b| {
        let registry = build_registry(5, 5);
        let router = PrismRouter::new(registry);
        let state = AppState::new(router, std::path::PathBuf::from("test.yaml"));
        b.iter(|| {
            let new_registry = build_registry(5, 5);
            let new_router = PrismRouter::new(new_registry);
            state.swap_router(new_router);
            black_box(state.generation())
        });
    });

    // Load under contention: simulate concurrent reads while swapping
    group.bench_function("load_during_swap/5x5", |b| {
        let registry = build_registry(5, 5);
        let router = PrismRouter::new(registry);
        let state = AppState::new(router, std::path::PathBuf::from("test.yaml"));

        // Do a swap every N iterations to simulate background reload
        let mut counter = 0u64;
        b.iter(|| {
            counter += 1;
            if counter.is_multiple_of(100) {
                let new_registry = build_registry(5, 5);
                state.swap_router(PrismRouter::new(new_registry));
            }
            let guard = state.router();
            black_box(guard.registry.models.len())
        });
    });

    // Registry rebuild cost (the expensive part of reload)
    for (providers, models) in [(3, 3), (5, 5), (10, 10), (20, 20)] {
        let total = providers * models;
        group.bench_with_input(
            BenchmarkId::new(
                "registry_rebuild",
                format!("{providers}p×{models}m={total}v"),
            ),
            &(providers, models),
            |b, &(p, m)| {
                b.iter(|| {
                    let reg = build_registry(p, m);
                    black_box(PrismRouter::new(reg))
                });
            },
        );
    }

    group.finish();
}

// ════════════════════════════════════════════════
// 4. Metrics Collector Benchmarks
// ════════════════════════════════════════════════

fn bench_metrics(c: &mut Criterion) {
    let mut group = c.benchmark_group("metrics");

    // Basic record (per-variant window)
    group.bench_function("record/single_variant", |b| {
        let collector = MetricsCollector::new(1000);
        b.iter(|| {
            collector.record(RequestRecord {
                variant_id: "gpt-4o@openai".to_string(),
                latency_ms: 150,
                ttft_ms: Some(50),
                success: true,
                input_tokens: Some(100),
                output_tokens: Some(200),
                timestamp: Instant::now(),
            });
        });
    });

    // Record with full multi-dimensional labels
    group.bench_function("record_with_context/6_dims", |b| {
        let collector = MetricsCollector::new(1000);
        let labels = RequestLabels {
            model: "gpt-4o".to_string(),
            provider: "openai".to_string(),
            endpoint: "openai-ep".to_string(),
            account: "openai-acct".to_string(),
            region: "us-east-1".to_string(),
            variant: "gpt-4o@openai".to_string(),
        };
        b.iter(|| {
            collector.record_with_context(
                RequestRecord {
                    variant_id: "gpt-4o@openai".to_string(),
                    latency_ms: 150,
                    ttft_ms: Some(50),
                    success: true,
                    input_tokens: Some(100),
                    output_tokens: Some(200),
                    timestamp: Instant::now(),
                },
                &labels,
            );
        });
    });

    // Snapshot (read path)
    group.bench_function("snapshot/100_records", |b| {
        let collector = MetricsCollector::new(1000);
        for i in 0..100 {
            collector.record(RequestRecord {
                variant_id: "v1".to_string(),
                latency_ms: 100 + i,
                ttft_ms: Some(30 + i / 3),
                success: i % 10 != 0,
                input_tokens: Some(100),
                output_tokens: Some(200),
                timestamp: Instant::now(),
            });
        }
        b.iter(|| black_box(collector.snapshot("v1")));
    });

    // Prometheus format output
    group.bench_function("format_prometheus/5_variants", |b| {
        let collector = MetricsCollector::new(1000);
        for v in 0..5 {
            for i in 0..20 {
                collector.record(RequestRecord {
                    variant_id: format!("variant-{v}"),
                    latency_ms: 100 + i * 10,
                    ttft_ms: Some(30 + i),
                    success: true,
                    input_tokens: Some(100),
                    output_tokens: Some(200),
                    timestamp: Instant::now(),
                });
            }
        }
        b.iter(|| black_box(collector.format_prometheus(3600, 5, 25)));
    });

    // Routing decision recording
    group.bench_function("record_routing_decision", |b| {
        let collector = MetricsCollector::new(1000);
        b.iter(|| {
            collector.record_routing_decision(42, "auto_balanced");
        });
    });

    // Active requests inc/dec (gauge)
    group.bench_function("active_requests_inc_dec", |b| {
        let collector = MetricsCollector::new(1000);
        b.iter(|| {
            collector.inc_active_requests("gpt-4o@openai");
            collector.dec_active_requests("gpt-4o@openai");
        });
    });

    group.finish();
}

// ════════════════════════════════════════════════
// 5. End-to-End Routing at Scale (100 → 100k variants)
// ════════════════════════════════════════════════

fn simple_route_request() -> prism_core::router::RouteRequest {
    use prism_core::analyzer::ChatMessage;
    prism_core::router::RouteRequest {
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Hello, world!".to_string(),
        }],
        strategy: None,
        metadata: prism_core::router::RouteMetadata::default(),
        analyze_context: None,
        required_protocol: None,
    }
}

fn complex_route_request() -> prism_core::router::RouteRequest {
    use prism_core::analyzer::ChatMessage;
    prism_core::router::RouteRequest {
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You are an expert software architect.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Design a microservices architecture for a real-time trading platform \
                    that handles 10 million orders per day. Include event sourcing, CQRS, \
                    and discuss trade-offs. Provide code examples in Rust."
                    .to_string(),
            },
        ],
        strategy: None,
        metadata: prism_core::router::RouteMetadata::default(),
        analyze_context: None,
        required_protocol: None,
    }
}

/// Build a large registry with p providers × m models = p*m variants.
/// Tuned for benchmarking: uses sqrt decomposition for realistic provider/model ratios.
fn build_large_registry(total_variants: usize) -> InMemoryRegistry {
    // Find a reasonable p×m decomposition
    let p = (total_variants as f64).sqrt().ceil() as usize;
    let m = total_variants.div_ceil(p);
    build_registry(p, m)
}

fn bench_e2e_routing(c: &mut Criterion) {
    use prism_core::router::Router;

    let mut group = c.benchmark_group("e2e/routing");
    group.sample_size(50); // fewer samples for large registries

    for target_variants in [100, 1_000, 10_000, 100_000] {
        let registry = build_large_registry(target_variants);
        let actual = registry.variants.len();
        let router = PrismRouter::new(registry);
        let label = format!("{actual}_variants");

        group.bench_with_input(
            BenchmarkId::new("simple_msg", &label),
            &router,
            |b, router| {
                let req = simple_route_request();
                b.iter(|| black_box(router.route(&req).unwrap()));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("complex_msg", &label),
            &router,
            |b, router| {
                let req = complex_route_request();
                b.iter(|| black_box(router.route(&req).unwrap()));
            },
        );
    }

    group.finish();
}

// ════════════════════════════════════════════════
// 6. End-to-End Proxy Pipeline (routing + ratelimit + codec + metrics)
// ════════════════════════════════════════════════

fn bench_e2e_proxy_pipeline(c: &mut Criterion) {
    use prism_core::router::Router;

    let mut group = c.benchmark_group("e2e/proxy_pipeline");
    group.sample_size(50);

    for target_variants in [100, 1_000, 10_000, 100_000] {
        let registry = build_large_registry(target_variants);
        let actual = registry.variants.len();
        let router = PrismRouter::new(registry);

        // Rate limiter with realistic rules
        let limiter = RateLimiter::new(vec![
            RateLimitRule {
                name: "global".to_string(),
                key_headers: vec![],
                window_secs: 60,
                max_requests: Some(10_000_000),
                max_tokens: None,
                quotas: vec![],
                overrides: vec![],
                enabled: true,
                status_code: 429,
                message: "Rate limit exceeded".to_string(),
            },
            RateLimitRule {
                name: "per-model".to_string(),
                key_headers: vec!["x-model".to_string()],
                window_secs: 60,
                max_requests: Some(1_000_000),
                max_tokens: None,
                quotas: vec![],
                overrides: vec![],
                enabled: true,
                status_code: 429,
                message: "Rate limit exceeded".to_string(),
            },
        ]);

        // Codec chain (single plugin)
        let mut codec_chain = ProxyPluginChain::new();
        codec_chain.add(Box::new(BenchCodecPlugin::new("converter")));

        // Metrics collector
        let metrics = MetricsCollector::new(1000);

        // Request body for codec processing
        let request_body =
            br#"{"model":"gpt-4o","messages":[{"role":"user","content":"Hello"}],"stream":false}"#
                .to_vec();
        let response_body = br#"{"id":"chatcmpl-abc","object":"chat.completion","choices":[{"message":{"role":"assistant","content":"Hi there!"},"index":0}]}"#.to_vec();
        let rate_limit_headers = headers(&[("x-model", "gpt-4o")]);

        let label = format!("{actual}_variants");

        // Full pipeline: ratelimit check → route → codec request → codec response → metrics record
        group.bench_with_input(BenchmarkId::new("full_pipeline", &label), &(), |b, _| {
            let req = simple_route_request();
            b.iter(|| {
                // 1. Rate limit check
                let _ = black_box(limiter.check(&rate_limit_headers));

                // 2. Route
                let decision = router.route(&req).unwrap();

                // 3. Codec: transform request body
                let mut codec_ctx = CodecProxyContext::new();
                let _ = black_box(codec_chain.process_request_headers(&mut codec_ctx));
                let _ = black_box(
                    codec_chain.process_request_body(&mut codec_ctx, request_body.clone()),
                );

                // 4. Codec: transform response body
                let _ = black_box(codec_chain.process_response_headers(&mut codec_ctx));
                let _ = black_box(
                    codec_chain.process_response_body(&mut codec_ctx, response_body.clone()),
                );

                // 5. Record metrics
                metrics.record(RequestRecord {
                    variant_id: decision.selected_variant.clone(),
                    latency_ms: 150,
                    ttft_ms: Some(50),
                    success: true,
                    input_tokens: Some(100),
                    output_tokens: Some(200),
                    timestamp: Instant::now(),
                });

                black_box(decision)
            });
        });

        // Route-only (for comparison)
        group.bench_with_input(BenchmarkId::new("route_only", &label), &(), |b, _| {
            let req = simple_route_request();
            b.iter(|| black_box(router.route(&req).unwrap()));
        });
    }

    group.finish();
}

// ════════════════════════════════════════════════

criterion_group!(
    benches,
    bench_ratelimit,
    bench_codec_chain,
    bench_config_hotreload,
    bench_metrics,
    bench_e2e_routing,
    bench_e2e_proxy_pipeline,
);
criterion_main!(benches);
