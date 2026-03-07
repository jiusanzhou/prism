//! HTTP Discovery Poller — background task that periodically polls HTTP discovery
//! sources and merges results into the live router registry.
//!
//! On each tick:
//! 1. Poll all configured HTTP discovery endpoints
//! 2. Reload static config from file
//! 3. Merge HTTP discovery results into the registry
//! 4. Atomic swap the router

use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info, warn};

use prism_core::discovery::Discovery;

use crate::config::{build_registry, build_router_from_config, load_config};
use crate::config::types::{DiscoverySourceType, PrismConfig};
use crate::http_discovery::{HttpDiscovery, HttpDiscoveryConfig};
use crate::state::AppState;

/// Configuration for the discovery poller.
pub struct DiscoveryPollerConfig {
    /// Polling interval (default: 30s).
    pub interval: Duration,
}

impl Default for DiscoveryPollerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
        }
    }
}

/// Extract HTTP discovery sources from a PrismConfig.
fn build_http_discoveries(config: &PrismConfig) -> Vec<(String, HttpDiscovery, Duration)> {
    let mut sources = Vec::new();

    for dc in config.all_discovery_configs() {
        if !dc.enabled {
            continue;
        }

        if let DiscoverySourceType::Http { http } = &dc.source_type {
            let hdc = HttpDiscoveryConfig {
                url: http.url.clone(),
                timeout_secs: http.timeout_secs,
                bearer_token: http.bearer_token.clone(),
                headers: Default::default(),
            };

            match HttpDiscovery::new(hdc) {
                Ok(disc) => {
                    let interval = Duration::from_secs(http.interval_secs);
                    sources.push((dc.name.clone(), disc, interval));
                }
                Err(e) => {
                    error!(name = %dc.name, "Failed to create HTTP discovery: {}", e);
                }
            }
        }
    }

    sources
}

/// Spawn the discovery poller background task.
///
/// Returns `None` if there are no HTTP discovery sources configured.
pub fn spawn_discovery_poller(
    state: AppState,
    config: &PrismConfig,
) -> Option<tokio::task::JoinHandle<()>> {
    let sources = build_http_discoveries(config);

    if sources.is_empty() {
        return None;
    }

    // Use the minimum interval across all sources
    let min_interval = sources
        .iter()
        .map(|(_, _, interval)| *interval)
        .min()
        .unwrap_or(Duration::from_secs(30));

    let source_names: Vec<String> = sources.iter().map(|(n, _, _)| n.clone()).collect();
    info!(
        "🔍 Discovery poller starting — {} HTTP source(s): [{}], interval: {:?}",
        sources.len(),
        source_names.join(", "),
        min_interval,
    );

    // Initial poll
    poll_and_merge(&state, &sources);

    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(min_interval);
        interval.tick().await; // skip first immediate tick (already did initial poll)

        loop {
            interval.tick().await;
            poll_and_merge(&state, &sources);
        }
    });

    Some(handle)
}

/// Execute one poll cycle: discover from all HTTP sources and merge into the router.
fn poll_and_merge(state: &AppState, sources: &[(String, HttpDiscovery, Duration)]) {
    let start = std::time::Instant::now();

    // 1. Reload base config from file
    let base_config = match load_config(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            warn!("Discovery poller: failed to reload config: {}", e);
            return;
        }
    };

    // 2. Build base registry from static config
    let mut registry = build_registry(&base_config);

    // 3. Poll each HTTP discovery source and merge
    let mut total_variants = 0usize;
    let mut total_models = 0usize;
    let mut errors = 0usize;

    for (name, disc, _) in sources {
        match disc.discover() {
            Ok(result) => {
                let v_count = result.variants.len();
                let m_count = result.models.len();
                registry.apply_discovery(&result);
                total_variants += v_count;
                total_models += m_count;
                info!(
                    source = %name,
                    models = m_count,
                    variants = v_count,
                    "Discovery poll OK"
                );
            }
            Err(e) => {
                errors += 1;
                error!(source = %name, "Discovery poll failed: {}", e);
            }
        }
    }

    // 4. Build new router and swap
    let new_router = build_router_from_config(registry, &base_config.routing);
    let prev_gen = state.generation();
    state.swap_router(new_router);

    let elapsed = start.elapsed();
    info!(
        generation = state.generation(),
        prev_generation = prev_gen,
        discovered_models = total_models,
        discovered_variants = total_variants,
        errors = errors,
        elapsed_ms = elapsed.as_millis(),
        "Discovery poll cycle complete"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::DiscoveryHttpConfig;

    #[test]
    fn test_build_http_discoveries_empty_config() {
        let config = PrismConfig::default();
        let sources = build_http_discoveries(&config);
        assert!(sources.is_empty());
    }

    #[test]
    fn test_build_http_discoveries_with_http_source() {
        let mut config = PrismConfig::default();
        config.discoveries.push(crate::config::types::DiscoveryConfig {
            name: "test-k8s".into(),
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

        let sources = build_http_discoveries(&config);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].0, "test-k8s");
        assert_eq!(sources[0].2, Duration::from_secs(15));
    }

    #[test]
    fn test_build_http_discoveries_disabled_skipped() {
        let mut config = PrismConfig::default();
        config.discoveries.push(crate::config::types::DiscoveryConfig {
            name: "disabled".into(),
            source_type: DiscoverySourceType::Http {
                http: DiscoveryHttpConfig {
                    url: "http://localhost:9091/prism".into(),
                    interval_secs: 15,
                    timeout_secs: 5,
                    bearer_token: None,
                },
            },
            config: serde_json::Value::Null,
            enabled: false,
        });

        let sources = build_http_discoveries(&config);
        assert!(sources.is_empty());
    }

    #[test]
    fn test_build_http_discoveries_wasm_source_ignored() {
        let mut config = PrismConfig::default();
        config.discoveries.push(crate::config::types::DiscoveryConfig {
            name: "wasm-disc".into(),
            source_type: DiscoverySourceType::Wasm {
                wasm: crate::config::types::DiscoveryWasmConfig {
                    path: "./plugin.wasm".into(),
                },
            },
            config: serde_json::Value::Null,
            enabled: true,
        });

        let sources = build_http_discoveries(&config);
        assert!(sources.is_empty());
    }
}
