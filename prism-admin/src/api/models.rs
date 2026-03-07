use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use crate::AppState;

/// GET /api/models
pub async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.list_models().await {
        Ok(models) => {
            let models: Vec<serde_json::Value> = models.iter().map(|m| {
                serde_json::json!({
                    "id": m.id,
                    "display_name": m.display_name,
                    "quality_tier": format!("{:?}", m.quality_tier),
                    "capabilities": serde_json::to_value(&m.capabilities).unwrap_or_default(),
                    "pricing": serde_json::to_value(&m.pricing).unwrap_or_default(),
                    "specialties": m.specialties.iter().map(|s| format!("{:?}", s)).collect::<Vec<_>>(),
                })
            }).collect();
            (StatusCode::OK, Json(serde_json::json!({"models": models}))).into_response()
        }
        Err(e) => error_response(e.to_string()),
    }
}

/// POST /api/models
pub async fn create_model(
    State(state): State<AppState>,
    Json(model): Json<prism_router::config::types::ModelConfig>,
) -> impl IntoResponse {
    let id = model.id.clone();
    match state.store.upsert_model(&id, model).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok", "id": id}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// PUT /api/models/:id
pub async fn update_model(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(model): Json<prism_router::config::types::ModelConfig>,
) -> impl IntoResponse {
    match state.store.upsert_model(&id, model).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok", "id": id}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// DELETE /api/models/:id
pub async fn delete_model(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.delete_model(&id).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "deleted", "id": id}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

fn error_response(e: String) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))).into_response()
}
