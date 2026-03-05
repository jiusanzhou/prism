use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::collections::HashMap;

use crate::cluster::{Cluster, ProxyStatus};
use crate::push::ConfigPusher;
use crate::AppState;

fn error_response(e: impl std::fmt::Display) -> impl IntoResponse {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": e.to_string()})),
    )
}

/// GET /api/clusters — list all clusters with proxy count
pub async fn list_clusters(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.list_clusters().await {
        Ok(clusters) => {
            let proxies = state.store.list_proxies(None).await.unwrap_or_default();
            let result: Vec<serde_json::Value> = clusters
                .into_iter()
                .map(|c| {
                    let proxy_count = proxies.iter().filter(|p| p.cluster_id == c.id).count();
                    let online_count = proxies
                        .iter()
                        .filter(|p| p.cluster_id == c.id && p.status == ProxyStatus::Online)
                        .count();
                    serde_json::json!({
                        "id": c.id,
                        "name": c.name,
                        "description": c.description,
                        "tags": c.tags,
                        "created_at": c.created_at,
                        "proxy_count": proxy_count,
                        "online_count": online_count,
                    })
                })
                .collect();
            (StatusCode::OK, Json(serde_json::json!(result))).into_response()
        }
        Err(e) => error_response(e).into_response(),
    }
}

#[derive(Deserialize)]
pub struct CreateClusterRequest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

/// POST /api/clusters — create cluster
pub async fn create_cluster(
    State(state): State<AppState>,
    Json(req): Json<CreateClusterRequest>,
) -> impl IntoResponse {
    let cluster = Cluster {
        id: req.id.clone(),
        name: req.name,
        description: req.description,
        tags: req.tags,
        created_at: chrono::Utc::now(),
    };
    match state.store.upsert_cluster(&req.id, cluster).await {
        Ok(()) => (StatusCode::CREATED, Json(serde_json::json!({"ok": true}))).into_response(),
        Err(e) => error_response(e).into_response(),
    }
}

/// GET /api/clusters/{id} — get cluster detail with its proxies
pub async fn get_cluster(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.get_cluster(&id).await {
        Ok(cluster) => {
            let proxies = state.store.list_proxies(Some(&id)).await.unwrap_or_default();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": cluster.id,
                    "name": cluster.name,
                    "description": cluster.description,
                    "tags": cluster.tags,
                    "created_at": cluster.created_at,
                    "proxies": proxies,
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
pub struct UpdateClusterRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

/// PUT /api/clusters/{id} — update cluster
pub async fn update_cluster(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateClusterRequest>,
) -> impl IntoResponse {
    // Get existing to preserve created_at
    let existing = match state.store.get_cluster(&id).await {
        Ok(c) => c,
        Err(crate::store::StoreError::NotFound(msg)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": msg})),
            )
                .into_response()
        }
        Err(e) => return error_response(e).into_response(),
    };
    let cluster = Cluster {
        id: id.clone(),
        name: req.name,
        description: req.description,
        tags: req.tags,
        created_at: existing.created_at,
    };
    match state.store.upsert_cluster(&id, cluster).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Err(e) => error_response(e).into_response(),
    }
}

/// DELETE /api/clusters/{id} — delete cluster (must have no proxies)
pub async fn delete_cluster(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.delete_cluster(&id).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Err(crate::store::StoreError::Conflict(msg)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": msg})),
        )
            .into_response(),
        Err(e) => error_response(e).into_response(),
    }
}

/// GET /api/clusters/{id}/config — get effective config for cluster
pub async fn get_cluster_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.cluster_config(&id).await {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => error_response(e).into_response(),
    }
}

/// POST /api/clusters/{id}/sync — trigger config push to cluster proxies
pub async fn sync_cluster(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let config = match state.store.cluster_config(&id).await {
        Ok(c) => c,
        Err(e) => return error_response(e).into_response(),
    };
    let proxies = match state.store.list_proxies(Some(&id)).await {
        Ok(p) => p,
        Err(e) => return error_response(e).into_response(),
    };
    let results = ConfigPusher::push_to_cluster(&proxies, &config).await;
    let summary: Vec<serde_json::Value> = results
        .into_iter()
        .map(|(id, result)| {
            serde_json::json!({
                "instance_id": id,
                "success": result.is_ok(),
                "error": result.err(),
            })
        })
        .collect();
    (StatusCode::OK, Json(serde_json::json!({"results": summary}))).into_response()
}
