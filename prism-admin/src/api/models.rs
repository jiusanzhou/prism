use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use crate::AppState;

/// GET /api/models
pub async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    match std::fs::read_to_string(&state.config_path)
        .map_err(|e| e.to_string())
        .and_then(|y| prism_router::config::load_config_from_str(&y))
    {
        Ok(config) => {
            let models: Vec<serde_json::Value> = config.models.iter().map(|m| {
                serde_json::json!({
                    "id": m.id,
                    "display_name": m.display_name,
                    "quality_tier": format!("{:?}", m.quality_tier),
                })
            }).collect();
            (StatusCode::OK, Json(serde_json::json!({"models": models}))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))).into_response(),
    }
}






