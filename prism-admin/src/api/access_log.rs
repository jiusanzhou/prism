use crate::AppState;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

/// GET /api/access-log
pub async fn get_access_log(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.get_access_log().await {
        Ok(access_log) => (
            StatusCode::OK,
            Json(serde_json::to_value(&access_log).unwrap_or_default()),
        )
            .into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// PUT /api/access-log
pub async fn update_access_log(
    State(state): State<AppState>,
    Json(access_log): Json<prism_router::config::types::AccessLogConfig>,
) -> impl IntoResponse {
    match state.store.set_access_log(access_log).await {
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
