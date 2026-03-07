//! Discovery Runner — loads WASM discovery plugins and runs them periodically.
//!
//! Replaces the old HTTP-based discovery_poller with a unified WASM plugin approach.
//! All discovery sources go through the Discovery trait (WASM or static).

use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info, warn};

use prism_core::discovery::{Discovery, DiscoveryResult};

#[cfg(feature = "wasm")]
use prism_core::discovery::WasmDiscovery;

use crate::config::{build_registry, build_router_from_config, load_config};
use crate::config::types::{DiscoverySourceType, PrismConfig};
use crate::state::AppState;

/// Default interval for running discovery plugins.
const DEFAULT_INTERVAL_SECS: u64 = 15;

/// Build discovery plugins from config.
fn load_discovery_plugins(config: &PrismConfig) -> Vec<Box<dyn Discovery>> {
    let mut plugins: Vec<Box<dyn Discovery>> = Vec::new();

    for dc in config.all_discovery_configs() {
        if !dc.enabled {
            continue;
        }

        match &dc.source_type {
            #[cfg(feature = "wasm")]
            DiscoverySourceType::Wasm { wasm } => {
                match WasmDiscovery::from_file(&dc.name, &wasm.path) {
                    Ok(mut discovery) => {
                        // Pass plugin config as JSON string
                        if !dc.config.is_null() {
                            let config_str = dc.config.to_string();
                            discovery = discovery.with_config(config_str);
                        }
                        info!(name = %dc.name, path = %wasm.path, "Loaded WASM discovery plugin");
                        plugins.push(Box::new(discovery));
                    }
                    Err(e) => {
                        error!(name = %dc.name, path = %wasm.path, "Failed to load WASM discovery: {}", e);
                    }
                }
            }
            #[cfg(not(feature = "wasm"))]
            DiscoverySourceType::Wasm { wasm } => {
                warn!(
                    name = %dc.name,
                    path = %wasm.path,
                    "WASM discovery plugin configured but 'wasm' feature is not enabled — skipping"
                );
            }
            DiscoverySourceType::Static(_) => {
                // Static discovery is handled by build_registry directly
            }
        }
    }

    plugins
}

/// Run all discovery plugins once and merge results into the registry.
/// Returns the merged DiscoveryResult for logging.
fn run_discovery_cycle(
    state: &AppState,
    plugins: &[Box<dyn Discovery>],
) -> (usize, usize, usize) {
    let base_config = match load_config(&state.config_path) {
        Ok(c) => c,
        Err(e) => {
            warn!("Discovery runner: failed to reload config: {}", e);
            return (0, 0, 0);
        }
    };

    let mut registry = build_registry(&base_config);

    let mut total_models = 0usize;
    let mut total_variants = 0usize;
    let mut errors = 0usize;

    for plugin in plugins {
        match plugin.discover() {
            Ok(result) => {
                total_models += result.models.len();
                total_variants += result.variants.len();
                info!(
                    source = %plugin.name(),
                    models = result.models.len(),
                    variants = result.variants.len(),
                    "Discovery OK"
                );
                registry.apply_discovery(&result);
            }
            Err(e) => {
                errors += 1;
                error!(source = %plugin.name(), "Discovery failed: {}", e);
            }
        }
    }

    let new_router = build_router_from_config(registry, &base_config.routing);
    state.swap_router(new_router);

    (total_models, total_variants, errors)
}

/// Spawn the discovery runner background task.
///
/// Returns `None` if no WASM discovery plugins are configured.
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

    let interval = Duration::from_secs(interval_secs.unwrap_or(DEFAULT_INTERVAL_SECS));
    let plugin_names: Vec<&str> = plugins.iter().map(|p| p.name()).collect();

    info!(
        "🔍 Discovery runner starting — {} plugin(s): [{}], interval: {:?}",
        plugins.len(),
        plugin_names.join(", "),
        interval,
    );

    // Initial discovery run
    let (models, variants, errors) = run_discovery_cycle(&state, &plugins);
    info!(
        "Initial discovery: {} models, {} variants, {} errors",
        models, variants, errors
    );

    let plugins = Arc::new(plugins);

    let handle = tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        tick.tick().await; // skip first (already did initial run)

        loop {
            tick.tick().await;

            // Run discovery in a blocking task (WASM execution is synchronous)
            let state_clone = state.clone();
            let plugins_clone = Arc::clone(&plugins);
            let result = tokio::task::spawn_blocking(move || {
                run_discovery_cycle(&state_clone, &plugins_clone)
            }).await;

            match result {
                Ok((models, variants, errors)) => {
                    info!(
                        generation = state.generation(),
                        models = models,
                        variants = variants,
                        errors = errors,
                        "Discovery cycle complete"
                    );
                }
                Err(e) => {
                    error!("Discovery task panicked: {}", e);
                }
            }
        }
    });

    Some(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_discovery_plugins_empty_config() {
        let config = PrismConfig::default();
        let plugins = load_discovery_plugins(&config);
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_load_discovery_plugins_static_only() {
        let mut config = PrismConfig::default();
        config.discoveries.push(crate::config::types::DiscoveryConfig {
            name: "static".into(),
            source_type: DiscoverySourceType::Static("static".into()),
            config: serde_json::Value::Null,
            enabled: true,
        });
        let plugins = load_discovery_plugins(&config);
        assert!(plugins.is_empty()); // Static is handled by build_registry
    }

    #[cfg(not(feature = "wasm"))]
    #[test]
    fn test_load_discovery_plugins_wasm_without_feature() {
        let mut config = PrismConfig::default();
        config.discoveries.push(crate::config::types::DiscoveryConfig {
            name: "k8s".into(),
            source_type: DiscoverySourceType::Wasm {
                wasm: crate::config::types::DiscoveryWasmConfig {
                    path: "./nonexistent.wasm".into(),
                },
            },
            config: serde_json::Value::Null,
            enabled: true,
        });
        let plugins = load_discovery_plugins(&config);
        assert!(plugins.is_empty()); // No wasm feature
    }

    #[test]
    fn test_load_discovery_plugins_disabled_skipped() {
        let mut config = PrismConfig::default();
        config.discoveries.push(crate::config::types::DiscoveryConfig {
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
