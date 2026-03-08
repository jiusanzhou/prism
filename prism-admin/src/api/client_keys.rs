use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

/// GET /api/client-keys
pub async fn list_client_keys(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.list_client_keys().await {
        Ok(keys) => {
            let keys: Vec<serde_json::Value> = keys
                .iter()
                .map(|k| {
                    let masked_key = if k.key.len() > 4 {
                        format!("{}...{}", &k.key[..3], &k.key[k.key.len() - 4..])
                    } else {
                        "****".to_string()
                    };
                    serde_json::json!({
                        "name": k.name,
                        "key_masked": masked_key,
                        "enabled": k.enabled,
                        "allowed_models": k.allowed_models,
                        "quota": {
                            "max_requests_per_minute": k.quota.max_requests_per_minute,
                            "max_tokens_per_day": k.quota.max_tokens_per_day,
                            "max_concurrent": k.quota.max_concurrent,
                        },
                        "metadata": k.metadata,
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({"client_keys": keys})),
            )
                .into_response()
        }
        Err(e) => error_response(e.to_string()),
    }
}

/// GET /api/client-keys/:name
pub async fn get_client_key(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match state.store.get_client_key(&name).await {
        Ok(k) => (StatusCode::OK, Json(serde_json::to_value(k).unwrap())).into_response(),
        Err(crate::store::StoreError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "client key not found"})),
        )
            .into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// POST /api/client-keys
pub async fn create_client_key(
    State(state): State<AppState>,
    Json(key): Json<prism_router::config::types::ClientKeyConfig>,
) -> impl IntoResponse {
    let name = key.name.clone();
    match state.store.upsert_client_key(&name, key).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "ok", "name": name})),
        )
            .into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// PUT /api/client-keys/:name
pub async fn update_client_key(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(key): Json<prism_router::config::types::ClientKeyConfig>,
) -> impl IntoResponse {
    match state.store.upsert_client_key(&name, key).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "ok", "name": name})),
        )
            .into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// DELETE /api/client-keys/:name
pub async fn delete_client_key(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match state.store.delete_client_key(&name).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "deleted", "name": name})),
        )
            .into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

fn error_response(e: String) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": e})),
    )
        .into_response()
}
