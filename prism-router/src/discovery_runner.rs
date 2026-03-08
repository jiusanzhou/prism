#![allow(dead_code)]
//! Discovery Runner — loads WASM discovery plugins and runs them.
//!
//! Two modes:
//! - **Streaming** (preferred): plugin exports `prism_discovery_run()`, runs in its
//!   own thread, pushes events via `prism_discovery_push_event()`. Real-time.
//! - **Polling** (fallback): plugin exports `prism_discovery_discover()`, host calls
//!   it periodically. For backward compat with simple plugins.

use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info, warn};

use prism_core::discovery::Discovery;

#[cfg(feature = "wasm")]
use prism_core::discovery::WasmDiscovery;

use crate::config::types::{DiscoverySourceType, PrismConfig};
use crate::config::{build_registry, build_router_from_config, load_config};
use crate::state::AppState;

const DEFAULT_POLL_INTERVAL_SECS: u64 = 15;

/// A loaded discovery plugin — either WASM (with streaming support) or generic.
enum DiscoveryPlugin {
    #[cfg(feature = "wasm")]
    Wasm(WasmDiscovery),
    Generic(Box<dyn Discovery>),
}

/// Build discovery plugins from config.
fn load_discovery_plugins(config: &PrismConfig) -> Vec<(String, DiscoveryPlugin)> {
    let plugins = Vec::new();

    for dc in config.all_discovery_configs() {
        if !dc.enabled {
            continue;
        }

        match &dc.source_type {
            #[cfg(feature = "wasm")]
            DiscoverySourceType::Wasm { wasm } => {
                match WasmDiscovery::from_file(&dc.name, &wasm.path) {
                    Ok(mut discovery) => {
                        if !dc.config.is_null() {
                            discovery = discovery.with_config(dc.config.to_string());
                        }
                        info!(name = %dc.name, path = %wasm.path, "Loaded WASM discovery plugin");
                        plugins.push((dc.name.clone(), DiscoveryPlugin::Wasm(discovery)));
                    }
                    Err(e) => {
                        error!(name = %dc.name, path = %wasm.path, "Failed to load WASM discovery: {}", e);
                    }
                }
            }
            #[cfg(not(feature = "wasm"))]
            DiscoverySourceType::Wasm { wasm } => {
                warn!(name = %dc.name, path = %wasm.path, "WASM feature not enabled — skipping");
            }
            DiscoverySourceType::Static(_) => {}
        }
    }

    plugins
}

/// Rebuild router from base config + discovery result, then swap.
fn rebuild_and_swap(state: &AppState, event: &prism_core::discovery::DiscoveryEvent) {
    let base_config = match load_config(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            warn!("Discovery runner: config reload failed: {}", e);
            return;
        }
    };

    let mut registry = build_registry(&base_config);
    registry.apply_event(event);

    let new_router = build_router_from_config(registry, &base_config.routing);
    state.swap_router(new_router);
}

/// Spawn the discovery runner.
///
/// For each plugin:
/// - If it supports streaming (`prism_discovery_run`): spawn a thread, consume events.
/// - Otherwise: poll `discover()` periodically.
pub fn spawn_discovery_runner(
    state: AppState,
    config: &PrismConfig,
    interval_secs: Option<u64>,
) -> Option<tokio::task::JoinHandle<()>> {
    let plugins = load_discovery_plugins(config);

    if plugins.is_empty() {
        info!("No discovery plugins configured — static config only");
        return None;
    }

    let interval = Duration::from_secs(interval_secs.unwrap_or(DEFAULT_POLL_INTERVAL_SECS));
    let plugin_names: Vec<&str> = plugins.iter().map(|(n, _)| n.as_str()).collect();
    info!(
        "🔍 Discovery runner: {} plugin(s): [{}]",
        plugins.len(),
        plugin_names.join(", "),
    );

    let handle = tokio::spawn(async move {
        let mut poll_plugins: Vec<Box<dyn Discovery>> = Vec::new();

        for (name, plugin) in plugins {
            match plugin {
                #[cfg(feature = "wasm")]
                DiscoveryPlugin::Wasm(wasm_disc) => {
                    if wasm_disc.supports_streaming() {
                        let state_clone = state.clone();
                        match wasm_disc.run_streaming() {
                            Ok(rx) => {
                                info!(plugin = %name, "Started in streaming mode (real-time)");
                                let consumer_name = name.clone();
                                tokio::task::spawn_blocking(move || {
                                    for event in rx {
                                        log_event(&consumer_name, &event);
                                        rebuild_and_swap(&state_clone, &event);
                                    }
                                    warn!(plugin = %consumer_name, "Event channel closed");
                                });
                                continue;
                            }
                            Err(e) => {
                                warn!(plugin = %name, "Streaming failed, falling back to poll: {}", e);
                                poll_plugins.push(Box::new(wasm_disc));
                            }
                        }
                    } else {
                        info!(plugin = %name, interval = ?interval, "Using poll mode (no prism_discovery_run export)");
                        poll_plugins.push(Box::new(wasm_disc));
                    }
                }
                DiscoveryPlugin::Generic(p) => {
                    info!(plugin = %name, interval = ?interval, "Using poll mode");
                    poll_plugins.push(p);
                }
            }
        }

        // Poll loop for non-streaming plugins
        if !poll_plugins.is_empty() {
            let poll_plugins = Arc::new(poll_plugins);
            let mut tick = tokio::time::interval(interval);

            // Initial poll
            {
                let state_clone = state.clone();
                let plugins_clone = Arc::clone(&poll_plugins);
                let _ = tokio::task::spawn_blocking(move || {
                    poll_once(&state_clone, &plugins_clone);
                })
                .await;
            }

            loop {
                tick.tick().await;
                let state_clone = state.clone();
                let plugins_clone = Arc::clone(&poll_plugins);
                let _ = tokio::task::spawn_blocking(move || {
                    poll_once(&state_clone, &plugins_clone);
                })
                .await;
            }
        } else {
            // All plugins are streaming — keep task alive
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }
    });

    Some(handle)
}

fn log_event(plugin: &str, event: &prism_core::discovery::DiscoveryEvent) {
    use prism_core::discovery::DiscoveryEvent;
    let desc = match event {
        DiscoveryEvent::FullSync(r) => format!(
            "full_sync ({} models, {} variants)",
            r.models.len(),
            r.variants.len()
        ),
        DiscoveryEvent::IncrementalUpsert(r) => format!("upsert ({} variants)", r.variants.len()),
        DiscoveryEvent::Remove { variant_ids, .. } => {
            format!("remove ({} variants)", variant_ids.len())
        }
        DiscoveryEvent::VariantUpsert(v) => format!("variant_upsert({})", v.id),
        DiscoveryEvent::VariantRemove(id) => format!("variant_remove({})", id),
    };
    info!(plugin = %plugin, "{}", desc);
}

/// Run one poll cycle for non-streaming plugins.
fn poll_once(state: &AppState, plugins: &[Box<dyn Discovery>]) {
    let base_config = match load_config(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            warn!("Poll: config reload failed: {}", e);
            return;
        }
    };

    let mut registry = build_registry(&base_config);
    let mut total_v = 0;

    for plugin in plugins {
        match plugin.discover() {
            Ok(result) => {
                total_v += result.variants.len();
                registry.apply_discovery(&result);
            }
            Err(e) => {
                error!(source = %plugin.name(), "Poll discover failed: {}", e);
            }
        }
    }

    let new_router = build_router_from_config(registry, &base_config.routing);
    state.swap_router(new_router);

    info!(
        generation = state.generation(),
        variants = total_v,
        "Poll cycle done"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_empty_config() {
        let config = PrismConfig::default();
        let plugins = load_discovery_plugins(&config);
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_disabled_plugin_skipped() {
        let mut config = PrismConfig::default();
        config
            .discoveries
            .push(crate::config::types::DiscoveryConfig {
                name: "disabled".into(),
                source_type: DiscoverySourceType::Wasm {
                    wasm: crate::config::types::DiscoveryWasmConfig {
                        path: "./test.wasm".into(),
                    },
                },
                config: serde_json::Value::Null,
                enabled: false,
            });
        let plugins = load_discovery_plugins(&config);
        assert!(plugins.is_empty());
    }
}
