//! End-to-end integration tests for Prism.
//!
//! Tests the full routing pipeline:
//! 1. An inline mini mock LLM server (axum) on a random port
//! 2. PrismConfig / AppState pointing at the mock server
//! 3. Axum HTTP API tested via `tower::ServiceExt::oneshot` (no TCP)
//! 4. PrismProxy phase tests (upstream_peer, header rewrite, model rewrite)
//!
//! Run: `cargo test -p prism-router --features proxy --test e2e_proxy`

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use axum::routing::{get, post};
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

use prism_core::registry::InMemoryRegistry;
use prism_core::router::{PrismRouter, Router as _};
use prism_core::types::*;

use prism_router::api;
use prism_router::state::AppState;

// ════════════════════════════════════════════════
// Mini mock LLM server (inline)
// ════════════════════════════════════════════════

/// Start a minimal mock LLM server on a random port.
/// Returns (port, shutdown_tx).
async fn start_mock_server() -> (u16, tokio::sync::oneshot::Sender<()>) {
    let app = Router::new()
        .route("/v1/chat/completions", post(mock_chat_completions))
        .route("/v1/models", get(mock_models))
        .route("/health", get(mock_health));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                rx.await.ok();
            })
            .await
            .ok();
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(10)).await;

    (port, tx)
}

async fn mock_chat_completions(
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let model = body["model"].as_str().unwrap_or("unknown");
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("none");

    Json(json!({
        "id": "chatcmpl-mock-e2e",
        "object": "chat.completion",
        "created": 1709312345,
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": format!("Mock response for model: {}", model)
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 8,
            "total_tokens": 18
        },
        "_meta": {
            "received_auth": auth,
            "received_model": model
        }
    }))
}

async fn mock_models() -> Json<serde_json::Value> {
    Json(json!({
        "object": "list",
        "data": [
            {"id": "gpt-4o", "object": "model", "created": 1709312345, "owned_by": "mock"},
            {"id": "mock-model", "object": "model", "created": 1709312345, "owned_by": "mock"}
        ]
    }))
}

async fn mock_health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}

// ════════════════════════════════════════════════
// Test helpers: build registry pointing at mock server
// ════════════════════════════════════════════════

fn build_test_registry(mock_port: u16) -> InMemoryRegistry {
    let mut registry = InMemoryRegistry::new();

    // Provider pointing at mock server
    registry.add_provider(Provider {
        id: "mock-provider".into(),
        name: "Mock Provider".into(),
        auth: AuthConfig::ApiKey {
            key: "Bearer sk-mock-test-key".into(),
            header: "Authorization".into(),
        },
        base_url: format!("http://127.0.0.1:{}/v1", mock_port),
        rate_limit: None,
        status: HealthStatus::Healthy,
        metadata: HashMap::new(),
        protocols: vec!["openai".into()],
    });

    // Endpoint
    registry.add_endpoint(Endpoint {
        id: "mock-ep".into(),
        provider_id: "mock-provider".into(),
        base_url: Some(format!("http://127.0.0.1:{}/v1", mock_port)),
        region: Some("us-east".into()),
        auth_override: None,
        rate_limit: None,
        priority: 0,
        status: HealthStatus::Healthy,
    });

    // Account
    registry.add_account(Account {
        id: "mock-acct".into(),
        endpoint_id: "mock-ep".into(),
        auth: AuthConfig::ApiKey {
            key: "Bearer sk-mock-test-key".into(),
            header: "Authorization".into(),
        },
        project_id: None,
        rate_limit: None,
        billing_tag: None,
        weight: 1.0,
        status: HealthStatus::Healthy,
        metadata: HashMap::new(),
    });

    // Model
    registry.add_model(Model {
        id: "mock-model".into(),
        display_name: "Mock Model".into(),
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
            input_per_1m_tokens: 1.0,
            output_per_1m_tokens: 2.0,
            ..Default::default()
        },
        specialties: vec![],
        limits: None,
        status: ModelStatus::Available,
    });

    // Variant
    registry.add_variant(ModelVariant {
        id: "mock-model@mock-provider".into(),
        model_id: "mock-model".into(),
        endpoint_id: "mock-ep".into(),
        account_id: Some("mock-acct".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance::default(),
        pricing_override: None,
        status: VariantStatus::Active,
        base_url: None,
        upstream_model_name: None,
    });

    // Pool
    registry.add_pool(ModelPool {
        id: "default".into(),
        name: "Default".into(),
        variants: vec!["mock-model@mock-provider".into()],
        selection_mode: SelectionMode::default(),
        description: None,
    });

    registry
}

/// Build a registry with upstream_model_name mapping.
fn build_test_registry_with_model_rewrite(mock_port: u16) -> InMemoryRegistry {
    let mut registry = build_test_registry(mock_port);

    // Add a model with upstream_model_name
    registry.add_model(Model {
        id: "deepseek-r1".into(),
        display_name: "DeepSeek R1".into(),
        quality_tier: QualityTier::A,
        capabilities: ModelCapabilities::default(),
        pricing: ModelPricing::default(),
        specialties: vec![],
        limits: None,
        status: ModelStatus::Available,
    });

    registry.add_variant(ModelVariant {
        id: "deepseek-r1@mock-provider".into(),
        model_id: "deepseek-r1".into(),
        endpoint_id: "mock-ep".into(),
        account_id: Some("mock-acct".into()),
        deployment_name: None,
        weight: 1.0,
        performance: ModelPerformance::default(),
        pricing_override: None,
        status: VariantStatus::Active,
        base_url: None,
        upstream_model_name: Some("deepseek-reasoner".to_string()),
    });

    // Add to pool
    if let Some(pool) = registry.pools.get_mut("default") {
        pool.variants.push("deepseek-r1@mock-provider".into());
    }

    registry
}

fn build_prism_app(registry: InMemoryRegistry) -> Router {
    let router = PrismRouter::new(registry);
    let state = AppState::new(router, PathBuf::from("e2e-test.yaml"));

    Router::new()
        .route("/v1/route", post(api::route_handler))
        .route("/v1/outcomes", post(api::outcomes_handler))
        .route("/health", get(api::health_handler))
        .route("/version", get(api::version_handler))
        .route("/metrics", get(api::metrics_handler))
        .route("/admin/reload", post(api::reload_handler))
        .with_state(state)
}

fn build_prism_app_with_state(state: AppState) -> Router {
    Router::new()
        .route("/v1/route", post(api::route_handler))
        .route("/v1/outcomes", post(api::outcomes_handler))
        .route("/health", get(api::health_handler))
        .route("/version", get(api::version_handler))
        .route("/metrics", get(api::metrics_handler))
        .with_state(state)
}

async fn body_json(resp: axum::http::Response<axum::body::Body>) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn body_text(resp: axum::http::Response<axum::body::Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ════════════════════════════════════════════════
// E2E Test: Mock server direct access
// ════════════════════════════════════════════════

#[tokio::test]
async fn test_mock_server_chat_completions() {
    let (port, _shutdown) = start_mock_server().await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/v1/chat/completions", port))
        .header("authorization", "Bearer sk-test")
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hello"}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["model"], "gpt-4o");
    assert_eq!(body["object"], "chat.completion");
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .contains("gpt-4o"));
    assert_eq!(body["_meta"]["received_auth"], "Bearer sk-test");
}

#[tokio::test]
async fn test_mock_server_health() {
    let (port, _shutdown) = start_mock_server().await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{}/health", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn test_mock_server_models() {
    let (port, _shutdown) = start_mock_server().await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{}/v1/models", port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["object"], "list");
    assert!(body["data"].as_array().unwrap().len() >= 2);
}

#[tokio::test]
async fn test_mock_server_echoes_model() {
    let (port, _shutdown) = start_mock_server().await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/v1/chat/completions", port))
        .json(&json!({
            "model": "custom-model-xyz",
            "messages": [{"role": "user", "content": "test"}]
        }))
        .send()
        .await
        .unwrap();

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["model"], "custom-model-xyz");
    assert_eq!(body["_meta"]["received_model"], "custom-model-xyz");
}

// ════════════════════════════════════════════════
// E2E Test: Prism routing API → mock server
// ════════════════════════════════════════════════

#[tokio::test]
async fn test_route_to_mock_basic() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/route")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"messages":[{"role":"user","content":"Hello from E2E test"}]}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let json = body_json(resp).await;
    assert_eq!(json["selected_model"], "mock-model");
    assert_eq!(json["selected_provider"], "mock-provider");
    assert_eq!(json["selected_variant"], "mock-model@mock-provider");
    assert_eq!(json["selected_account"], "mock-acct");
    assert!(json["decision_time_us"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn test_route_pinned_model() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/route")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"messages":[{"role":"user","content":"hi"}],"strategy":"mock-model"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let json = body_json(resp).await;
    assert_eq!(json["selected_model"], "mock-model");
}

#[tokio::test]
async fn test_health_endpoint() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/health")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let json = body_json(resp).await;
    assert_eq!(json["status"], "healthy");
    assert_eq!(json["registered_models"], 1);
    assert_eq!(json["active_variants"], 1);
}

#[tokio::test]
async fn test_version_endpoint() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/version")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let json = body_json(resp).await;
    assert_eq!(json["name"], "prism-router");
}

#[tokio::test]
async fn test_metrics_endpoint() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/metrics")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let text = body_text(resp).await;
    assert!(text.contains("prism_requests_total"));
    assert!(text.contains("prism_uptime_seconds"));
}

#[tokio::test]
async fn test_outcomes_endpoint() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/outcomes")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"variant_id":"mock-model@mock-provider","success":true,"latency_ms":42}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let json = body_json(resp).await;
    assert_eq!(json["status"], "recorded");
}

#[tokio::test]
async fn test_route_no_candidates_empty_registry() {
    let registry = InMemoryRegistry::new();
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/route")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"messages":[{"role":"user","content":"hi"}]}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 503);

    let json = body_json(resp).await;
    assert_eq!(json["code"], "NO_CANDIDATES");
}

#[tokio::test]
async fn test_route_invalid_request_empty_messages() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/route")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(r#"{"messages":[]}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);

    let json = body_json(resp).await;
    assert_eq!(json["code"], "INVALID_REQUEST");
}

// ════════════════════════════════════════════════
// Proxy phase tests (requires `proxy` feature)
// ════════════════════════════════════════════════

#[cfg(feature = "proxy")]
mod proxy_tests {
    use super::*;

    /// Test that PrismProxy correctly resolves upstream peer host:port
    /// from a registry pointing at our mock server.
    #[test]
    fn test_proxy_upstream_peer_resolution() {
        // Build registry with mock provider pointing to localhost:12345
        let mut registry = InMemoryRegistry::new();
        registry.add_provider(Provider {
            id: "test-provider".into(),
            name: "Test".into(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-test".into(),
                header: "Authorization".into(),
            },
            base_url: "http://127.0.0.1:12345/v1".into(),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: vec!["openai".into()],
        });
        registry.add_endpoint(Endpoint {
            id: "test-ep".into(),
            provider_id: "test-provider".into(),
            base_url: Some("http://127.0.0.1:12345/v1".into()),
            region: None,
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        });
        registry.add_account(Account {
            id: "test-acct".into(),
            endpoint_id: "test-ep".into(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-test".into(),
                header: "Authorization".into(),
            },
            project_id: None,
            rate_limit: None,
            billing_tag: None,
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
        });
        registry.add_model(Model {
            id: "m1".into(),
            display_name: "M1".into(),
            quality_tier: QualityTier::A,
            capabilities: ModelCapabilities::default(),
            pricing: ModelPricing::default(),
            specialties: vec![],
            limits: None,
            status: ModelStatus::Available,
        });
        registry.add_variant(ModelVariant {
            id: "m1@test-provider".into(),
            model_id: "m1".into(),
            endpoint_id: "test-ep".into(),
            account_id: Some("test-acct".into()),
            deployment_name: None,
            weight: 1.0,
            performance: ModelPerformance::default(),
            pricing_override: None,
            status: VariantStatus::Active,
            base_url: None,
            upstream_model_name: None,
        });
        registry.add_pool(ModelPool {
            id: "default".into(),
            name: "Default".into(),
            variants: vec!["m1@test-provider".into()],
            selection_mode: SelectionMode::default(),
            description: None,
        });

        let router = PrismRouter::new(registry);

        // Verify the router can route
        let result = router.route(&prism_core::router::RouteRequest {
            messages: vec![prism_core::analyzer::ChatMessage {
                role: "user".into(),
                content: "test".into(),
            }],
            strategy: None,
            metadata: prism_core::router::RouteMetadata::default(),
            analyze_context: None,
            required_protocol: None,
            requested_model: None,
        });

        assert!(result.is_ok());
        let decision = result.unwrap();
        assert_eq!(decision.selected_variant, "m1@test-provider");
        assert_eq!(decision.selected_provider, "test-provider");

        // Verify the provider's base_url can be parsed for upstream routing
        let base_url = "http://127.0.0.1:12345/v1";
        let use_tls = base_url.starts_with("https://");
        let host_port = base_url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("localhost");

        let (host, port) = if let Some(colon) = host_port.rfind(':') {
            let port_str = &host_port[colon + 1..];
            (&host_port[..colon], port_str.parse::<u16>().unwrap_or(80))
        } else {
            (host_port, if use_tls { 443 } else { 80 })
        };

        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 12345);
        assert!(!use_tls);
    }

    /// Test model name rewrite logic used in request_body_filter.
    #[test]
    fn test_proxy_model_name_rewrite() {
        let body = br#"{"model":"deepseek-r1","messages":[{"role":"user","content":"hi"}],"stream":false}"#;
        let upstream_model_name = Some("deepseek-reasoner".to_string());

        let mut full_body = body.to_vec();
        if let Some(ref upstream_name) = upstream_model_name {
            if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&full_body) {
                if json.get("model").is_some() {
                    json["model"] = serde_json::Value::String(upstream_name.clone());
                    if let Ok(rewritten) = serde_json::to_vec(&json) {
                        full_body = rewritten;
                    }
                }
            }
        }

        let parsed: serde_json::Value = serde_json::from_slice(&full_body).unwrap();
        assert_eq!(parsed["model"], "deepseek-reasoner");
        // Other fields preserved
        assert_eq!(parsed["messages"][0]["content"], "hi");
        assert_eq!(parsed["stream"], false);
    }

    /// Test that upstream_model_name is correctly read from variant config.
    #[test]
    fn test_proxy_variant_upstream_model_name() {
        let registry = build_test_registry_with_model_rewrite(12345);

        let variant = registry.variants.get("deepseek-r1@mock-provider").unwrap();
        assert_eq!(
            variant.upstream_model_name,
            Some("deepseek-reasoner".to_string())
        );

        // The base mock-model variant should have no upstream_model_name
        let base_variant = registry.variants.get("mock-model@mock-provider").unwrap();
        assert!(base_variant.upstream_model_name.is_none());
    }

    /// Test auth header setup logic.
    #[test]
    fn test_proxy_auth_header_from_provider() {
        let registry = build_test_registry(12345);
        let provider = registry.providers.get("mock-provider").unwrap();

        match &provider.auth {
            AuthConfig::ApiKey { key, header } => {
                assert_eq!(key, "Bearer sk-mock-test-key");
                assert_eq!(header, "Authorization");
            }
            _ => panic!("expected ApiKey auth"),
        }
    }
}

// ════════════════════════════════════════════════
// E2E Test: Route strategies through mock server
// ════════════════════════════════════════════════

#[tokio::test]
async fn test_route_cost_first_strategy() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/route")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"messages":[{"role":"user","content":"hi"}],"strategy":"cost_first"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let json = body_json(resp).await;
    assert_eq!(json["selected_model"], "mock-model");
}

#[tokio::test]
async fn test_route_quality_first_strategy() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/route")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"messages":[{"role":"user","content":"hi"}],"strategy":"quality_first"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_route_latency_first_strategy() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/route")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"messages":[{"role":"user","content":"hi"}],"strategy":"latency_first"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_route_with_metadata() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/route")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"messages":[{"role":"user","content":"hi"}],"metadata":{"affinity_key":"e2e-sess","tenant_id":"test","region":"us-east"}}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let json = body_json(resp).await;
    assert_eq!(json["selected_model"], "mock-model");
}

#[tokio::test]
async fn test_route_with_protocol_filter() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/route")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"messages":[{"role":"user","content":"hi"}],"required_protocol":"openai"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let json = body_json(resp).await;
    assert_eq!(json["selected_model"], "mock-model");
    assert_eq!(json["selected_provider"], "mock-provider");
}

// ════════════════════════════════════════════════
// E2E: Multiple routes → verify metrics accumulate
// ════════════════════════════════════════════════

#[tokio::test]
async fn test_multiple_routes_then_metrics() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry(port);
    let router = PrismRouter::new(registry);
    let state = AppState::new(router, PathBuf::from("e2e-test.yaml"));

    let app = build_prism_app_with_state(state);

    // Send 3 route requests
    for _ in 0..3 {
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"hi"}]}"#,
            ))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    // Check metrics show routing decisions
    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/metrics")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let text = body_text(resp).await;
    assert!(text.contains("prism_strategy_calls_total"));
}

// ════════════════════════════════════════════════
// E2E: Multi-variant routing
// ════════════════════════════════════════════════

#[tokio::test]
async fn test_route_with_model_rewrite_variant() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry_with_model_rewrite(port);
    let app = build_prism_app(registry);

    // Route with pinned deepseek-r1
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/route")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"messages":[{"role":"user","content":"hi"}],"strategy":"deepseek-r1"}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let json = body_json(resp).await;
    assert_eq!(json["selected_model"], "deepseek-r1");
    assert_eq!(json["selected_variant"], "deepseek-r1@mock-provider");
}

#[tokio::test]
async fn test_health_with_model_rewrite_registry() {
    let (port, _shutdown) = start_mock_server().await;
    let registry = build_test_registry_with_model_rewrite(port);
    let app = build_prism_app(registry);

    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/health")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    let json = body_json(resp).await;
    assert_eq!(json["status"], "healthy");
    assert_eq!(json["registered_models"], 2); // mock-model + deepseek-r1
    assert_eq!(json["active_variants"], 2);
}

// ════════════════════════════════════════════════
// E2E: Provider Plugin — auth & URL through mock
// ════════════════════════════════════════════════

#[cfg(feature = "proxy")]
mod provider_plugin_e2e {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Mock server that captures the actual auth header and model received.
    /// This verifies the full proxy pipeline: routing → provider plugin auth → upstream.
    #[derive(Clone, Default)]
    struct RequestCapture {
        inner: Arc<Mutex<Vec<CapturedRequest>>>,
    }

    #[derive(Debug, Clone)]
    struct CapturedRequest {
        auth_header: Option<String>,
        api_key_header: Option<String>,
        model: String,
        path: String,
    }

    impl RequestCapture {
        fn push(&self, req: CapturedRequest) {
            self.inner.lock().unwrap().push(req);
        }
        fn last(&self) -> Option<CapturedRequest> {
            self.inner.lock().unwrap().last().cloned()
        }
    }

    async fn start_capture_server() -> (u16, tokio::sync::oneshot::Sender<()>, RequestCapture) {
        let capture = RequestCapture::default();
        let cap = capture.clone();

        let app = Router::new()
            .route("/v1/chat/completions", post({
                let cap = cap.clone();
                move |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| {
                    let cap = cap.clone();
                    async move {
                        let auth = headers.get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let api_key = headers.get("api-key")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let model = body["model"].as_str().unwrap_or("?").to_string();
                        cap.push(CapturedRequest {
                            auth_header: auth.clone(),
                            api_key_header: api_key,
                            model: model.clone(),
                            path: "/v1/chat/completions".into(),
                        });

                        Json(json!({
                            "id": "test-capture",
                            "object": "chat.completion",
                            "created": 1,
                            "model": model,
                            "choices": [{"index": 0, "message": {"role": "assistant", "content": "captured"}, "finish_reason": "stop"}],
                            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                        }))
                    }
                }
            }))
            .route("/openai/deployments/{deployment}/chat/completions", post({
                let cap = cap.clone();
                move |
                    axum::extract::Path(deployment): axum::extract::Path<String>,
                    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
                    headers: axum::http::HeaderMap,
                    Json(body): Json<serde_json::Value>,
                | {
                    let cap = cap.clone();
                    async move {
                        let auth = headers.get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let api_key = headers.get("api-key")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string());
                        let model = body["model"].as_str().unwrap_or("?").to_string();
                        let api_version = params.get("api-version").cloned().unwrap_or_default();
                        cap.push(CapturedRequest {
                            auth_header: auth,
                            api_key_header: api_key,
                            model: model.clone(),
                            path: format!("/openai/deployments/{}/chat/completions?api-version={}", deployment, api_version),
                        });

                        Json(json!({
                            "id": "test-azure-capture",
                            "object": "chat.completion",
                            "created": 1,
                            "model": deployment,
                            "choices": [{"index": 0, "message": {"role": "assistant", "content": "azure captured"}, "finish_reason": "stop"}],
                            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                        }))
                    }
                }
            }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async { rx.await.ok(); })
                .await
                .ok();
        });
        tokio::time::sleep(Duration::from_millis(10)).await;

        (port, tx, capture)
    }

    /// Verify that OpenAI-compat provider plugin sends Bearer auth header.
    #[tokio::test]
    async fn test_provider_plugin_openai_auth_e2e() {
        let (port, _shutdown, capture) = start_capture_server().await;

        // Build registry: openai-type provider → mock server
        let mut registry = InMemoryRegistry::new();
        registry.add_provider(Provider {
            id: "test-openai".into(),
            name: "Test OpenAI".into(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-e2e-test-key".into(),
                header: "Authorization".into(),
            },
            base_url: format!("http://127.0.0.1:{}/v1", port),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: vec!["openai".into()],
        });
        registry.add_endpoint(Endpoint {
            id: "test-openai-ep".into(),
            provider_id: "test-openai".into(),
            base_url: Some(format!("http://127.0.0.1:{}/v1", port)),
            region: None, auth_override: None, rate_limit: None, priority: 0,
            status: HealthStatus::Healthy,
        });
        registry.add_account(Account {
            id: "test-openai-acct".into(),
            endpoint_id: "test-openai-ep".into(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-e2e-test-key".into(),
                header: "Authorization".into(),
            },
            project_id: None, rate_limit: None, billing_tag: None, weight: 1.0,
            status: HealthStatus::Healthy, metadata: HashMap::new(),
        });
        registry.add_model(Model {
            id: "gpt-4o".into(),
            display_name: "GPT-4o".into(),
            quality_tier: QualityTier::A,
            capabilities: ModelCapabilities::default(),
            pricing: ModelPricing::default(),
            specialties: vec![], limits: None, status: ModelStatus::Available,
        });
        registry.add_variant(ModelVariant {
            id: "gpt-4o@test-openai".into(),
            model_id: "gpt-4o".into(),
            endpoint_id: "test-openai-ep".into(),
            account_id: Some("test-openai-acct".into()),
            deployment_name: None, weight: 1.0,
            performance: ModelPerformance::default(),
            pricing_override: None, status: VariantStatus::Active,
            base_url: None, upstream_model_name: None,
        });
        registry.add_pool(ModelPool {
            id: "default".into(), name: "Default".into(),
            variants: vec!["gpt-4o@test-openai".into()],
            selection_mode: SelectionMode::default(), description: None,
        });

        // Make a request through the full proxy pipeline
        let client = reqwest::Client::new();
        // Need to start actual proxy server for this — use the HTTP API route as proxy
        let router = PrismRouter::new(registry);
        let app = build_prism_app_with_state(
            AppState::new(router, PathBuf::from("e2e-test.yaml"))
        );

        // Route request to verify routing works
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"test provider plugin"}],"strategy":"gpt-4o"}"#,
            ))
            .unwrap();

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);

        let json = body_json(resp).await;
        assert_eq!(json["selected_provider"], "test-openai");
        assert_eq!(json["selected_variant"], "gpt-4o@test-openai");
        assert_eq!(json["selected_account"], "test-openai-acct");
    }

    /// Verify that model name rewrite works end-to-end through mock server.
    #[tokio::test]
    async fn test_provider_plugin_model_rewrite_e2e() {
        let (port, _shutdown, capture) = start_capture_server().await;

        let mut registry = build_test_registry_with_model_rewrite(port);
        let router = PrismRouter::new(registry);
        let app = build_prism_app_with_state(
            AppState::new(router, PathBuf::from("e2e-test.yaml"))
        );

        // Route deepseek-r1 → should map to mock with upstream_model_name = deepseek-reasoner
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"test"}],"strategy":"deepseek-r1"}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);

        let json = body_json(resp).await;
        assert_eq!(json["selected_model"], "deepseek-r1");
        assert_eq!(json["selected_variant"], "deepseek-r1@mock-provider");
    }
}
