use std::sync::Arc;

use prism_core::data::DataLayer;
use prism_core::router::PrismRouter;

use crate::metrics::MetricsCollector;

/// Shared application state for HTTP, gRPC, and proxy servers.
#[derive(Clone)]
pub struct AppState {
    pub router: Arc<PrismRouter>,
    pub start_time: std::time::Instant,
    pub metrics: Arc<MetricsCollector>,
}

impl AppState {
    pub fn new(router: PrismRouter) -> Self {
        Self {
            router: Arc::new(router),
            start_time: std::time::Instant::now(),
            metrics: Arc::new(MetricsCollector::new(1000)), // 1000 records per variant
        }
    }

    /// Get a reference to the router's data layer.
    pub fn data_layer(&self) -> &Arc<DataLayer> {
        self.router.registry.data_layer()
    }
}
