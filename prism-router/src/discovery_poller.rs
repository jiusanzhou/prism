//! Discovery Poller — background task for HTTP discovery sources.
//!
//! Supports two modes:
//! - **Poll**: periodic full fetch from `/prism` endpoint (fallback)
//! - **SSE**: event-driven streaming from `/events` endpoint (preferred)
//!
//! On SSE connect, the sidecar sends a `full_sync` event with all instances.
//! Subsequent events are incremental: `instance_add`, `instance_del`, `model_remove`.
//! If SSE disconnects, falls back to poll mode and retries SSE.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::Notify;
use tracing::{error, info, warn};

use prism_core::discovery::Discovery;

use crate::config::{build_registry, build_router_from_config, load_config};
use crate::config::types::{DiscoverySourceType, PrismConfig};
use crate::http_discovery::{HttpDiscovery, HttpDiscoveryConfig, HttpDiscoveryPayload};
use crate::state::AppState;

// ── SSE Event Types ──

#[derive(Debug, Clone, Deserialize)]
struct SseDiscoveryEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    instance: Option<SseInstance>,
    #[serde(default)]
    timestamp: i64,
    #[serde(default)]
    snapshot: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct SseInstance {
    address: String,
    protocol: String,
    #[serde(default)]
    pod_name: Option<String>,
    #[serde(default)]
    labels: Option<serde_json::Map<String, serde_json::Value>>,
}

// ── Config ──

struct HttpSource {
    name: String,
    poll_discovery: HttpDiscovery,
    sse_url: String,
    poll_url: String,
    poll_interval: Duration,
    timeout_secs: u64,
    bearer_token: Option<String>,
}

fn build_http_sources(config: &PrismConfig) -> Vec<HttpSource> {
    let mut sources = Vec::new();

    for dc in config.all_discovery_configs() {
        if !dc.enabled {
            continue;
        }
        if let DiscoverySourceType::Http { http } = &dc.source_type {
            let base_url = http.url.trim_end_matches('/').to_string();
            // Derive SSE URL from base: /prism → /events (same host)
            let sse_url = if let Some(pos) = base_url.rfind('/') {
                format!("{}/events", &base_url[..pos])
            } else {
                format!("{}/events", base_url)
            };

            let hdc = HttpDiscoveryConfig {
                url: http.url.clone(),
                timeout_secs: http.timeout_secs,
                bearer_token: http.bearer_token.clone(),
                headers: Default::default(),
            };

            match HttpDiscovery::new(hdc) {
                Ok(disc) => {
                    sources.push(HttpSource {
                        name: dc.name.clone(),
                        poll_discovery: disc,
                        sse_url,
                        poll_url: http.url.clone(),
                        poll_interval: Duration::from_secs(http.interval_secs),
                        timeout_secs: http.timeout_secs,
                        bearer_token: http.bearer_token.clone(),
                    });
                }
                Err(e) => {
                    error!(name = %dc.name, "Failed to create HTTP discovery: {}", e);
                }
            }
        }
    }
    sources
}

/// Spawn the discovery poller/SSE consumer.
pub fn spawn_discovery_poller(
    state: AppState,
    config: &PrismConfig,
) -> Option<tokio::task::JoinHandle<()>> {
    let sources = build_http_sources(config);
    if sources.is_empty() {
        return None;
    }

    let source_names: Vec<String> = sources.iter().map(|s| s.name.clone()).collect();
    info!(
        "🔍 Discovery starting — {} source(s): [{}]",
        sources.len(),
        source_names.join(", "),
    );

    // Initial full poll
    full_poll_and_merge(&state, &sources);

    let handle = tokio::spawn(async move {
        // For each source, spawn an SSE listener task
        // On SSE event or disconnect→poll, trigger a rebuild via notify
        let rebuild_notify = Arc::new(Notify::new());

        for source in &sources {
            let sse_url = source.sse_url.clone();
            let poll_url = source.poll_url.clone();
            let name = source.name.clone();
            let interval = source.poll_interval;
            let timeout = source.timeout_secs;
            let bearer = source.bearer_token.clone();
            let notify = Arc::clone(&rebuild_notify);
            let state_clone = state.clone();

            tokio::spawn(async move {
                loop {
                    info!(source = %name, "Connecting SSE: {}", sse_url);
                    match connect_sse(&sse_url, bearer.as_deref(), timeout).await {
                        Ok(mut stream) => {
                            info!(source = %name, "SSE connected");
                            loop {
                                match read_sse_event(&mut stream).await {
                                    Ok(Some(ev)) => {
                                        match ev.event_type.as_str() {
                                            "full_sync" => {
                                                info!(source = %name, "SSE full_sync received");
                                                notify.notify_one();
                                            }
                                            "instance_add" | "instance_del" | "model_remove" => {
                                                info!(
                                                    source = %name,
                                                    event = %ev.event_type,
                                                    model = ?ev.model,
                                                    "SSE incremental event"
                                                );
                                                // Trigger full rebuild (incremental merge is
                                                // complex with atomic router swap — full rebuild
                                                // from sidecar snapshot is fast enough)
                                                notify.notify_one();
                                            }
                                            _ => {}
                                        }
                                    }
                                    Ok(None) => {
                                        // Keepalive comment, ignore
                                    }
                                    Err(e) => {
                                        warn!(source = %name, "SSE disconnected: {}", e);
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(source = %name, "SSE connect failed: {}, falling back to poll", e);
                        }
                    }

                    // Fallback: poll once, then retry SSE after interval
                    notify.notify_one();
                    tokio::time::sleep(interval).await;
                }
            });
        }

        // Rebuild loop — triggered by SSE events or poll fallback
        loop {
            rebuild_notify.notified().await;
            // Debounce: wait 100ms and drain additional notifications
            tokio::time::sleep(Duration::from_millis(100)).await;

            full_poll_and_merge(&state, &sources);
        }
    });

    Some(handle)
}

/// Full poll from all sources and rebuild router.
fn full_poll_and_merge(state: &AppState, sources: &[HttpSource]) {
    let start = std::time::Instant::now();

    let base_config = match load_config(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            warn!("Discovery: failed to reload config: {}", e);
            return;
        }
    };

    let mut registry = build_registry(&base_config);
    let mut total_variants = 0usize;
    let mut errors = 0usize;

    for source in sources {
        match source.poll_discovery.discover() {
            Ok(result) => {
                total_variants += result.variants.len();
                registry.apply_discovery(&result);
            }
            Err(e) => {
                errors += 1;
                error!(source = %source.name, "Poll failed: {}", e);
            }
        }
    }

    let new_router = build_router_from_config(registry, &base_config.routing);
    state.swap_router(new_router);

    info!(
        generation = state.generation(),
        variants = total_variants,
        errors = errors,
        elapsed_ms = start.elapsed().as_millis(),
        "Discovery rebuild complete"
    );
}

// ── SSE client ──

async fn connect_sse(
    url: &str,
    bearer: Option<&str>,
    timeout_secs: u64,
) -> Result<reqwest::Response, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(0)) // no timeout for SSE
        .connect_timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| format!("build client: {}", e))?;

    let mut req = client.get(url)
        .header("Accept", "text/event-stream")
        .header("Cache-Control", "no-cache");

    if let Some(token) = bearer {
        req = req.bearer_auth(token);
    }

    let resp = req.send().await.map_err(|e| format!("connect: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    Ok(resp)
}

async fn read_sse_event(resp: &mut reqwest::Response) -> Result<Option<SseDiscoveryEvent>, String> {
    let mut event_type = String::new();
    let mut data = String::new();

    // Read chunks and parse SSE format
    loop {
        let chunk = resp.chunk().await.map_err(|e| format!("read: {}", e))?;

        let chunk = match chunk {
            Some(c) => c,
            None => return Err("stream ended".to_string()),
        };

        let text = String::from_utf8_lossy(&chunk);

        for line in text.lines() {
            if line.starts_with("event: ") {
                event_type = line[7..].to_string();
            } else if line.starts_with("data: ") {
                data = line[6..].to_string();
            } else if line.starts_with(':') {
                // Comment (keepalive), ignore
                return Ok(None);
            } else if line.is_empty() && !data.is_empty() {
                // End of event
                let ev: SseDiscoveryEvent = serde_json::from_str(&data)
                    .map_err(|e| format!("parse event: {}", e))?;
                return Ok(Some(ev));
            }
        }

        // If we got data in this chunk, try to parse
        if !data.is_empty() && !event_type.is_empty() {
            if let Ok(ev) = serde_json::from_str::<SseDiscoveryEvent>(&data) {
                return Ok(Some(ev));
            }
        }
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::DiscoveryHttpConfig;

    #[test]
    fn test_build_http_sources_empty() {
        let config = PrismConfig::default();
        let sources = build_http_sources(&config);
        assert!(sources.is_empty());
    }

    #[test]
    fn test_build_http_sources_derives_sse_url() {
        let mut config = PrismConfig::default();
        config.discoveries.push(crate::config::types::DiscoveryConfig {
            name: "test".into(),
            source_type: DiscoverySourceType::Http {
                http: DiscoveryHttpConfig {
                    url: "http://localhost:9091/prism".into(),
                    interval_secs: 15,
                    timeout_secs: 5,
                    bearer_token: None,
                },
            },
            config: serde_json::Value::Null,
            enabled: true,
        });

        let sources = build_http_sources(&config);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].sse_url, "http://localhost:9091/events");
        assert_eq!(sources[0].poll_url, "http://localhost:9091/prism");
    }

    #[test]
    fn test_build_http_sources_disabled_skipped() {
        let mut config = PrismConfig::default();
        config.discoveries.push(crate::config::types::DiscoveryConfig {
            name: "off".into(),
            source_type: DiscoverySourceType::Http {
                http: DiscoveryHttpConfig {
                    url: "http://localhost:9091/prism".into(),
                    interval_secs: 10,
                    timeout_secs: 5,
                    bearer_token: None,
                },
            },
            config: serde_json::Value::Null,
            enabled: false,
        });
        assert!(build_http_sources(&config).is_empty());
    }

    #[test]
    fn test_sse_event_deserialize_full_sync() {
        let json = r#"{"type":"full_sync","timestamp":1709812345000,"snapshot":{}}"#;
        let ev: SseDiscoveryEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.event_type, "full_sync");
        assert!(ev.snapshot.is_some());
    }

    #[test]
    fn test_sse_event_deserialize_instance_add() {
        let json = r#"{"type":"instance_add","model":"llama-70b","instance":{"address":"10.0.1.5:8000","protocol":"http","pod_name":"llama-pod-0","labels":{"cluster":"c1"}},"timestamp":1709812345000}"#;
        let ev: SseDiscoveryEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.event_type, "instance_add");
        assert_eq!(ev.model, Some("llama-70b".to_string()));
        assert!(ev.instance.is_some());
        let inst = ev.instance.unwrap();
        assert_eq!(inst.address, "10.0.1.5:8000");
        assert_eq!(inst.pod_name, Some("llama-pod-0".to_string()));
    }

    #[test]
    fn test_sse_event_deserialize_instance_del() {
        let json = r#"{"type":"instance_del","model":"llama-70b","instance":{"address":"10.0.1.5:8000","protocol":"http"},"timestamp":1709812345000}"#;
        let ev: SseDiscoveryEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.event_type, "instance_del");
    }

    #[test]
    fn test_sse_event_deserialize_model_remove() {
        let json = r#"{"type":"model_remove","model":"llama-70b","timestamp":1709812345000}"#;
        let ev: SseDiscoveryEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.event_type, "model_remove");
        assert_eq!(ev.model, Some("llama-70b".to_string()));
    }
}
