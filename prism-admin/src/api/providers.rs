use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use crate::AppState;

/// GET /api/providers
pub async fn list_providers(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.list_providers().await {
        Ok(providers) => {
            let providers: Vec<serde_json::Value> = providers
                .iter()
                .map(|(id, p)| serde_json::json!({
                    "id": id,
                    "base_url": p.base_url,
                    "auth_type": p.auth_type,
                    "protocols": p.protocols,
                    "headers": p.headers,
                    "circuit_breaker_threshold": p.circuit_breaker_threshold,
                    "circuit_breaker_recovery_secs": p.circuit_breaker_recovery_secs,
                }))
                .collect();
            (StatusCode::OK, Json(serde_json::json!({"providers": providers}))).into_response()
        }
        Err(e) => error_response(e.to_string()),
    }
}

/// GET /api/providers/:id
pub async fn get_provider(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match state.store.get_provider(&id).await {
        Ok(p) => (StatusCode::OK, Json(serde_json::to_value(p).unwrap())).into_response(),
        Err(crate::store::StoreError::NotFound(_)) => {
            (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "provider not found"}))).into_response()
        }
        Err(e) => error_response(e.to_string()),
    }
}

/// PUT /api/providers/:id
pub async fn update_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(provider): Json<prism_router::config::types::ProviderConfig>,
) -> impl IntoResponse {
    match state.store.upsert_provider(&id, provider).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok", "id": id}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// DELETE /api/providers/:id
pub async fn delete_provider(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match state.store.delete_provider(&id).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "deleted", "id": id}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

fn error_response(e: String) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))).into_response()
}
