#![allow(dead_code)]
//! E2E integration tests for gRPC RouterService.
//!
//! Each test starts a real tonic gRPC server on a random port and exercises
//! it through a real `RouterServiceClient` over TCP.
//!
//! Run: `cargo test -p prism-router --features grpc --test e2e_grpc`

use std::collections::HashMap;
use std::path::PathBuf;

use prism_core::registry::InMemoryRegistry;
use prism_core::router::PrismRouter;
use prism_core::types::*;

use prism_router::gen::prism_router_v1 as pb;
use prism_router::gen::prism_router_v1::router_service_client::RouterServiceClient;
use prism_router::gen::prism_router_v1::router_service_server::RouterServiceServer;
use prism_router::grpc::GrpcRouterService;
use prism_router::state::AppState;

// ════════════════════════════════════════════════════════════════════
// Helpers
// ════════════════════════════════════════════════════════════════════

/// Build a minimal in-memory registry with one complete provider chain.
fn build_test_registry() -> InMemoryRegistry {
    let mut registry = InMemoryRegistry::new();

    registry.add_provider(Provider {
        id: "openai".to_string(),
        name: "OpenAI".to_string(),
        auth: AuthConfig::ApiKey {
            key: "Bearer sk-test".to_string(),
            header: "Authorization".to_string(),
        },
        base_url: "https://api.openai.com/v1".to_string(),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: HashMap::new(),
        protocols: vec!["openai".to_string()],
    });

    registry.add_endpoint(Endpoint {
        id: "openai-ep".to_string(),
        provider_id: "openai".to_string(),
        base_url: Some("https://api.openai.com/v1".to_string()),
        region: None,
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    });

    registry.add_account(Account {
        id: "openai-acct".to_string(),
        endpoint_id: "openai-ep".to_string(),
        auth: AuthConfig::ApiKey {
            key: "Bearer sk-test".to_string(),
            header: "Authorization".to_string(),
        },
        project_id: None,
        rate_limit: None,
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: HashMap::new(),
    });

    registry.add_model(Model {
        id: "gpt-4o".to_string(),
        display_name: "GPT-4o".to_string(),
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
            ..Default::default()
        },
        specialties: vec![],
        limits: None,
        status: ModelStatus::Available,
    });

    registry.add_variant(ModelVariant {
        id: "gpt-4o@openai".to_string(),
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
        metadata: HashMap::new(),
    });

    registry.add_pool(ModelPool {
        id: "default".to_string(),
        name: "Default".to_string(),
        variants: vec!["gpt-4o@openai".to_string()],
        selection_mode: SelectionMode::default(),
        description: None,
    });

    registry
}

/// Build AppState from the test registry with default configuration.
fn build_test_state() -> AppState {
    let registry = build_test_registry();
    let router = PrismRouter::new(registry);
    AppState::new(router, PathBuf::from("test.yaml"))
}

/// Start a real tonic gRPC server bound to a random port.
/// Returns the allocated port and a oneshot sender to trigger graceful shutdown.
async fn start_grpc_server(state: AppState) -> (u16, tokio::sync::oneshot::Sender<()>) {
    let svc = GrpcRouterService::new(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(RouterServiceServer::new(svc))
            .serve_with_incoming_shutdown(incoming, async {
                rx.await.ok();
            })
            .await
            .ok();
    });

    // Give the server a moment to accept connections.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (port, tx)
}

/// Build a simple RouteRequest with a single user message.
fn simple_route_request() -> pb::RouteRequest {
    pb::RouteRequest {
        messages: vec![pb::ChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        }],
        strategy: None,
        model: None,
        pool_ids: vec![],
        exclude_models: vec![],
        exclude_providers: vec![],
        affinity_key: None,
        tenant_id: None,
        capabilities: None,
        strategy_params: None,
        metadata: HashMap::new(),
    }
}

// ════════════════════════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════════════════════════

/// Basic end-to-end route: send one message, expect gpt-4o routed through openai.
#[tokio::test]
async fn test_grpc_e2e_route_basic() {
    let state = build_test_state();
    let (port, _shutdown) = start_grpc_server(state).await;

    let mut client = RouterServiceClient::connect(format!("http://127.0.0.1:{}", port))
        .await
        .unwrap();

    let resp = client.route(simple_route_request()).await.unwrap();
    let body = resp.into_inner();

    assert_eq!(body.model, "gpt-4o");
    assert_eq!(body.provider, "openai");
    assert_eq!(body.variant, "gpt-4o@openai");
    assert_eq!(body.account, "openai-acct");
    assert!(body.upstream.is_some());

    let upstream = body.upstream.unwrap();
    assert_eq!(upstream.base_url, "https://api.openai.com/v1");
    assert_eq!(upstream.model_name, "gpt-4o");
    assert_eq!(upstream.variant_id, "gpt-4o@openai");
    assert_eq!(upstream.path, "/v1/chat/completions");
    assert!(upstream.headers.contains_key("Authorization"));
}

/// Model alias resolution: "gpt-4" should resolve to "gpt-4o" through aliases.
#[tokio::test]
async fn test_grpc_e2e_route_model_alias() {
    let state = build_test_state();
    let mut aliases = HashMap::new();
    aliases.insert("gpt-4".to_string(), "gpt-4o".to_string());
    let state = state.with_model_aliases(aliases);

    let (port, _shutdown) = start_grpc_server(state).await;

    let mut client = RouterServiceClient::connect(format!("http://127.0.0.1:{}", port))
        .await
        .unwrap();

    let mut req = simple_route_request();
    req.model = Some("gpt-4".to_string());

    let resp = client.route(req).await.unwrap();
    let body = resp.into_inner();

    assert_eq!(body.model, "gpt-4o");
    assert_eq!(body.provider, "openai");
    assert_eq!(body.variant, "gpt-4o@openai");
}

/// Requesting a model that does not exist should return NotFound.
#[tokio::test]
async fn test_grpc_e2e_route_unknown_model() {
    let state = build_test_state();
    let (port, _shutdown) = start_grpc_server(state).await;

    let mut client = RouterServiceClient::connect(format!("http://127.0.0.1:{}", port))
        .await
        .unwrap();

    let mut req = simple_route_request();
    req.model = Some("nonexistent-model-xyz".to_string());

    let err = client.route(req).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
}

/// Health check should report correct counts for the test registry.
#[tokio::test]
async fn test_grpc_e2e_health_check() {
    let state = build_test_state();
    let (port, _shutdown) = start_grpc_server(state).await;

    let mut client = RouterServiceClient::connect(format!("http://127.0.0.1:{}", port))
        .await
        .unwrap();

    let resp = client
        .health_check(pb::HealthCheckRequest {})
        .await
        .unwrap();
    let body = resp.into_inner();

    assert_eq!(body.status, "healthy");
    assert_eq!(body.registered_models, 1);
    assert_eq!(body.active_variants, 1);
    assert!(body.uptime_seconds >= 0);
}

/// Fire 10 concurrent requests and verify all succeed with the expected model.
#[tokio::test]
async fn test_grpc_e2e_concurrent_requests() {
    let state = build_test_state();
    let (port, _shutdown) = start_grpc_server(state).await;

    let mut handles = Vec::with_capacity(10);
    for _ in 0..10 {
        let addr = format!("http://127.0.0.1:{}", port);
        handles.push(tokio::spawn(async move {
            let mut client = RouterServiceClient::connect(addr).await.unwrap();
            client.route(simple_route_request()).await
        }));
    }

    for handle in handles {
        let resp = handle.await.unwrap().unwrap();
        let body = resp.into_inner();
        assert_eq!(body.model, "gpt-4o");
        assert_eq!(body.provider, "openai");
        assert_eq!(body.variant, "gpt-4o@openai");
    }
}

/// Batch routing: send 3 requests in one RPC, verify all 3 responses.
#[tokio::test]
async fn test_grpc_e2e_route_batch() {
    let state = build_test_state();
    let (port, _shutdown) = start_grpc_server(state).await;

    let mut client = RouterServiceClient::connect(format!("http://127.0.0.1:{}", port))
        .await
        .unwrap();

    let batch = pb::RouteBatchRequest {
        requests: vec![
            simple_route_request(),
            simple_route_request(),
            simple_route_request(),
        ],
    };

    let resp = client.route_batch(batch).await.unwrap();
    let body = resp.into_inner();

    assert_eq!(body.responses.len(), 3);
    for r in &body.responses {
        assert_eq!(r.model, "gpt-4o");
        assert_eq!(r.provider, "openai");
        assert_eq!(r.variant, "gpt-4o@openai");
    }
}

/// Report an outcome and verify the RPC returns Ok.
#[tokio::test]
async fn test_grpc_e2e_report_outcome() {
    let state = build_test_state();
    let (port, _shutdown) = start_grpc_server(state).await;

    let mut client = RouterServiceClient::connect(format!("http://127.0.0.1:{}", port))
        .await
        .unwrap();

    let report = pb::OutcomeReport {
        request_id: "req-e2e-1".to_string(),
        variant_id: "gpt-4o@openai".to_string(),
        status_code: 200,
        latency_ms: 150,
        ttft_ms: 50,
        input_tokens: 100,
        output_tokens: 200,
        error: None,
        used_fallback: false,
        fallback_variant_id: None,
    };

    let resp = client.report_outcome(report).await;
    assert!(resp.is_ok());
}

/// Sending an empty messages vec should return InvalidArgument.
#[tokio::test]
async fn test_grpc_e2e_empty_messages_error() {
    let state = build_test_state();
    let (port, _shutdown) = start_grpc_server(state).await;

    let mut client = RouterServiceClient::connect(format!("http://127.0.0.1:{}", port))
        .await
        .unwrap();

    let req = pb::RouteRequest {
        messages: vec![],
        strategy: None,
        model: None,
        pool_ids: vec![],
        exclude_models: vec![],
        exclude_providers: vec![],
        affinity_key: None,
        tenant_id: None,
        capabilities: None,
        strategy_params: None,
        metadata: HashMap::new(),
    };

    let err = client.route(req).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}
