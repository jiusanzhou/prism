//! Configuration watcher — monitors config sources and triggers hot-reload.
//!
//! Supports:
//! - File system watching via `notify` crate
//! - Debounced reload to avoid rapid reloads from multiple file events
//! - Atomic router swap (all-or-nothing reload)

use std::path::PathBuf;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::config::{build_registry, load_config};
use crate::state::AppState;

/// Configuration for the watcher.
pub struct WatcherConfig {
    /// Config file path to watch.
    pub config_path: PathBuf,
    /// Debounce duration (avoid rapid reloads from multiple file events).
    pub debounce: Duration,
    /// Whether to reload on startup.
    pub reload_on_start: bool,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            config_path: PathBuf::from("configs/prism.yaml"),
            debounce: Duration::from_secs(2),
            reload_on_start: false,
        }
    }
}

/// Reload result.
#[derive(Debug)]
pub struct ReloadResult {
    pub generation: u64,
    pub providers_count: usize,
    pub models_count: usize,
    pub variants_count: usize,
    pub duration: Duration,
}

/// Start watching a config file and hot-reload on changes.
///
/// Returns a JoinHandle for the background task.
pub fn spawn_config_watcher(
    state: AppState,
    watcher_config: WatcherConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::channel::<()>(10);

        // Set up file watcher
        let tx_clone = tx.clone();
        let config_path = watcher_config.config_path.clone();

        // Watch the parent directory (notify sometimes misses direct file watches)
        let watch_dir = config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        let config_filename = config_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut watcher = match notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| match res {
                Ok(event) => {
                    // Only trigger on modify/create events for our config file
                    match event.kind {
                        EventKind::Modify(_) | EventKind::Create(_) => {
                            let is_our_file = event.paths.iter().any(|p| {
                                p.file_name()
                                    .map(|n| n.to_string_lossy() == config_filename)
                                    .unwrap_or(false)
                            });
                            if is_our_file {
                                let _ = tx_clone.try_send(());
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    warn!("File watcher error: {}", e);
                }
            },
        ) {
            Ok(w) => w,
            Err(e) => {
                error!("Failed to create file watcher: {}", e);
                return;
            }
        };

        if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
            error!("Failed to watch directory {:?}: {}", watch_dir, e);
            return;
        }

        info!("🔄 Config watcher started — watching {:?}", config_path);

        // Debounced reload loop
        loop {
            // Wait for a change notification
            if rx.recv().await.is_none() {
                break; // Channel closed
            }

            // Debounce: drain any additional events within the window
            tokio::time::sleep(watcher_config.debounce).await;
            while rx.try_recv().is_ok() {} // drain

            // Reload
            info!("📋 Config change detected, reloading...");
            match reload_config(&state, &watcher_config.config_path) {
                Ok(result) => {
                    info!(
                        "✅ Config reloaded (gen={}) — {} providers, {} models, {} variants [{:?}]",
                        result.generation,
                        result.providers_count,
                        result.models_count,
                        result.variants_count,
                        result.duration,
                    );
                }
                Err(e) => {
                    error!("❌ Config reload failed: {} — keeping previous config", e);
                }
            }
        }
    })
}

/// Reload configuration and swap the router atomically.
///
/// This is also callable directly (e.g., from an admin API endpoint).
pub fn reload_config(
    state: &AppState,
    config_path: &std::path::Path,
) -> Result<ReloadResult, String> {
    let start = std::time::Instant::now();

    // 1. Load new config
    let new_config = load_config(config_path)?;

    // 2. Build new registry
    let new_registry = build_registry(&new_config);

    // 3. Build new router
    let new_router = prism_core::router::PrismRouter::new(new_registry);

    // 4. Gather stats before swap
    let providers_count = new_router.registry.providers.len();
    let models_count = new_router.registry.models.len();
    let variants_count = new_router.registry.variants.len();

    // 5. Atomic swap
    state.swap_router(new_router);

    let duration = start.elapsed();
    let generation = state.generation();

    Ok(ReloadResult {
        generation,
        providers_count,
        models_count,
        variants_count,
        duration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::registry::InMemoryRegistry;
    use prism_core::router::PrismRouter;
    use tempfile::TempDir;

    fn minimal_config_yaml() -> &'static str {
        r#"
providers:
  openai:
    base_url: https://api.openai.com/v1
    protocols: [openai]
models:
  - id: gpt-4o
    display_name: GPT-4o
    quality_tier: S
variants:
  - id: gpt-4o@openai
    model_id: gpt-4o
    provider_id: openai
pools:
  - id: default
    name: Default Pool
    variants: [gpt-4o@openai]
"#
    }

    #[test]
    fn test_reload_config_success() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("prism.yaml");
        std::fs::write(&config_path, minimal_config_yaml()).unwrap();

        let router = PrismRouter::new(InMemoryRegistry::new());
        let state = AppState::new(router, config_path.clone());

        assert_eq!(state.generation(), 0);
        assert_eq!(state.router().registry.models.len(), 0);

        let result = reload_config(&state, &config_path).unwrap();
        assert_eq!(result.generation, 1);
        assert_eq!(result.providers_count, 1);
        assert_eq!(result.models_count, 1);
        assert_eq!(result.variants_count, 1);
        assert!(result.duration.as_millis() < 5000);

        // Verify the router was actually swapped
        assert_eq!(state.router().registry.models.len(), 1);
        assert_eq!(state.generation(), 1);
    }

    #[test]
    fn test_reload_config_invalid_keeps_previous() {
        let tmp = TempDir::new().unwrap();
        let good_path = tmp.path().join("good.yaml");
        let bad_path = tmp.path().join("bad.yaml");

        std::fs::write(&good_path, minimal_config_yaml()).unwrap();
        std::fs::write(&bad_path, "this is not valid yaml: [[[").unwrap();

        let router = PrismRouter::new(InMemoryRegistry::new());
        let state = AppState::new(router, good_path.clone());

        // First load the good config
        reload_config(&state, &good_path).unwrap();
        assert_eq!(state.generation(), 1);
        assert_eq!(state.router().registry.models.len(), 1);

        // Try loading bad config — should fail
        let result = reload_config(&state, &bad_path);
        assert!(result.is_err());

        // State should be unchanged
        assert_eq!(state.generation(), 1);
        assert_eq!(state.router().registry.models.len(), 1);
    }

    #[test]
    fn test_reload_config_missing_file() {
        let router = PrismRouter::new(InMemoryRegistry::new());
        let state = AppState::new(router, PathBuf::from("nonexistent.yaml"));

        let result = reload_config(&state, std::path::Path::new("nonexistent.yaml"));
        assert!(result.is_err());
        assert_eq!(state.generation(), 0);
    }

    #[test]
    fn test_watcher_config_default() {
        let config = WatcherConfig::default();
        assert_eq!(config.config_path, PathBuf::from("configs/prism.yaml"));
        assert_eq!(config.debounce, Duration::from_secs(2));
        assert!(!config.reload_on_start);
    }

    #[test]
    fn test_generation_increments_on_multiple_reloads() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("prism.yaml");
        std::fs::write(&config_path, minimal_config_yaml()).unwrap();

        let router = PrismRouter::new(InMemoryRegistry::new());
        let state = AppState::new(router, config_path.clone());

        for i in 1..=5 {
            reload_config(&state, &config_path).unwrap();
            assert_eq!(state.generation(), i);
        }
    }
}
