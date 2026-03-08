use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

/// GET /api/plugins
pub async fn list_plugins(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.list_plugins().await {
        Ok(plugins) => {
            let plugins: Vec<serde_json::Value> = plugins
                .iter()
                .map(|p| {
                    let source_type = match &p.source {
                        prism_router::config::types::PluginSourceConfig::Wasm { wasm } => {
                            serde_json::json!({"type": "wasm", "path": wasm})
                        }
                        prism_router::config::types::PluginSourceConfig::Builtin { builtin } => {
                            serde_json::json!({"type": "builtin", "name": builtin})
                        }
                        prism_router::config::types::PluginSourceConfig::Url { url, sha256 } => {
                            serde_json::json!({"type": "url", "url": url, "sha256": sha256})
                        }
                        prism_router::config::types::PluginSourceConfig::Oci { oci, digest } => {
                            serde_json::json!({"type": "oci", "oci": oci, "digest": digest})
                        }
                    };
                    serde_json::json!({
                        "name": p.name,
                        "source": source_type,
                        "phases": p.phases,
                        "config": p.config,
                        "enabled": p.enabled,
                        "priority": p.priority,
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({"plugins": plugins})),
            )
                .into_response()
        }
        Err(e) => error_response(e.to_string()),
    }
}

/// POST /api/plugins
pub async fn create_plugin(
    State(state): State<AppState>,
    Json(plugin): Json<prism_router::config::types::PluginConfig>,
) -> impl IntoResponse {
    let name = plugin.name.clone();
    match state.store.upsert_plugin(&name, plugin).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "ok", "name": name})),
        )
            .into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// PUT /api/plugins/:name
pub async fn update_plugin(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(plugin): Json<prism_router::config::types::PluginConfig>,
) -> impl IntoResponse {
    match state.store.upsert_plugin(&name, plugin).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "ok", "name": name})),
        )
            .into_response(),
        Err(e) => error_response(e.to_string()),
    }
}

/// DELETE /api/plugins/:name
pub async fn delete_plugin(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match state.store.delete_plugin(&name).await {
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
