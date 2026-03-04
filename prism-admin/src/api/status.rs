use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use crate::AppState;

/// GET /api/status — system overview
pub async fn get_status(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.snapshot().await {
        Ok(config) => {
            (StatusCode::OK, Json(serde_json::json!({
                "store": state.store_name,
                "providers": config.providers.len(),
                "models": config.models.len(),
                "variants": config.variants.len(),
                "accounts": config.accounts.len(),
                "rate_limits": config.rate_limits.len(),
                "auth_enabled": config.auth.enabled,
                "concurrency_limits": config.concurrency_limits.len(),
                "access_log_enabled": config.access_log.enabled,
            }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}
