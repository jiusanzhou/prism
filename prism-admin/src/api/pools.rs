use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use crate::AppState;

/// GET /api/pools
pub async fn list_pools(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.list_pools().await {
        Ok(pools) => {
            let pools: Vec<serde_json::Value> = pools.iter().map(|p| {
                serde_json::json!({
                    "id": p.id,
                    "name": p.name,
                    "variants": p.variants,
                    "selection_mode": p.selection_mode.as_ref().map(|s| format!("{:?}", s)),
                })
            }).collect();
            (StatusCode::OK, Json(serde_json::json!({"pools": pools}))).into_response()
        }
        Err(e) => error_response(e.to_string()),
    }
}

/// POST /api/pools
pub async fn create_pool(
    State(state): State<AppState>,
    Json(pool): Json<prism_router::config::types::PoolConfig>,
) -> impl IntoResponse {
    let id = pool.id.clone();
    match state.store.upsert_pool(&id, pool).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok", "id": id}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// PUT /api/pools/:id
pub async fn update_pool(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(pool): Json<prism_router::config::types::PoolConfig>,
) -> impl IntoResponse {
    match state.store.upsert_pool(&id, pool).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok", "id": id}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// DELETE /api/pools/:id
pub async fn delete_pool(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.delete_pool(&id).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "deleted", "id": id}))).into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

fn error_response(e: String) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))).into_response()
}
