use std::sync::Arc;

use prism_core::router::PrismRouter;

/// Shared application state for the Axum server.
#[derive(Clone)]
pub struct AppState {
    pub router: Arc<PrismRouter>,
    pub start_time: std::time::Instant,
}

impl AppState {
    pub fn new(router: PrismRouter) -> Self {
        Self {
            router: Arc::new(router),
            start_time: std::time::Instant::now(),
        }
    }
}
