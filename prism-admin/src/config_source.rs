use std::sync::Arc;

use crate::store::Store;

/// Bridges the async Store trait to the sync ConfigSource trait
/// so that prism-router can pull config from a database backend.
pub struct DbConfigSource {
    store: Arc<dyn Store>,
    runtime: tokio::runtime::Handle,
}

impl DbConfigSource {
    pub fn new(store: Arc<dyn Store>, runtime: tokio::runtime::Handle) -> Self {
        Self { store, runtime }
    }
}

impl prism_router::config::ConfigSource for DbConfigSource {
    fn name(&self) -> &str {
        "database"
    }

    fn load(&self) -> Result<prism_router::config::types::PrismConfig, prism_router::config::ConfigError> {
        self.runtime.block_on(async {
            self.store
                .snapshot()
                .await
                .map_err(|e| prism_router::config::ConfigError::Other(e.to_string()))
        })
    }
}
