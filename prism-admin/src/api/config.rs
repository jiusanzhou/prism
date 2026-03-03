use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

use crate::AppState;

/// GET /api/config — read current config as JSON
pub async fn get_config(State(state): State<AppState>) -> impl IntoResponse {
    match std::fs::read_to_string(&state.config_path) {
        Ok(yaml_str) => {
            match serde_yaml::from_str::<serde_json::Value>(&yaml_str) {
                Ok(val) => (StatusCode::OK, Json(val)).into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("parse error: {}", e)})),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("read error: {}", e)})),
        )
            .into_response(),
    }
}

/// PUT /api/config — write full config (YAML body)
pub async fn update_config(
    State(state): State<AppState>,
    body: String,
) -> impl IntoResponse {
    // Validate first
    match prism_router::config::load_config_from_str(&body) {
        Ok(_config) => {
            // Write to file
            match std::fs::write(&state.config_path, &body) {
                Ok(()) => (
                    StatusCode::OK,
                    Json(serde_json::json!({"status": "ok", "message": "Config saved. Reload required to apply."})),
                ).into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("write error: {}", e)})),
                ).into_response(),
            }
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"status": "invalid", "error": e})),
        ).into_response(),
    }
}

/// POST /api/config/validate — dry-run validation
pub async fn validate_config(body: String) -> impl IntoResponse {
    match prism_router::config::load_config_from_str(&body) {
        Ok(config) => {
            let mut warnings = Vec::new();
            for variant in &config.variants {
                if !config.providers.contains_key(&variant.provider_id) {
                    warnings.push(format!("variant '{}' → unknown provider '{}'", variant.id, variant.provider_id));
                }
                if !config.models.iter().any(|m| m.id == variant.model_id) {
                    warnings.push(format!("variant '{}' → unknown model '{}'", variant.id, variant.model_id));
                }
            }
            (StatusCode::OK, Json(serde_json::json!({
                "status": "valid",
                "providers": config.providers.len(),
                "models": config.models.len(),
                "variants": config.variants.len(),
                "warnings": warnings,
            }))).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"status": "invalid", "error": e})),
        ).into_response(),
    }
}
