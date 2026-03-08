use crate::AppState;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

/// GET /api/auth
pub async fn get_auth(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.get_auth().await {
        Ok(auth) => (
            StatusCode::OK,
            Json(serde_json::to_value(&auth).unwrap_or_default()),
        )
            .into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// PUT /api/auth
pub async fn update_auth(
    State(state): State<AppState>,
    Json(auth): Json<prism_router::config::types::AuthConfig>,
) -> impl IntoResponse {
    match state.store.set_auth(auth).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok"}))).into_response(),
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
