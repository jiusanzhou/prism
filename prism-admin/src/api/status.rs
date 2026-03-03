use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use crate::AppState;







/// GET /api/status — system overview
pub async fn get_status(State(state): State<AppState>) -> impl IntoResponse {
    match std::fs::read_to_string(&state.config_path)
        .map_err(|e| e.to_string())
        .and_then(|y| prism_router::config::load_config_from_str(&y))
    {
        Ok(config) => {
            (StatusCode::OK, Json(serde_json::json!({
                "config_path": state.config_path.display().to_string(),
                "providers": config.providers.len(),
                "models": config.models.len(),
                "variants": config.variants.len(),
                "rate_limits": config.rate_limits.len(),
                "auth_enabled": config.auth.enabled,
                "concurrency_limits": config.concurrency_limits.len(),
                "access_log_enabled": config.access_log.enabled,
            }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))).into_response(),
    }
}
