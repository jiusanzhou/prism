use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use crate::AppState;

/// GET /api/variants
pub async fn list_variants(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.list_variants().await {
        Ok(variants) => {
            let variants: Vec<serde_json::Value> = variants.iter().map(|v| {
                serde_json::json!({
                    "id": v.id,
                    "model_id": v.model_id,
                    "provider_id": v.provider_id,
                    "weight": v.weight,
                    "account_id": v.account_id,
                    "upstream_model_name": v.upstream_model_name,
                    "base_url": v.base_url,
                })
            }).collect();
            (StatusCode::OK, Json(serde_json::json!({"variants": variants}))).into_response()
        }
        Err(e) => error_response(e.to_string()),
    }
}

/// POST /api/variants
pub async fn create_variant(
    State(state): State<AppState>,
    Json(variant): Json<prism_router::config::types::VariantConfig>,
) -> impl IntoResponse {
    let id = variant.id.clone();
    match state.store.upsert_variant(&id, variant).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok", "id": id}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// PUT /api/variants/:id
pub async fn update_variant(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(variant): Json<prism_router::config::types::VariantConfig>,
) -> impl IntoResponse {
    match state.store.upsert_variant(&id, variant).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok", "id": id}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// DELETE /api/variants/:id
pub async fn delete_variant(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.delete_variant(&id).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "deleted", "id": id}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

fn error_response(e: String) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))).into_response()
}
