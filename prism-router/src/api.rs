use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use std::time::Instant;

use prism_core::analyzer::ChatMessage;
use prism_core::feedback::OutcomeReport;
use prism_core::registry::ModelRegistryTrait;
use prism_core::router::{RouteMetadata, RouteRequest, Router, RouterError};
use prism_core::strategy::ModelSelectionStrategy;

use crate::async_task::TaskStatus;
use crate::config::types::AdminConfig;
use crate::metrics::{RequestLabels, RequestRecord};
use crate::state::AppState;

// ──────────────────────────────────────────────
// Request / Response types
// ──────────────────────────────────────────────

/// Route request body.
#[derive(Debug, Deserialize)]
pub struct RouteRequestBody {
    pub messages: Vec<MessageBody>,
    pub strategy: Option<String>,
    pub metadata: Option<RouteMetadataBody>,
    pub required_protocol: Option<String>,
    pub model: Option<String>,
}

/// Simplified message for the API.
#[derive(Debug, Deserialize)]
pub struct MessageBody {
    pub role: String,
    pub content: String,
}

/// Optional metadata in the route request.
#[derive(Debug, Deserialize)]
pub struct RouteMetadataBody {
    pub affinity_key: Option<String>,
    pub tenant_id: Option<String>,
    pub region: Option<String>,
}

/// Route response body.
#[derive(Debug, Serialize)]
pub struct RouteResponseBody {
    pub selected_variant: String,
    pub selected_model: String,
    pub selected_provider: String,
    pub selected_account: String,
    pub fallback_chain: Vec<String>,
    pub score: f32,
    pub decision_time_us: u64,
    pub reason: serde_json::Value,
}

/// Health check response.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub registered_models: usize,
    pub active_variants: usize,
    pub uptime_seconds: u64,
}

/// Version response.
#[derive(Debug, Serialize)]
pub struct VersionResponse {
    pub name: String,
    pub version: String,
}

/// Error response body.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}

// ──────────────────────────────────────────────
// Handlers
// ──────────────────────────────────────────────

/// POST /v1/route — routing decision
pub async fn route_handler(
    State(state): State<AppState>,
    Json(body): Json<RouteRequestBody>,
) -> impl IntoResponse {
    if body.messages.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::to_value(ErrorResponse {
                    error: "messages must not be empty".to_string(),
                    code: "INVALID_REQUEST".to_string(),
                })
                .unwrap(),
            ),
        );
    }

    let messages: Vec<ChatMessage> = body
        .messages
        .iter()
        .map(|m| ChatMessage {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();

    let metadata = match &body.metadata {
        Some(m) => RouteMetadata {
            affinity_key: m.affinity_key.clone(),
            tenant_id: m.tenant_id.clone(),
            region: m.region.clone(),
            ..Default::default()
        },
        None => RouteMetadata::default(),
    };

    // Parse strategy from string to enum
    let strategy = parse_strategy(body.strategy.as_deref());

    // Determine strategy name for metrics
    let strategy_name = match body.strategy.as_deref() {
        Some("cost_first") => "cost_first",
        Some("quality_first") => "quality_first",
        Some("latency_first") => "latency_first",
        Some("balanced") | None => "balanced",
        Some(_) => "pinned",
    };

    // Resolve model alias if model is provided
    let requested_model = body.model.map(|m| state.resolve_model_alias(&m));

    let request = RouteRequest {
        messages,
        strategy,
        metadata,
        analyze_context: None,
        requested_model,
        inflight_snapshot: Some(state.inflight.snapshot()),
        prefix_cache_index: Some(state.prefix_cache.clone()),
        required_protocol: body.required_protocol,
        required_capability: None,
        content_hint: None,
    };

    let route_start = Instant::now();
    let router = state.router();
    let result = router.route(&request);
    let decision_time_us = route_start.elapsed().as_micros() as u64;

    match result {
        Ok(decision) => {
            // Record routing decision metrics
            state
                .metrics
                .record_routing_decision(decision_time_us, strategy_name);

            // Record traffic distribution
            state.metrics.routes_by_model.inc(&decision.selected_model);
            state
                .metrics
                .routes_by_provider
                .inc(&decision.selected_provider);
            state
                .metrics
                .routes_by_account
                .inc(&decision.selected_account);

            // Record fallback if fallback_chain is non-empty
            if !decision.fallback_chain.is_empty() {
                state.metrics.record_fallback();
            }

            let response = RouteResponseBody {
                selected_variant: decision.selected_variant,
                selected_model: decision.selected_model,
                selected_provider: decision.selected_provider,
                selected_account: decision.selected_account,
                fallback_chain: decision.fallback_chain,
                score: decision.score,
                decision_time_us: decision.decision_time_us,
                reason: serde_json::to_value(&decision.reason).unwrap_or_default(),
            };
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
        }
        Err(e) => {
            let (status, code) = match &e {
                RouterError::NoCandidates => (StatusCode::SERVICE_UNAVAILABLE, "NO_CANDIDATES"),
                RouterError::AllFiltered { .. } => {
                    (StatusCode::SERVICE_UNAVAILABLE, "ALL_FILTERED")
                }
                RouterError::InternalError { .. } => {
                    (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")
                }
            };
            (
                status,
                Json(
                    serde_json::to_value(ErrorResponse {
                        error: e.to_string(),
                        code: code.to_string(),
                    })
                    .unwrap(),
                ),
            )
        }
    }
}

/// POST /v1/outcomes — feedback reporting
pub async fn outcomes_handler(
    State(state): State<AppState>,
    Json(report): Json<OutcomeReport>,
) -> impl IntoResponse {
    // Record into metrics collector for DataLayer feedback loop
    let record = RequestRecord {
        variant_id: report.variant_id.clone(),
        latency_ms: report.latency_ms,
        ttft_ms: None,
        success: report.success,
        input_tokens: report.prompt_tokens,
        output_tokens: report.completion_tokens,
        timestamp: Instant::now(),
    };

    // Extract provider/model/account from variant_id if possible (format: model@provider)
    // Fall back to variant_id itself for each dimension
    let variant_id = &report.variant_id;
    let (model, provider) = if let Some(at_pos) = variant_id.find('@') {
        (&variant_id[..at_pos], &variant_id[at_pos + 1..])
    } else {
        (variant_id.as_str(), variant_id.as_str())
    };

    // Resolve endpoint and region from registry
    let router = state.router();
    let (endpoint_id, region) = {
        let registry = &router.registry;
        if let Some(variant) = registry.variants.get(variant_id.as_str()) {
            let ep_id = variant.endpoint_id.clone();
            let reg = registry
                .endpoints
                .get(&ep_id)
                .and_then(|ep| ep.region.clone())
                .unwrap_or_else(|| "unknown".to_string());
            (ep_id, reg)
        } else {
            ("unknown".to_string(), "unknown".to_string())
        }
    };

    let labels = RequestLabels {
        model: model.to_string(),
        provider: provider.to_string(),
        endpoint: endpoint_id,
        account: variant_id.to_string(),
        region,
        variant: variant_id.to_string(),
    };

    state.metrics.record_with_context(record, &labels);

    router.record_outcome(&report);
    (
        StatusCode::OK,
        Json(serde_json::json!({"status": "recorded"})),
    )
}

/// GET /health — health check
pub async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let router = state.router();
    let candidates = router.registry.get_all_candidates();
    let model_count = router.registry.models.len();
    let variant_count = candidates.len();
    let uptime = state.start_time.elapsed().as_secs();

    let response = HealthResponse {
        status: "healthy".to_string(),
        registered_models: model_count,
        active_variants: variant_count,
        uptime_seconds: uptime,
    };

    (StatusCode::OK, Json(response))
}

/// GET /readyz — readiness probe (Kubernetes-style)
///
/// Returns 200 when the router has at least one variant registered and config loaded.
/// Returns 503 when not ready.
pub async fn readyz_handler(State(state): State<AppState>) -> impl IntoResponse {
    let router = state.router();
    let variant_count = router.registry.get_all_candidates().len();
    let model_count = router.registry.models.len();

    if variant_count > 0 && model_count > 0 {
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "ready",
                "models": model_count,
                "variants": variant_count,
            })),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "reason": "no models or variants registered",
                "models": model_count,
                "variants": variant_count,
            })),
        )
    }
}

/// GET /version — version info
pub async fn version_handler() -> impl IntoResponse {
    let response = VersionResponse {
        name: "prism-router".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    (StatusCode::OK, Json(response))
}

/// GET /metrics — Prometheus metrics
pub async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs();
    let router = state.router();
    let model_count = router.registry.models.len();
    let variant_count = router.registry.get_all_candidates().len();

    let output = state
        .metrics
        .format_prometheus(uptime, model_count, variant_count);

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        output,
    )
}

/// POST /admin/reload — trigger manual config reload
pub async fn reload_handler(State(state): State<AppState>) -> impl IntoResponse {
    use crate::config_watcher::reload_config;

    match reload_config(&state, &state.config_path) {
        Ok(result) => {
            state.metrics.record_config_reload();
            let json = serde_json::json!({
                "status": "ok",
                "generation": result.generation,
                "providers": result.providers_count,
                "models": result.models_count,
                "variants": result.variants_count,
                "reload_time_ms": result.duration.as_millis() as u64,
            });
            (StatusCode::OK, Json(json))
        }
        Err(e) => {
            let json = serde_json::json!({
                "status": "error",
                "message": e,
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json))
        }
    }
}

/// POST /admin/config/validate — dry-run config validation
///
/// Accepts YAML body, parses it, validates structure, returns errors if any.
/// Does NOT apply the config.
pub async fn validate_config_handler(body: String) -> impl IntoResponse {
    use crate::config::load_config_from_str;

    match load_config_from_str(&body) {
        Ok(config) => {
            let providers_count = config.providers.len();
            let models_count = config.models.len();
            let variants_count = config.variants.len();
            let rate_limits_count = config.rate_limits.len();

            // Validate cross-references
            let mut warnings = Vec::new();

            // Check that variants reference valid providers and models
            for variant in &config.variants {
                if !config.providers.contains_key(&variant.provider_id) {
                    warnings.push(format!(
                        "variant '{}' references unknown provider '{}'",
                        variant.id, variant.provider_id
                    ));
                }
                if !config.models.iter().any(|m| m.id == variant.model_id) {
                    warnings.push(format!(
                        "variant '{}' references unknown model '{}'",
                        variant.id, variant.model_id
                    ));
                }
            }

            // Check auth config
            if config.auth.enabled && config.auth.keys.is_empty() {
                warnings.push("auth is enabled but no keys configured".to_string());
            }

            let json = serde_json::json!({
                "status": "valid",
                "providers": providers_count,
                "models": models_count,
                "variants": variants_count,
                "rate_limits": rate_limits_count,
                "warnings": warnings,
            });
            (StatusCode::OK, Json(json))
        }
        Err(e) => {
            let json = serde_json::json!({
                "status": "invalid",
                "error": e,
            });
            (StatusCode::BAD_REQUEST, Json(json))
        }
    }
}

// ──────────────────────────────────────────────
// Async Task handlers
// ──────────────────────────────────────────────

/// Query params for GET /v1/tasks
#[derive(Debug, Deserialize)]
pub struct TaskListQuery {
    pub client_key: Option<String>,
    pub status: Option<String>,
    pub limit: Option<usize>,
}

/// GET /v1/tasks — list tasks
pub async fn list_tasks_handler(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<TaskListQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(50).min(200);
    let tasks = state
        .async_tasks
        .list(query.client_key.as_deref(), query.status.as_deref(), limit);
    (StatusCode::OK, Json(serde_json::to_value(&tasks).unwrap()))
}

/// GET /v1/tasks/{task_id} — get task status
pub async fn get_task_handler(
    State(state): State<AppState>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.async_tasks.get(&task_id) {
        Some(entry) => (StatusCode::OK, Json(serde_json::to_value(&entry).unwrap())),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "task not found", "code": "NOT_FOUND"})),
        ),
    }
}

/// POST /v1/tasks/{task_id}/cancel — cancel task
pub async fn cancel_task_handler(
    State(state): State<AppState>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.async_tasks.cancel(&task_id) {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "cancelled", "task_id": task_id})),
        ),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e, "code": "CANCEL_FAILED"})),
        ),
    }
}

/// Webhook payload from upstream providers.
#[derive(Debug, Deserialize)]
pub struct WebhookPayload {
    pub task_id: String,
    pub status: Option<String>,
    pub progress: Option<f32>,
    pub result_url: Option<String>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// POST /v1/tasks/webhook — receive upstream webhook callbacks
pub async fn task_webhook_handler(
    State(state): State<AppState>,
    Json(payload): Json<WebhookPayload>,
) -> impl IntoResponse {
    // Look up by upstream task ID
    let entry = state.async_tasks.get_by_upstream(&payload.task_id);
    if entry.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "unknown upstream task_id", "code": "NOT_FOUND"})),
        );
    }
    let entry = entry.unwrap();

    let new_status = match payload.status.as_deref() {
        Some("completed") | Some("complete") | Some("success") => TaskStatus::Completed {
            result_url: payload.result_url,
            result: payload.result,
        },
        Some("failed") | Some("error") => TaskStatus::Failed {
            error: payload.error.unwrap_or_else(|| "unknown error".to_string()),
        },
        Some("processing") | Some("in_progress") | Some("running") => TaskStatus::Processing {
            progress: payload.progress,
        },
        Some("cancelled") | Some("canceled") => TaskStatus::Cancelled,
        _ => {
            // If no status string but has result_url, treat as completed
            if payload.result_url.is_some() || payload.result.is_some() {
                TaskStatus::Completed {
                    result_url: payload.result_url,
                    result: payload.result,
                }
            } else if payload.error.is_some() {
                TaskStatus::Failed {
                    error: payload.error.unwrap(),
                }
            } else {
                TaskStatus::Processing {
                    progress: payload.progress,
                }
            }
        }
    };

    state.async_tasks.update_status(&entry.task_id, new_status);
    (
        StatusCode::OK,
        Json(serde_json::json!({"status": "updated", "task_id": entry.task_id})),
    )
}

// ──────────────────────────────────────────────
// Admin auth middleware
// ──────────────────────────────────────────────

/// GET /admin/usage — query aggregated usage
#[cfg(feature = "usage-db")]
pub async fn usage_handler(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<prism_core::usage::UsageQuery>,
) -> impl IntoResponse {
    let store = match &state.usage_store {
        Some(s) => s.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(
                    serde_json::json!({"error": "usage-db not configured", "code": "UNAVAILABLE"}),
                ),
            );
        }
    };

    match tokio::task::spawn_blocking(move || store.query(&query)).await {
        Ok(Ok(results)) => (StatusCode::OK, Json(serde_json::to_value(results).unwrap())),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e, "code": "QUERY_FAILED"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string(), "code": "INTERNAL_ERROR"})),
        ),
    }
}

/// Build the Axum router with optional admin auth middleware.
pub fn build_app(state: AppState, admin_config: &AdminConfig) -> axum::Router {
    use axum::routing::{get, post};

    // Public routes (no auth)
    let public = axum::Router::new()
        .route("/v1/route", post(route_handler))
        .route("/v1/outcomes", post(outcomes_handler))
        .route("/v1/tasks", get(list_tasks_handler))
        .route("/v1/tasks/webhook", post(task_webhook_handler))
        .route("/v1/tasks/{task_id}", get(get_task_handler))
        .route("/v1/tasks/{task_id}/cancel", post(cancel_task_handler))
        .route("/health", get(health_handler))
        .route("/readyz", get(readyz_handler))
        .route("/version", get(version_handler))
        .route("/metrics", get(metrics_handler));

    // Admin routes
    let mut admin = axum::Router::new()
        .route("/admin/reload", post(reload_handler))
        .route("/admin/config/validate", post(validate_config_handler));

    // Usage endpoint (requires usage-db feature)
    #[cfg(feature = "usage-db")]
    {
        admin = admin.route("/admin/usage", get(usage_handler));
    }

    // Apply auth middleware to admin routes if token is configured
    let admin = if let Some(ref token) = admin_config.auth_token {
        let token = token.clone();
        admin.layer(axum::middleware::from_fn(move |req, next| {
            admin_auth_middleware(req, next, token.clone())
        }))
    } else {
        admin
    };

    public
        .merge(admin)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state)
}

/// Middleware that validates Bearer token on admin routes.
async fn admin_auth_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
    expected_token: String,
) -> axum::response::Response {
    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            if token == expected_token {
                next.run(req).await
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({"error": "invalid admin token", "code": "UNAUTHORIZED"})),
                )
                    .into_response()
            }
        }
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "missing or invalid Authorization header", "code": "UNAUTHORIZED"})),
        )
            .into_response(),
    }
}

// ──────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────

/// Parse a strategy name string into a `ModelSelectionStrategy`.
pub(crate) fn parse_strategy(name: Option<&str>) -> Option<ModelSelectionStrategy> {
    match name {
        Some("balanced") | None => None, // use default
        Some("cost_first") => Some(ModelSelectionStrategy::Auto {
            objective: prism_core::strategy::RoutingObjective::CostFirst {
                min_quality_score: 0.0,
                budget_per_request: None,
            },
            pool_ids: None,
            required_capabilities: None,
        }),
        Some("quality_first") => Some(ModelSelectionStrategy::Auto {
            objective: prism_core::strategy::RoutingObjective::QualityFirst {
                max_latency_ms: None,
            },
            pool_ids: None,
            required_capabilities: None,
        }),
        Some("latency_first") => Some(ModelSelectionStrategy::Auto {
            objective: prism_core::strategy::RoutingObjective::SloFirst {
                max_latency_ms: 5000,
                max_ttft_ms: 2000,
                min_quality_score: 0.0,
            },
            pool_ids: None,
            required_capabilities: None,
        }),
        Some(model_id) => {
            // Treat unknown strategy as pinned model request
            Some(ModelSelectionStrategy::Pinned {
                model_id: model_id.to_string(),
            })
        }
    }
}

// ──────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use axum::Router;
    use http_body_util::BodyExt;
    use prism_core::registry::InMemoryRegistry;
    use prism_core::router::PrismRouter;
    use prism_core::types::*;
    use std::collections::HashMap;
    use tower::ServiceExt;

    fn test_registry() -> InMemoryRegistry {
        let mut registry = InMemoryRegistry::new();

        registry.add_provider(Provider {
            id: "openai".into(),
            name: "OpenAI".into(),
            auth: AuthConfig::ApiKey {
                key: "Bearer sk-test".into(),
                header: "Authorization".into(),
            },
            base_url: "https://api.openai.com/v1".into(),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: vec!["openai".into()],
        });

        registry.add_endpoint(Endpoint {
            id: "openai-ep".into(),
            provider_id: "openai".into(),
            base_url: Some("https://api.openai.com/v1".into()),
            region: None,
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        });

        registry.add_account(Account {
            id: "openai-acct".into(),
            endpoint_id: "openai-ep".into(),
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
            id: "gpt-4o".into(),
            display_name: "GPT-4o".into(),
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
            id: "gpt-4o@openai".into(),
            model_id: "gpt-4o".into(),
            endpoint_id: "openai-ep".into(),
            account_id: Some("openai-acct".into()),
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
            variants: vec!["gpt-4o@openai".into()],
            selection_mode: SelectionMode::default(),
            description: None,
        });

        registry
    }

    fn test_app() -> Router {
        let state = AppState::new(
            PrismRouter::new(test_registry()),
            std::path::PathBuf::from("test.yaml"),
        );
        Router::new()
            .route("/v1/route", post(route_handler))
            .route("/v1/outcomes", post(outcomes_handler))
            .route("/health", get(health_handler))
            .route("/version", get(version_handler))
            .route("/metrics", get(metrics_handler))
            .with_state(state)
    }

    fn empty_app() -> Router {
        let state = AppState::new(
            PrismRouter::new(InMemoryRegistry::new()),
            std::path::PathBuf::from("test.yaml"),
        );
        Router::new()
            .route("/v1/route", post(route_handler))
            .route("/health", get(health_handler))
            .with_state(state)
    }

    async fn body_json(resp: axum::http::Response<axum::body::Body>) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    // ── POST /v1/route ──

    #[tokio::test]
    async fn test_http_route_basic() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"hello"}]}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["selected_model"], "gpt-4o");
        assert_eq!(json["selected_provider"], "openai");
        assert_eq!(json["selected_variant"], "gpt-4o@openai");
        assert_eq!(json["selected_account"], "openai-acct");
        assert!(json["score"].as_f64().unwrap() > 0.0);
        assert!(json["decision_time_us"].as_u64().is_some());
    }

    #[tokio::test]
    async fn test_http_route_empty_messages() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(r#"{"messages":[]}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let json = body_json(resp).await;
        assert_eq!(json["code"], "INVALID_REQUEST");
    }

    #[tokio::test]
    async fn test_http_route_with_strategy_cost_first() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"hi"}],"strategy":"cost_first"}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["selected_model"], "gpt-4o");
    }

    #[tokio::test]
    async fn test_http_route_with_strategy_quality_first() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"hi"}],"strategy":"quality_first"}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_http_route_with_strategy_latency_first() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"hi"}],"strategy":"latency_first"}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_http_route_pinned_via_strategy() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"hi"}],"strategy":"gpt-4o"}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["selected_model"], "gpt-4o");
    }

    #[tokio::test]
    async fn test_http_route_with_metadata() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"hi"}],"metadata":{"affinity_key":"sess-1","tenant_id":"acme","region":"us-east"}}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_http_route_with_required_protocol() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"hi"}],"required_protocol":"openai"}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["selected_model"], "gpt-4o");
    }

    #[tokio::test]
    async fn test_http_route_no_candidates() {
        let app = empty_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"hi"}]}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let json = body_json(resp).await;
        assert_eq!(json["code"], "NO_CANDIDATES");
    }

    #[tokio::test]
    async fn test_http_route_multiple_messages() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"system","content":"You are helpful"},{"role":"user","content":"hello"},{"role":"assistant","content":"Hi!"},{"role":"user","content":"How are you?"}]}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["selected_model"], "gpt-4o");
    }

    #[tokio::test]
    async fn test_http_route_response_has_fallback_chain() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"hi"}]}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let json = body_json(resp).await;
        assert!(json["fallback_chain"].is_array());
    }

    #[tokio::test]
    async fn test_http_route_response_has_reason() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/route")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"messages":[{"role":"user","content":"hi"}]}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let json = body_json(resp).await;
        assert!(json["reason"].is_object());
    }

    // ── POST /v1/outcomes ──

    #[tokio::test]
    async fn test_http_outcomes() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/outcomes")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"variant_id":"gpt-4o@openai","success":true,"latency_ms":150}"#,
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["status"], "recorded");
    }

    // ── GET /health ──

    #[tokio::test]
    async fn test_http_health() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["status"], "healthy");
        assert_eq!(json["registered_models"], 1);
        assert_eq!(json["active_variants"], 1);
        assert!(json["uptime_seconds"].as_u64().is_some());
    }

    #[tokio::test]
    async fn test_http_health_empty_registry() {
        let app = empty_app();
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["registered_models"], 0);
        assert_eq!(json["active_variants"], 0);
    }

    // ── GET /version ──

    #[tokio::test]
    async fn test_http_version() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/version")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["name"], "prism-router");
        assert!(json["version"].as_str().is_some());
    }

    // ── GET /metrics ──

    #[tokio::test]
    async fn test_http_metrics() {
        let app = test_app();
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/metrics")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("prism_requests_total"));
    }

    // ── parse_strategy ──

    #[test]
    fn test_parse_strategy_balanced() {
        assert!(parse_strategy(Some("balanced")).is_none());
    }

    #[test]
    fn test_parse_strategy_none() {
        assert!(parse_strategy(None).is_none());
    }

    #[test]
    fn test_parse_strategy_cost_first() {
        let s = parse_strategy(Some("cost_first")).unwrap();
        match s {
            ModelSelectionStrategy::Auto { objective, .. } => {
                assert!(matches!(
                    objective,
                    prism_core::strategy::RoutingObjective::CostFirst { .. }
                ));
            }
            _ => panic!("expected Auto"),
        }
    }

    #[test]
    fn test_parse_strategy_quality_first() {
        let s = parse_strategy(Some("quality_first")).unwrap();
        match s {
            ModelSelectionStrategy::Auto { objective, .. } => {
                assert!(matches!(
                    objective,
                    prism_core::strategy::RoutingObjective::QualityFirst { .. }
                ));
            }
            _ => panic!("expected Auto"),
        }
    }

    #[test]
    fn test_parse_strategy_latency_first() {
        let s = parse_strategy(Some("latency_first")).unwrap();
        match s {
            ModelSelectionStrategy::Auto { objective, .. } => {
                assert!(matches!(
                    objective,
                    prism_core::strategy::RoutingObjective::SloFirst { .. }
                ));
            }
            _ => panic!("expected Auto"),
        }
    }

    #[test]
    fn test_parse_strategy_pinned() {
        let s = parse_strategy(Some("claude-3-opus")).unwrap();
        match s {
            ModelSelectionStrategy::Pinned { model_id } => {
                assert_eq!(model_id, "claude-3-opus");
            }
            _ => panic!("expected Pinned"),
        }
    }
}
