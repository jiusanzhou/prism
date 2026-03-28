#![allow(clippy::type_complexity)]
use prism_core::inflight::InFlightTracker;
use prism_core::prefix_cache::PrefixCacheIndex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use arc_swap::ArcSwap;

use prism_core::data::DataLayer;
use prism_core::router::PrismRouter;

use crate::metrics::MetricsCollector;

/// Shared application state for HTTP, gRPC, and proxy servers.
#[derive(Clone)]
pub struct AppState {
    /// Router — atomically swappable for hot-reload.
    pub router: Arc<ArcSwap<PrismRouter>>,
    pub start_time: std::time::Instant,
    pub metrics: Arc<MetricsCollector>,
    /// Path to the configuration file (for reload).
    pub config_path: Arc<PathBuf>,
    /// Generation counter — incremented on each reload.
    pub config_generation: Arc<AtomicU64>,
    /// In-flight request tracker for load-aware scheduling.
    pub inflight: Arc<InFlightTracker>,
    /// Prefix cache index for cache-aware routing.
    pub prefix_cache: Arc<dyn PrefixCacheIndex>,
    /// Per-provider discovery state: provider_id → models→instances.
    /// Written by individual sidecar consumers, read during registry rebuild.
    pub discovery_states: Arc<RwLock<HashMap<String, HashMap<String, Vec<serde_json::Value>>>>>,
    /// Model alias / fallback mapping: client name → internal model id.
    /// Atomically swappable for hot-reload.
    pub model_aliases: Arc<ArcSwap<HashMap<String, String>>>,
    /// Usage backend for persistence (requires `usage-db` feature).
    #[cfg(feature = "usage-db")]
    pub usage_store: Option<Arc<dyn prism_core::usage::UsageBackend>>,
}

impl AppState {
    pub fn new(router: PrismRouter, config_path: PathBuf) -> Self {
        Self {
            router: Arc::new(ArcSwap::new(Arc::new(router))),
            start_time: std::time::Instant::now(),
            metrics: Arc::new(MetricsCollector::new(1000)),
            config_path: Arc::new(config_path),
            config_generation: Arc::new(AtomicU64::new(0)),
            inflight: Arc::new(InFlightTracker::new()),
            prefix_cache: prism_core::prefix_cache::default_prefix_cache(),
            discovery_states: Arc::new(RwLock::new(HashMap::new())),
            model_aliases: Arc::new(ArcSwap::new(Arc::new(HashMap::new()))),
            #[cfg(feature = "usage-db")]
            usage_store: None,
        }
    }

    /// Create with model aliases pre-populated.
    pub fn with_model_aliases(self, aliases: HashMap<String, String>) -> Self {
        self.model_aliases.store(Arc::new(aliases));
        self
    }

    /// Resolve a model name through alias mapping.
    /// Returns the resolved model name (or the original if no alias matches).
    pub fn resolve_model_alias(&self, model: &str) -> String {
        let aliases = self.model_aliases.load();
        if let Some(resolved) = aliases.get(model) {
            resolved.clone()
        } else if let Some(wildcard) = aliases.get("*") {
            wildcard.clone()
        } else {
            model.to_string()
        }
    }

    /// Atomically swap model aliases (used during hot-reload).
    pub fn swap_model_aliases(&self, aliases: HashMap<String, String>) {
        self.model_aliases.store(Arc::new(aliases));
    }

    /// Get the current router (cheap load from ArcSwap).
    pub fn router(&self) -> arc_swap::Guard<Arc<PrismRouter>> {
        self.router.load()
    }

    /// Swap the router atomically.
    pub fn swap_router(&self, new_router: PrismRouter) {
        self.router.store(Arc::new(new_router));
        self.config_generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Get the current config generation.
    pub fn generation(&self) -> u64 {
        self.config_generation.load(Ordering::Relaxed)
    }

    /// Get a reference to the router's data layer.
    pub fn data_layer(&self) -> Arc<DataLayer> {
        let router = self.router.load();
        Arc::clone(router.registry.data_layer())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::registry::InMemoryRegistry;

    #[test]
    fn test_app_state_new_initial_generation() {
        let router = PrismRouter::new(InMemoryRegistry::new());
        let state = AppState::new(router, PathBuf::from("test.yaml"));
        assert_eq!(state.generation(), 0);
    }

    #[test]
    fn test_swap_router_increments_generation() {
        let router = PrismRouter::new(InMemoryRegistry::new());
        let state = AppState::new(router, PathBuf::from("test.yaml"));
        assert_eq!(state.generation(), 0);

        state.swap_router(PrismRouter::new(InMemoryRegistry::new()));
        assert_eq!(state.generation(), 1);

        state.swap_router(PrismRouter::new(InMemoryRegistry::new()));
        assert_eq!(state.generation(), 2);
    }

    #[test]
    fn test_router_returns_current() {
        let registry = InMemoryRegistry::new();
        let router = PrismRouter::new(registry);
        let state = AppState::new(router, PathBuf::from("test.yaml"));

        let guard = state.router();
        assert_eq!(guard.registry.models.len(), 0);
    }

    #[test]
    fn test_config_path_stored() {
        let router = PrismRouter::new(InMemoryRegistry::new());
        let state = AppState::new(router, PathBuf::from("/some/path.yaml"));
        assert_eq!(*state.config_path, PathBuf::from("/some/path.yaml"));
    }

    #[test]
    fn test_resolve_model_alias_exact_match() {
        let router = PrismRouter::new(InMemoryRegistry::new());
        let mut aliases = HashMap::new();
        aliases.insert("gpt-4".to_string(), "qwen3-72b".to_string());
        aliases.insert("claude-sonnet".to_string(), "deepseek-r1".to_string());
        aliases.insert("*".to_string(), "qwen3-0.6b".to_string());
        let state = AppState::new(router, PathBuf::from("test.yaml")).with_model_aliases(aliases);

        assert_eq!(state.resolve_model_alias("gpt-4"), "qwen3-72b");
        assert_eq!(state.resolve_model_alias("claude-sonnet"), "deepseek-r1");
    }

    #[test]
    fn test_resolve_model_alias_wildcard_fallback() {
        let router = PrismRouter::new(InMemoryRegistry::new());
        let mut aliases = HashMap::new();
        aliases.insert("gpt-4".to_string(), "qwen3-72b".to_string());
        aliases.insert("*".to_string(), "qwen3-0.6b".to_string());
        let state = AppState::new(router, PathBuf::from("test.yaml")).with_model_aliases(aliases);

        assert_eq!(state.resolve_model_alias("unknown-model"), "qwen3-0.6b");
    }

    #[test]
    fn test_resolve_model_alias_no_match_passthrough() {
        let router = PrismRouter::new(InMemoryRegistry::new());
        let mut aliases = HashMap::new();
        aliases.insert("gpt-4".to_string(), "qwen3-72b".to_string());
        let state = AppState::new(router, PathBuf::from("test.yaml")).with_model_aliases(aliases);

        assert_eq!(state.resolve_model_alias("unknown-model"), "unknown-model");
    }

    #[test]
    fn test_resolve_model_alias_empty_aliases() {
        let router = PrismRouter::new(InMemoryRegistry::new());
        let state = AppState::new(router, PathBuf::from("test.yaml"));

        assert_eq!(state.resolve_model_alias("gpt-4"), "gpt-4");
    }

    #[test]
    fn test_swap_model_aliases() {
        let router = PrismRouter::new(InMemoryRegistry::new());
        let state = AppState::new(router, PathBuf::from("test.yaml"));

        assert_eq!(state.resolve_model_alias("gpt-4"), "gpt-4");

        let mut aliases = HashMap::new();
        aliases.insert("gpt-4".to_string(), "qwen3-72b".to_string());
        state.swap_model_aliases(aliases);

        assert_eq!(state.resolve_model_alias("gpt-4"), "qwen3-72b");
    }
}
