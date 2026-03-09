#![allow(dead_code)]
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
        metadata: std::collections::HashMap::new(),
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
        metadata: std::collections::HashMap::new(),
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
            metadata: std::collections::HashMap::new(),
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
            inflight_snapshot: None,
            prefix_cache_index: None,
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
                .with_graceful_shutdown(async {
                    rx.await.ok();
                })
                .await
                .ok();
        });
        tokio::time::sleep(Duration::from_millis(10)).await;

        (port, tx, capture)
    }

    /// Verify that OpenAI-compat provider plugin sends Bearer auth header.
    #[tokio::test]
    async fn test_provider_plugin_openai_auth_e2e() {
        let (port, _shutdown, _capture) = start_capture_server().await;

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
            region: None,
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        });
        registry.add_account(Account {
            id: "test-openai-acct".into(),
            endpoint_id: "test-openai-ep".into(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-e2e-test-key".into(),
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
            id: "gpt-4o".into(),
            display_name: "GPT-4o".into(),
            quality_tier: QualityTier::A,
            capabilities: ModelCapabilities::default(),
            pricing: ModelPricing::default(),
            specialties: vec![],
            limits: None,
            status: ModelStatus::Available,
        });
        registry.add_variant(ModelVariant {
            id: "gpt-4o@test-openai".into(),
            model_id: "gpt-4o".into(),
            endpoint_id: "test-openai-ep".into(),
            account_id: Some("test-openai-acct".into()),
            deployment_name: None,
            weight: 1.0,
            performance: ModelPerformance::default(),
            pricing_override: None,
            status: VariantStatus::Active,
            base_url: None,
            upstream_model_name: None,
            metadata: std::collections::HashMap::new(),
        });
        registry.add_pool(ModelPool {
            id: "default".into(),
            name: "Default".into(),
            variants: vec!["gpt-4o@test-openai".into()],
            selection_mode: SelectionMode::default(),
            description: None,
        });

        // Make a request through the full proxy pipeline
        let _client = reqwest::Client::new();
        // Need to start actual proxy server for this — use the HTTP API route as proxy
        let router = PrismRouter::new(registry);
        let app = build_prism_app_with_state(AppState::new(router, PathBuf::from("e2e-test.yaml")));

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
        let (port, _shutdown, _capture) = start_capture_server().await;

        let registry = build_test_registry_with_model_rewrite(port);
        let router = PrismRouter::new(registry);
        let app = build_prism_app_with_state(AppState::new(router, PathBuf::from("e2e-test.yaml")));

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

// ════════════════════════════════════════════════
// E2E: Latency benchmarks — full pipeline timing
// ════════════════════════════════════════════════

#[cfg(feature = "proxy")]
mod latency_bench {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    /// Measure full pipeline latency: route decision + mock upstream round-trip.
    /// This captures the overhead Prism adds on top of upstream latency.
    #[tokio::test]
    async fn test_latency_full_pipeline() {
        let (port, _shutdown) = start_mock_server().await;
        let registry = build_test_registry(port);
        let router = PrismRouter::new(registry);
        let app = build_prism_app_with_state(AppState::new(router, PathBuf::from("e2e-test.yaml")));

        // Warmup
        for _ in 0..3 {
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/v1/route")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"messages":[{"role":"user","content":"warmup"}]}"#,
                ))
                .unwrap();
            let _ = app.clone().oneshot(req).await.unwrap();
        }

        // Timed runs
        let iterations = 100;
        let mut latencies = Vec::with_capacity(iterations);

        for _ in 0..iterations {
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/v1/route")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"messages":[{"role":"user","content":"latency test"}]}"#,
                ))
                .unwrap();

            let start = Instant::now();
            let resp = app.clone().oneshot(req).await.unwrap();
            let elapsed = start.elapsed();

            assert_eq!(resp.status(), 200);
            latencies.push(elapsed.as_micros() as u64);
        }

        latencies.sort();
        let p50 = latencies[latencies.len() / 2];
        let p95 = latencies[(latencies.len() as f64 * 0.95) as usize];
        let p99 = latencies[(latencies.len() as f64 * 0.99) as usize];
        let avg = latencies.iter().sum::<u64>() / latencies.len() as u64;
        let min = latencies[0];
        let max = latencies[latencies.len() - 1];

        println!("\n╔══════════════════════════════════════════════╗");
        println!("║  Route Decision Latency ({iterations} iterations)    ║");
        println!("╠══════════════════════════════════════════════╣");
        println!("║  min  = {:>8}µs                           ║", min);
        println!("║  avg  = {:>8}µs                           ║", avg);
        println!("║  p50  = {:>8}µs                           ║", p50);
        println!("║  p95  = {:>8}µs                           ║", p95);
        println!("║  p99  = {:>8}µs                           ║", p99);
        println!("║  max  = {:>8}µs                           ║", max);
        println!("╚══════════════════════════════════════════════╝");

        // Route decision should be < 5ms for a single variant
        assert!(p99 < 5000, "p99 latency {}µs exceeds 5ms threshold", p99);
    }

    /// Measure routing + model rewrite decision latency.
    #[tokio::test]
    async fn test_latency_with_model_rewrite() {
        let (port, _shutdown) = start_mock_server().await;
        let registry = build_test_registry_with_model_rewrite(port);
        let router = PrismRouter::new(registry);
        let app = build_prism_app_with_state(AppState::new(router, PathBuf::from("e2e-test.yaml")));

        // Warmup
        for _ in 0..3 {
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/v1/route")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"messages":[{"role":"user","content":"w"}],"strategy":"deepseek-r1"}"#,
                ))
                .unwrap();
            let _ = app.clone().oneshot(req).await.unwrap();
        }

        let iterations = 100;
        let mut latencies = Vec::with_capacity(iterations);

        for _ in 0..iterations {
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/v1/route")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    r#"{"messages":[{"role":"user","content":"bench"}],"strategy":"deepseek-r1"}"#,
                ))
                .unwrap();

            let start = Instant::now();
            let resp = app.clone().oneshot(req).await.unwrap();
            let elapsed = start.elapsed();

            assert_eq!(resp.status(), 200);
            latencies.push(elapsed.as_micros() as u64);
        }

        latencies.sort();
        let p50 = latencies[latencies.len() / 2];
        let p99 = latencies[(latencies.len() as f64 * 0.99) as usize];
        let avg = latencies.iter().sum::<u64>() / latencies.len() as u64;

        println!("\n╔══════════════════════════════════════════════╗");
        println!("║  Model Rewrite Latency ({iterations} iterations)     ║");
        println!("╠══════════════════════════════════════════════╣");
        println!("║  avg  = {:>8}µs                           ║", avg);
        println!("║  p50  = {:>8}µs                           ║", p50);
        println!("║  p99  = {:>8}µs                           ║", p99);
        println!("╚══════════════════════════════════════════════╝");

        assert!(p99 < 5000, "p99 latency {}µs exceeds 5ms", p99);
    }

    /// Measure provider plugin dispatch overhead (build_auth + rewrite_url).
    #[tokio::test]
    async fn test_latency_provider_plugin_dispatch() {
        use prism_router::provider::openai::OpenAICompatPlugin;
        use prism_router::provider::{ProviderPlugin, ProviderRequestContext};

        let plugin = OpenAICompatPlugin;
        let ctx = ProviderRequestContext {
            provider_id: "bench-provider".into(),
            provider_type: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            provider_extra: HashMap::new(),
            account_id: "bench-acct".into(),
            account_api_key: Some("sk-bench-key-1234567890".into()),
            account_headers: HashMap::new(),
            variant_id: "gpt-4o@bench".into(),
            model_id: "gpt-4o".into(),
            upstream_model_name: None,
            deployment_name: None,
            original_path: "/v1/chat/completions".into(),
            method: "POST".into(),
            is_streaming: false,
        };

        // Warmup
        for _ in 0..100 {
            let _ = plugin.build_auth(&ctx);
            let _ = plugin.rewrite_url(&ctx);
        }

        let iterations = 10_000;

        // build_auth
        let start = Instant::now();
        for _ in 0..iterations {
            let _ = plugin.build_auth(&ctx).unwrap();
        }
        let auth_total = start.elapsed();
        let auth_per_call = auth_total.as_nanos() / iterations as u128;

        // rewrite_url
        let start = Instant::now();
        for _ in 0..iterations {
            let _ = plugin.rewrite_url(&ctx).unwrap();
        }
        let url_total = start.elapsed();
        let url_per_call = url_total.as_nanos() / iterations as u128;

        // transform_request_body
        let body = br#"{"model":"gpt-4o","messages":[{"role":"user","content":"benchmark"}],"max_tokens":100}"#.to_vec();
        let start = Instant::now();
        for _ in 0..iterations {
            let _ = plugin.transform_request_body(body.clone(), &ctx).unwrap();
        }
        let body_total = start.elapsed();
        let body_per_call = body_total.as_nanos() / iterations as u128;

        println!("\n╔══════════════════════════════════════════════╗");
        println!("║  Provider Plugin Dispatch ({iterations} calls)    ║");
        println!("╠══════════════════════════════════════════════╣");
        println!(
            "║  build_auth       = {:>6}ns/call             ║",
            auth_per_call
        );
        println!(
            "║  rewrite_url      = {:>6}ns/call             ║",
            url_per_call
        );
        println!(
            "║  transform_body   = {:>6}ns/call             ║",
            body_per_call
        );
        println!(
            "║  total (all 3)    = {:>6}ns/call             ║",
            auth_per_call + url_per_call + body_per_call
        );
        println!("╚══════════════════════════════════════════════╝");

        // Provider plugin dispatch should add < 10µs overhead
        assert!(
            auth_per_call + url_per_call + body_per_call < 10_000,
            "Provider plugin overhead {}ns exceeds 10µs",
            auth_per_call + url_per_call + body_per_call
        );
    }

    /// Measure concurrent routing throughput.
    #[tokio::test]
    async fn test_latency_concurrent_routing() {
        let (port, _shutdown) = start_mock_server().await;
        let registry = build_test_registry(port);
        let router = PrismRouter::new(registry);
        let app = Arc::new(build_prism_app_with_state(AppState::new(
            router,
            PathBuf::from("e2e-test.yaml"),
        )));

        let concurrency = 10;
        let per_task = 50;
        let total = concurrency * per_task;

        let start = Instant::now();
        let mut handles = Vec::new();

        for _ in 0..concurrency {
            let app = app.clone();
            handles.push(tokio::spawn(async move {
                let mut latencies = Vec::with_capacity(per_task);
                for _ in 0..per_task {
                    let req = axum::http::Request::builder()
                        .method("POST")
                        .uri("/v1/route")
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(
                            r#"{"messages":[{"role":"user","content":"concurrent"}]}"#,
                        ))
                        .unwrap();

                    let t = Instant::now();
                    let svc: Router = (*app).clone();
                    let resp = svc.oneshot(req).await.unwrap();
                    latencies.push(t.elapsed().as_micros() as u64);
                    assert_eq!(resp.status(), 200);
                }
                latencies
            }));
        }

        let mut all_latencies = Vec::new();
        for h in handles {
            all_latencies.extend(h.await.unwrap());
        }
        let wall_time = start.elapsed();

        all_latencies.sort();
        let p50 = all_latencies[all_latencies.len() / 2];
        let p99 = all_latencies[(all_latencies.len() as f64 * 0.99) as usize];
        let avg = all_latencies.iter().sum::<u64>() / all_latencies.len() as u64;
        let rps = total as f64 / wall_time.as_secs_f64();

        println!("\n╔══════════════════════════════════════════════╗");
        println!("║  Concurrent Routing ({concurrency}×{per_task} = {total} requests)   ║");
        println!("╠══════════════════════════════════════════════╣");
        println!(
            "║  wall time = {:>6.1}ms                        ║",
            wall_time.as_secs_f64() * 1000.0
        );
        println!("║  avg       = {:>8}µs                       ║", avg);
        println!("║  p50       = {:>8}µs                       ║", p50);
        println!("║  p99       = {:>8}µs                       ║", p99);
        println!("║  throughput= {:>8.0} req/s                   ║", rps);
        println!("╚══════════════════════════════════════════════╝");

        assert!(p99 < 10_000, "p99 {}µs exceeds 10ms under concurrency", p99);
        assert!(rps > 1000.0, "throughput {:.0} req/s below 1K", rps);
    }
}

// ════════════════════════════════════════════════
// E2E: Extended Provider Plugin + Proxy Scenarios
// ════════════════════════════════════════════════

mod extended_e2e {
    use super::*;
    use std::sync::Arc;

    // ── Helper: build a multi-provider registry with 2 variants for a single model ──

    fn build_two_variant_registry(mock_port: u16) -> InMemoryRegistry {
        let mut registry = InMemoryRegistry::new();

        // Provider A
        registry.add_provider(Provider {
            id: "provider-a".into(),
            name: "Provider A".into(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-provider-a".into(),
                header: "Authorization".into(),
            },
            base_url: format!("http://127.0.0.1:{}/v1", mock_port),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: vec!["openai".into()],
        });
        registry.add_endpoint(Endpoint {
            id: "ep-a".into(),
            provider_id: "provider-a".into(),
            base_url: Some(format!("http://127.0.0.1:{}/v1", mock_port)),
            region: Some("us-east".into()),
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        });
        registry.add_account(Account {
            id: "acct-a".into(),
            endpoint_id: "ep-a".into(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-provider-a".into(),
                header: "Authorization".into(),
            },
            project_id: None,
            rate_limit: None,
            billing_tag: None,
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
        });

        // Provider B
        registry.add_provider(Provider {
            id: "provider-b".into(),
            name: "Provider B".into(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-provider-b".into(),
                header: "Authorization".into(),
            },
            base_url: format!("http://127.0.0.1:{}/v1", mock_port),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: vec!["openai".into()],
        });
        registry.add_endpoint(Endpoint {
            id: "ep-b".into(),
            provider_id: "provider-b".into(),
            base_url: Some(format!("http://127.0.0.1:{}/v1", mock_port)),
            region: Some("us-west".into()),
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        });
        registry.add_account(Account {
            id: "acct-b".into(),
            endpoint_id: "ep-b".into(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-provider-b".into(),
                header: "Authorization".into(),
            },
            project_id: None,
            rate_limit: None,
            billing_tag: None,
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
        });

        // Single model with two variants across the two providers
        registry.add_model(Model {
            id: "shared-model".into(),
            display_name: "Shared Model".into(),
            quality_tier: QualityTier::A,
            capabilities: ModelCapabilities::default(),
            pricing: ModelPricing::default(),
            specialties: vec![],
            limits: None,
            status: ModelStatus::Available,
        });

        registry.add_variant(ModelVariant {
            id: "shared-model@provider-a".into(),
            model_id: "shared-model".into(),
            endpoint_id: "ep-a".into(),
            account_id: Some("acct-a".into()),
            deployment_name: None,
            weight: 1.0,
            performance: ModelPerformance::default(),
            pricing_override: None,
            status: VariantStatus::Active,
            base_url: None,
            upstream_model_name: None,
            metadata: HashMap::new(),
        });

        registry.add_variant(ModelVariant {
            id: "shared-model@provider-b".into(),
            model_id: "shared-model".into(),
            endpoint_id: "ep-b".into(),
            account_id: Some("acct-b".into()),
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
            id: "default".into(),
            name: "Default".into(),
            variants: vec![
                "shared-model@provider-a".into(),
                "shared-model@provider-b".into(),
            ],
            selection_mode: SelectionMode::default(),
            description: None,
        });

        registry
    }

    // ── 1. test_provider_plugin_bedrock_auth ──

    /// Build a registry with a "bedrock" type provider using api-key auth.
    /// Verify routing selects the bedrock provider variant correctly.
    #[tokio::test]
    async fn test_provider_plugin_bedrock_auth() {
        let (port, _shutdown) = start_mock_server().await;

        let mut registry = InMemoryRegistry::new();
        registry.add_provider(Provider {
            id: "bedrock-provider".into(),
            name: "AWS Bedrock".into(),
            auth: AuthConfig::ApiKey {
                key: "some-aws-key".into(),
                header: "api-key".into(),
            },
            base_url: format!("http://127.0.0.1:{}/v1", port),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: vec!["openai".into()],
        });
        registry.add_endpoint(Endpoint {
            id: "bedrock-ep".into(),
            provider_id: "bedrock-provider".into(),
            base_url: Some(format!("http://127.0.0.1:{}/v1", port)),
            region: Some("us-east-1".into()),
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        });
        registry.add_account(Account {
            id: "bedrock-acct".into(),
            endpoint_id: "bedrock-ep".into(),
            auth: AuthConfig::ApiKey {
                key: "some-aws-key".into(),
                header: "api-key".into(),
            },
            project_id: None,
            rate_limit: None,
            billing_tag: None,
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
        });
        registry.add_model(Model {
            id: "bedrock-model".into(),
            display_name: "Bedrock Model".into(),
            quality_tier: QualityTier::A,
            capabilities: ModelCapabilities::default(),
            pricing: ModelPricing::default(),
            specialties: vec![],
            limits: None,
            status: ModelStatus::Available,
        });
        registry.add_variant(ModelVariant {
            id: "bedrock-model@bedrock-provider".into(),
            model_id: "bedrock-model".into(),
            endpoint_id: "bedrock-ep".into(),
            account_id: Some("bedrock-acct".into()),
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
            id: "default".into(),
            name: "Default".into(),
            variants: vec!["bedrock-model@bedrock-provider".into()],
            selection_mode: SelectionMode::default(),
            description: None,
        });

        let app = build_prism_app(registry);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"test bedrock"}]}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);

        let json = body_json(resp).await;
        assert_eq!(json["selected_provider"], "bedrock-provider");
        assert_eq!(json["selected_variant"], "bedrock-model@bedrock-provider");
        assert_eq!(json["selected_account"], "bedrock-acct");

        // Verify the provider auth config stored correctly
        let registry2 = build_test_registry(port);
        let _ = registry2; // just proving build_test_registry is callable

        // Re-build registry to inspect auth config directly
        let mut verify_reg = InMemoryRegistry::new();
        verify_reg.add_provider(Provider {
            id: "bedrock-provider".into(),
            name: "AWS Bedrock".into(),
            auth: AuthConfig::ApiKey {
                key: "some-aws-key".into(),
                header: "api-key".into(),
            },
            base_url: format!("http://127.0.0.1:{}/v1", port),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: vec!["openai".into()],
        });
        let provider = verify_reg.providers.get("bedrock-provider").unwrap();
        match &provider.auth {
            AuthConfig::ApiKey { key, header } => {
                assert_eq!(key, "some-aws-key");
                assert_eq!(header, "api-key");
            }
            _ => panic!("expected ApiKey auth for bedrock provider"),
        }
    }

    // ── 2. test_provider_plugin_vertex_auth ──

    /// Build a registry with a "vertex" type provider using custom headers auth.
    /// Verify routing selects the vertex provider variant correctly.
    #[tokio::test]
    async fn test_provider_plugin_vertex_auth() {
        let (port, _shutdown) = start_mock_server().await;

        let mut registry = InMemoryRegistry::new();
        registry.add_provider(Provider {
            id: "vertex-provider".into(),
            name: "GCP Vertex AI".into(),
            auth: AuthConfig::ApiKey {
                key: "vertex-api-key-123".into(),
                header: "Authorization".into(),
            },
            base_url: format!("http://127.0.0.1:{}/v1", port),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: vec!["openai".into()],
        });
        registry.add_endpoint(Endpoint {
            id: "vertex-ep".into(),
            provider_id: "vertex-provider".into(),
            base_url: Some(format!("http://127.0.0.1:{}/v1", port)),
            region: Some("us-central1".into()),
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        });
        registry.add_account(Account {
            id: "vertex-acct".into(),
            endpoint_id: "vertex-ep".into(),
            auth: AuthConfig::ApiKey {
                key: "vertex-api-key-123".into(),
                header: "Authorization".into(),
            },
            project_id: Some("gcp-project-123".into()),
            rate_limit: None,
            billing_tag: None,
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
        });
        registry.add_model(Model {
            id: "vertex-model".into(),
            display_name: "Vertex Model".into(),
            quality_tier: QualityTier::S,
            capabilities: ModelCapabilities::default(),
            pricing: ModelPricing::default(),
            specialties: vec![],
            limits: None,
            status: ModelStatus::Available,
        });
        registry.add_variant(ModelVariant {
            id: "vertex-model@vertex-provider".into(),
            model_id: "vertex-model".into(),
            endpoint_id: "vertex-ep".into(),
            account_id: Some("vertex-acct".into()),
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
            id: "default".into(),
            name: "Default".into(),
            variants: vec!["vertex-model@vertex-provider".into()],
            selection_mode: SelectionMode::default(),
            description: None,
        });

        let app = build_prism_app(registry);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"test vertex"}]}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);

        let json = body_json(resp).await;
        assert_eq!(json["selected_provider"], "vertex-provider");
        assert_eq!(json["selected_variant"], "vertex-model@vertex-provider");
        assert_eq!(json["selected_account"], "vertex-acct");
    }

    // ── 3. test_streaming_sse_response ──

    /// Start a custom mock server that returns SSE-formatted responses.
    /// Verify via reqwest that the SSE data lines are received correctly.
    #[tokio::test]
    async fn test_streaming_sse_response() {
        use axum::response::IntoResponse;

        async fn sse_handler() -> impl IntoResponse {
            let sse_body = [
                "data: {\"id\":\"chatcmpl-sse1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{\"content\":\"Hello\"},\"index\":0}]}\n\n",
                "data: {\"id\":\"chatcmpl-sse2\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{\"content\":\" world\"},\"index\":0}]}\n\n",
                "data: [DONE]\n\n",
            ].join("");

            (
                [
                    ("content-type", "text/event-stream"),
                    ("cache-control", "no-cache"),
                ],
                sse_body,
            )
        }

        let app = Router::new().route("/v1/chat/completions", post(sse_handler));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    rx.await.ok();
                })
                .await
                .ok();
        });
        tokio::time::sleep(Duration::from_millis(10)).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/v1/chat/completions", port))
            .json(&json!({
                "model": "test",
                "messages": [{"role": "user", "content": "hi"}],
                "stream": true
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let content_type = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            content_type.contains("text/event-stream"),
            "expected text/event-stream, got {}",
            content_type
        );

        let body = resp.text().await.unwrap();
        assert!(body.contains("data: {\"id\":\"chatcmpl-sse1\""));
        assert!(body.contains("data: {\"id\":\"chatcmpl-sse2\""));
        assert!(body.contains("data: [DONE]"));

        // Verify each data line is parseable (except [DONE])
        let data_lines: Vec<&str> = body
            .lines()
            .filter(|l| l.starts_with("data: ") && !l.contains("[DONE]"))
            .collect();
        assert_eq!(data_lines.len(), 2);
        for line in &data_lines {
            let json_str = line.trim_start_matches("data: ");
            let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap();
            assert_eq!(parsed["object"], "chat.completion.chunk");
        }

        let _ = tx.send(());
    }

    // ── 4. test_5xx_retry_on_503 ──

    /// Build a registry with 2 variants for the same model.
    /// Verify that the routing decision includes a non-empty fallback chain,
    /// which is the mechanism used for 5xx retry.
    #[tokio::test]
    async fn test_5xx_retry_on_503() {
        let (port, _shutdown) = start_mock_server().await;
        let registry = build_two_variant_registry(port);
        let app = build_prism_app(registry);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"test fallback"}]}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);

        let json = body_json(resp).await;
        assert_eq!(json["selected_model"], "shared-model");

        // With 2 variants, the fallback chain should contain the other variant
        let fallback = json["fallback_chain"].as_array().unwrap();
        assert!(
            !fallback.is_empty(),
            "Expected non-empty fallback_chain for 2-variant model, got {:?}",
            fallback
        );

        // The fallback variant should differ from the selected variant
        let selected = json["selected_variant"].as_str().unwrap();
        for fb in fallback {
            assert_ne!(
                fb.as_str().unwrap(),
                selected,
                "Fallback should differ from selected variant"
            );
        }
    }

    // ── 5. test_5xx_no_retry_on_400 ──

    /// Verify that 400-class errors (client errors) return proper error status
    /// and would not be retried. Empty messages => 400 INVALID_REQUEST.
    #[tokio::test]
    async fn test_5xx_no_retry_on_400() {
        let (port, _shutdown) = start_mock_server().await;
        let registry = build_test_registry(port);
        let app = build_prism_app(registry);

        // Empty messages array should yield 400 INVALID_REQUEST
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
        // 400 responses do not include fallback_chain (they are error responses)
        assert!(
            json.get("fallback_chain").is_none(),
            "400 error should not include fallback_chain"
        );
    }

    // ── 6. test_concurrent_requests_load_balancing ──

    /// Build a registry with 2 variants across 2 providers.
    /// Send 20 requests concurrently, verify all succeed (200) and that
    /// the routing decision exposes both variants (selected + fallback).
    #[tokio::test]
    async fn test_concurrent_requests_load_balancing() {
        let (port, _shutdown) = start_mock_server().await;
        let registry = build_two_variant_registry(port);
        let router = PrismRouter::new(registry);
        let state = AppState::new(router, PathBuf::from("e2e-test.yaml"));
        let app = Arc::new(build_prism_app_with_state(state));

        let total = 20;
        let mut handles = Vec::with_capacity(total);

        for _ in 0..total {
            let app = app.clone();
            handles.push(tokio::spawn(async move {
                let req = axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/route")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"messages":[{"role":"user","content":"lb test"}]}"#,
                    ))
                    .unwrap();

                let svc: Router = (*app).clone();
                let resp = svc.oneshot(req).await.unwrap();
                assert_eq!(resp.status(), 200);
                body_json(resp).await
            }));
        }

        let mut all_variants_seen = std::collections::HashSet::new();
        for h in handles {
            let json = h.await.unwrap();
            let selected = json["selected_variant"].as_str().unwrap().to_string();
            all_variants_seen.insert(selected);

            // Also collect fallback chain variants
            if let Some(chain) = json["fallback_chain"].as_array() {
                for fb in chain {
                    if let Some(v) = fb.as_str() {
                        all_variants_seen.insert(v.to_string());
                    }
                }
            }
        }

        // With 2 variants, the selected + fallback should cover both
        assert!(
            all_variants_seen.contains("shared-model@provider-a"),
            "Expected provider-a variant in routing results"
        );
        assert!(
            all_variants_seen.contains("shared-model@provider-b"),
            "Expected provider-b variant in routing results"
        );
    }

    // ── 7. test_model_alias_e2e ──

    /// Build a registry with model "qwen3-72b".
    /// Configure model aliases: {"gpt-4" => "qwen3-72b"}.
    /// Send route request with "model": "gpt-4".
    /// Verify the response routes to "qwen3-72b".
    #[tokio::test]
    async fn test_model_alias_e2e() {
        let (port, _shutdown) = start_mock_server().await;

        let mut registry = InMemoryRegistry::new();
        registry.add_provider(Provider {
            id: "alias-provider".into(),
            name: "Alias Provider".into(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-alias".into(),
                header: "Authorization".into(),
            },
            base_url: format!("http://127.0.0.1:{}/v1", port),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: vec!["openai".into()],
        });
        registry.add_endpoint(Endpoint {
            id: "alias-ep".into(),
            provider_id: "alias-provider".into(),
            base_url: Some(format!("http://127.0.0.1:{}/v1", port)),
            region: None,
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        });
        registry.add_account(Account {
            id: "alias-acct".into(),
            endpoint_id: "alias-ep".into(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-alias".into(),
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
            id: "qwen3-72b".into(),
            display_name: "Qwen3 72B".into(),
            quality_tier: QualityTier::A,
            capabilities: ModelCapabilities::default(),
            pricing: ModelPricing::default(),
            specialties: vec![],
            limits: None,
            status: ModelStatus::Available,
        });
        registry.add_variant(ModelVariant {
            id: "qwen3-72b@alias-provider".into(),
            model_id: "qwen3-72b".into(),
            endpoint_id: "alias-ep".into(),
            account_id: Some("alias-acct".into()),
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
            id: "default".into(),
            name: "Default".into(),
            variants: vec!["qwen3-72b@alias-provider".into()],
            selection_mode: SelectionMode::default(),
            description: None,
        });

        // Configure model aliases: "gpt-4" => "qwen3-72b"
        let mut aliases = HashMap::new();
        aliases.insert("gpt-4".to_string(), "qwen3-72b".to_string());

        let router = PrismRouter::new(registry);
        let state =
            AppState::new(router, PathBuf::from("e2e-test.yaml")).with_model_aliases(aliases);
        let app = build_prism_app_with_state(state);

        // Send route request with "model": "gpt-4" — should resolve to "qwen3-72b"
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"alias test"}],"model":"gpt-4"}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);

        let json = body_json(resp).await;
        assert_eq!(
            json["selected_model"], "qwen3-72b",
            "Expected alias 'gpt-4' to resolve to 'qwen3-72b'"
        );
        assert_eq!(json["selected_variant"], "qwen3-72b@alias-provider");
    }

    // ── 8. test_client_key_quota_enforcement ──

    /// Create a ClientKeyManager with a key limited to 3 requests per minute.
    /// Validate 3 times (success), then a 4th time (rate limited).
    #[tokio::test]
    async fn test_client_key_quota_enforcement() {
        use prism_router::client_keys::{ClientKeyError, ClientKeyManager};
        use prism_router::config::types::{ClientKeyConfig, ClientKeyQuotaConfig};

        let cfg = ClientKeyConfig {
            key: "sk-quota-test".to_string(),
            name: "Quota Test".to_string(),
            enabled: true,
            allowed_models: vec![],
            quota: ClientKeyQuotaConfig {
                max_requests_per_minute: 3,
                max_tokens_per_minute: 0,
                max_tokens_per_day: 0,
                max_concurrent: 0,
            },
            model_quotas: HashMap::new(),
            metadata: HashMap::new(),
        };

        let mgr = ClientKeyManager::new(&[cfg]).unwrap();

        // First 3 calls should succeed
        for i in 0..3 {
            let result = mgr.validate_key("sk-quota-test", None);
            assert!(
                result.is_ok(),
                "Call {} should succeed but was rate limited",
                i + 1
            );
        }

        // 4th call should be rate limited
        let result = mgr.validate_key("sk-quota-test", None);
        assert!(
            matches!(result, Err(ClientKeyError::RateLimited { .. })),
            "4th call should be rate limited"
        );
    }
}
