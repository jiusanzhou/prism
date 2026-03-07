use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use crate::AppState;

/// GET /api/routing
pub async fn get_routing(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.snapshot().await {
        Ok(config) => {
            (StatusCode::OK, Json(serde_json::json!({ "routing": config.routing }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

/// PUT /api/routing
pub async fn update_routing(
    State(state): State<AppState>,
    Json(routing): Json<prism_router::config::types::RoutingConfig>,
) -> impl IntoResponse {
    // Get current config, update just the routing section, save back
    match state.store.snapshot().await {
        Ok(mut config) => {
            config.routing = routing;
            match state.store.import_config(config).await {
                Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok"}))).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}
