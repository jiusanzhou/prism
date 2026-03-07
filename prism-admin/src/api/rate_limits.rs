use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use crate::AppState;

/// GET /api/rate-limits
pub async fn list_rate_limits(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.list_rate_limits().await {
        Ok(rate_limits) => {
            let rules: Vec<serde_json::Value> = rate_limits.iter().map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "key_headers": r.key_headers,
                    "window_secs": r.window_secs,
                    "max_requests": r.max_requests,
                    "max_tokens": r.max_tokens,
                    "enabled": r.enabled,
                    "overrides": r.overrides.len(),
                })
            }).collect();
            (StatusCode::OK, Json(serde_json::json!({"rate_limits": rules}))).into_response()
        }
        Err(e) => error_response(e.to_string()),
    }
}

/// POST /api/rate-limits
pub async fn create_rate_limit(
    State(state): State<AppState>,
    Json(rule): Json<prism_router::config::types::RateLimitConfig>,
) -> impl IntoResponse {
    let name = rule.name.clone();
    match state.store.upsert_rate_limit(&name, rule).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok", "name": name}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// PUT /api/rate-limits/:name
pub async fn update_rate_limit(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(rule): Json<prism_router::config::types::RateLimitConfig>,
) -> impl IntoResponse {
    match state.store.upsert_rate_limit(&name, rule).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok", "name": name}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// DELETE /api/rate-limits/:name
pub async fn delete_rate_limit(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match state.store.delete_rate_limit(&name).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "deleted", "name": name}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

fn error_response(e: String) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))).into_response()
}
