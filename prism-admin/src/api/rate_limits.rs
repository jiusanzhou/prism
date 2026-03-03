use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use crate::AppState;





/// GET /api/rate-limits
pub async fn list_rate_limits(State(state): State<AppState>) -> impl IntoResponse {
    match std::fs::read_to_string(&state.config_path)
        .map_err(|e| e.to_string())
        .and_then(|y| prism_router::config::load_config_from_str(&y))
    {
        Ok(config) => {
            let rules: Vec<serde_json::Value> = config.rate_limits.iter().map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "window_secs": r.window_secs,
                    "max_requests": r.max_requests,
                    "enabled": r.enabled,
                })
            }).collect();
            (StatusCode::OK, Json(serde_json::json!({"rate_limits": rules}))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))).into_response(),
    }
}


