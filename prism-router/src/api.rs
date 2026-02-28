use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use prism_core::analyzer::ChatMessage;
use prism_core::registry::ModelRegistryTrait;
use prism_core::feedback::OutcomeReport;
use prism_core::router::{RouteMetadata, RouteRequest, Router, RouterError};
use prism_core::strategy::ModelSelectionStrategy;

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
            Json(serde_json::to_value(ErrorResponse {
                error: "messages must not be empty".to_string(),
                code: "INVALID_REQUEST".to_string(),
            })
            .unwrap()),
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
            analyze_context: None,
        },
        None => RouteMetadata::default(),
    };

    // Parse strategy from string to enum
    let strategy = parse_strategy(body.strategy.as_deref());

    let request = RouteRequest {
        messages,
        strategy,
        metadata,
        analyze_context: None,
        required_protocol: body.required_protocol,
    };

    match state.router.route(&request) {
        Ok(decision) => {
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
            (StatusCode::OK, Json(serde_json::to_value(response).unwrap()))
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
    state.router.record_outcome(&report);
    (
        StatusCode::OK,
        Json(serde_json::json!({"status": "recorded"})),
    )
}

/// GET /health — health check
pub async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let candidates = state.router.registry.get_all_candidates();
    let model_count = state.router.registry.models.len();
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

/// GET /version — version info
pub async fn version_handler() -> impl IntoResponse {
    let response = VersionResponse {
        name: "prism-router".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    (StatusCode::OK, Json(response))
}

/// GET /metrics — Prometheus metrics placeholder
pub async fn metrics_handler() -> impl IntoResponse {
    // TODO: integrate with prometheus crate
    (
        StatusCode::OK,
        "# HELP prism_router_requests_total Total routing requests\n\
         # TYPE prism_router_requests_total counter\n\
         prism_router_requests_total 0\n",
    )
}

// ──────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────

/// Parse a strategy name string into a `ModelSelectionStrategy`.
fn parse_strategy(name: Option<&str>) -> Option<ModelSelectionStrategy> {
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
