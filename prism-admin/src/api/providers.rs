use axum::{extract::{Path, State}, http::StatusCode, response::IntoResponse, Json};
use crate::AppState;

/// GET /api/providers
pub async fn list_providers(State(state): State<AppState>) -> impl IntoResponse {
    match load_config(&state) {
        Ok(config) => {
            let providers: Vec<serde_json::Value> = config.providers
                .iter()
                .map(|(id, p)| serde_json::json!({
                    "id": id,
                    "base_url": p.base_url,
                    "protocols": p.protocols,
                    "circuit_breaker_threshold": p.circuit_breaker_threshold,
                    "circuit_breaker_recovery_secs": p.circuit_breaker_recovery_secs,
                }))
                .collect();
            (StatusCode::OK, Json(serde_json::json!({"providers": providers}))).into_response()
        }
        Err(e) => error_response(e),
    }
}

/// GET /api/providers/:id
pub async fn get_provider(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match load_config(&state) {
        Ok(config) => {
            match config.providers.get(&id) {
                Some(p) => (StatusCode::OK, Json(serde_json::to_value(p).unwrap())).into_response(),
                None => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "provider not found"}))).into_response(),
            }
        }
        Err(e) => error_response(e),
    }
}

/// PUT /api/providers/:id
pub async fn update_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(provider): Json<prism_router::config::types::ProviderConfig>,
) -> impl IntoResponse {
    match load_and_modify(&state, |config| {
        config.providers.insert(id.clone(), provider);
    }) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "ok", "id": id}))).into_response(),
        Err(e) => error_response(e),
    }
}

/// DELETE /api/providers/:id
pub async fn delete_provider(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match load_and_modify(&state, |config| {
        config.providers.remove(&id);
    }) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"status": "deleted", "id": id}))).into_response(),
        Err(e) => error_response(e),
    }
}

fn load_config(state: &AppState) -> Result<prism_router::config::types::PrismConfig, String> {
    let yaml = std::fs::read_to_string(&state.config_path).map_err(|e| e.to_string())?;
    prism_router::config::load_config_from_str(&yaml)
}

fn load_and_modify(state: &AppState, modify: impl FnOnce(&mut prism_router::config::types::PrismConfig)) -> Result<(), String> {
    let mut config = load_config(state)?;
    modify(&mut config);
    let yaml = serde_yaml::to_string(&config).map_err(|e| e.to_string())?;
    std::fs::write(&state.config_path, yaml).map_err(|e| e.to_string())
}

fn error_response(e: String) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e}))).into_response()
}
