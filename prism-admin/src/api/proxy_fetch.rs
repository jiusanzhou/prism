use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::AppState;

#[derive(Deserialize)]
pub struct ProxyFetchRequest {
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub method: Option<String>,
}

#[derive(Serialize)]
pub struct ProxyFetchResponse {
    pub status: u16,
    pub body: serde_json::Value,
}

pub async fn proxy_fetch(
    State(_state): State<AppState>,
    Json(req): Json<ProxyFetchRequest>,
) -> Result<Json<ProxyFetchResponse>, (StatusCode, String)> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("client error: {}", e),
            )
        })?;

    let method = req.method.as_deref().unwrap_or("GET");
    let mut builder = match method.to_uppercase().as_str() {
        "POST" => client.post(&req.url),
        "PUT" => client.put(&req.url),
        "DELETE" => client.delete(&req.url),
        _ => client.get(&req.url),
    };

    for (key, value) in &req.headers {
        builder = builder.header(key.as_str(), value.as_str());
    }

    let response = builder
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("fetch error: {}", e)))?;

    let status = response.status().as_u16();
    let body_text = response
        .text()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("read body error: {}", e)))?;

    let body: serde_json::Value =
        serde_json::from_str(&body_text).unwrap_or(serde_json::Value::String(body_text));

    Ok(Json(ProxyFetchResponse { status, body }))
}
