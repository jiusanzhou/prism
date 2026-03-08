use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

/// GET /api/accounts
pub async fn list_accounts(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.list_accounts().await {
        Ok(accounts) => {
            let accounts: Vec<serde_json::Value> = accounts
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "id": a.id,
                        "provider_id": a.provider_id,
                        "billing_tag": a.billing_tag,
                        "enabled": a.enabled,
                        "weight": a.weight,
                        "rate_limit": a.rate_limit.as_ref().map(|rl| serde_json::json!({
                            "rpm": rl.rpm,
                            "tpm": rl.tpm,
                            "concurrent": rl.concurrent,
                        })),
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({"accounts": accounts})),
            )
                .into_response()
        }
        Err(e) => error_response(e.to_string()),
    }
}

/// GET /api/accounts/:id
pub async fn get_account(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.get_account(&id).await {
        Ok(a) => (StatusCode::OK, Json(serde_json::to_value(a).unwrap())).into_response(),
        Err(crate::store::StoreError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "account not found"})),
        )
            .into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// POST /api/accounts
pub async fn create_account(
    State(state): State<AppState>,
    Json(account): Json<prism_router::config::types::AccountConfig>,
) -> impl IntoResponse {
    let id = account.id.clone();
    match state.store.upsert_account(&id, account).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "ok", "id": id})),
        )
            .into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// PUT /api/accounts/:id
pub async fn update_account(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(account): Json<prism_router::config::types::AccountConfig>,
) -> impl IntoResponse {
    match state.store.upsert_account(&id, account).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "ok", "id": id})),
        )
            .into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// DELETE /api/accounts/:id
pub async fn delete_account(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.delete_account(&id).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "deleted", "id": id})),
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
