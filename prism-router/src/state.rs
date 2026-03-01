use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
}

impl AppState {
    pub fn new(router: PrismRouter, config_path: PathBuf) -> Self {
        Self {
            router: Arc::new(ArcSwap::new(Arc::new(router))),
            start_time: std::time::Instant::now(),
            metrics: Arc::new(MetricsCollector::new(1000)),
            config_path: Arc::new(config_path),
            config_generation: Arc::new(AtomicU64::new(0)),
        }
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
}
