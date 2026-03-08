use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::collections::HashMap;

use crate::cluster::{ProxyInstance, ProxyStatus};
use crate::AppState;

fn error_response(e: impl std::fmt::Display) -> impl IntoResponse {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": e.to_string()})),
    )
}

#[derive(Deserialize)]
pub struct RegisterProxyRequest {
    pub instance_id: String,
    pub cluster_id: String,
    pub address: String,
    pub version: String,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// POST /api/proxies/register — proxy self-registration
pub async fn register_proxy(
    State(state): State<AppState>,
    Json(req): Json<RegisterProxyRequest>,
) -> impl IntoResponse {
    let now = chrono::Utc::now();
    let proxy = ProxyInstance {
        instance_id: req.instance_id,
        cluster_id: req.cluster_id.clone(),
        address: req.address,
        version: req.version,
        status: ProxyStatus::Online,
        last_heartbeat: now,
        registered_at: now,
        config_version: 0,
        metadata: req.metadata,
    };

    match state.store.register_proxy(proxy).await {
        Ok(()) => {
            // Return current config for the cluster
            match state.store.cluster_config(&req.cluster_id).await {
                Ok(config) => {
                    let config_json = serde_json::to_value(&config).unwrap_or_default();
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "config_version": 0,
                            "config": config_json,
                        })),
                    )
                        .into_response()
                }
                Err(e) => error_response(e).into_response(),
            }
        }
        Err(crate::store::StoreError::NotFound(msg)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": msg})),
        )
            .into_response(),
        Err(e) => error_response(e).into_response(),
    }
}

#[derive(Deserialize)]
pub struct HeartbeatRequest {
    pub instance_id: String,
    #[serde(default)]
    pub config_version: u64,
}

/// POST /api/proxies/heartbeat — proxy heartbeat
pub async fn heartbeat_proxy(
    State(state): State<AppState>,
    Json(req): Json<HeartbeatRequest>,
) -> impl IntoResponse {
    match state
        .store
        .heartbeat_proxy(&req.instance_id, req.config_version)
        .await
    {
        Ok(()) => {
            // Check if proxy needs config update
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "needs_update": false,
                })),
            )
                .into_response()
        }
        Err(crate::store::StoreError::NotFound(msg)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": msg})),
        )
            .into_response(),
        Err(e) => error_response(e).into_response(),
    }
}

#[derive(Deserialize)]
pub struct ListProxiesQuery {
    pub cluster_id: Option<String>,
}

/// GET /api/proxies — list all proxies (optional ?cluster_id= filter)
pub async fn list_proxies(
    State(state): State<AppState>,
    Query(query): Query<ListProxiesQuery>,
) -> impl IntoResponse {
    match state.store.list_proxies(query.cluster_id.as_deref()).await {
        Ok(proxies) => (StatusCode::OK, Json(serde_json::json!(proxies))).into_response(),
        Err(e) => error_response(e).into_response(),
    }
}

/// DELETE /api/proxies/{instance_id} — force remove a proxy
pub async fn delete_proxy(
    State(state): State<AppState>,
    Path(instance_id): Path<String>,
) -> impl IntoResponse {
    match state.store.unregister_proxy(&instance_id).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Err(e) => error_response(e).into_response(),
    }
}
