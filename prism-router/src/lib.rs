//! prism-router library — re-exports for integration tests and embedding.

pub mod api;
pub mod client_keys;
pub mod codec;
pub mod config;
pub mod config_watcher;
pub mod metrics;
pub mod plugin_loader;
pub mod plugin_manager;
pub mod provider;
pub mod sidecar_discovery;
pub mod state;
pub mod unified_plugin;

#[cfg(feature = "grpc")]
pub mod grpc;

#[cfg(feature = "grpc")]
pub mod gen {
    #[allow(clippy::all, warnings)]
    pub mod prism_router_v1 {
        include!("gen/prism.router.v1.rs");
    }
}

#[cfg(feature = "proxy")]
pub mod access_log;
#[cfg(feature = "proxy")]
pub mod circuit_breaker;
pub mod health;
#[cfg(feature = "proxy")]
pub mod protocol;
#[cfg(feature = "proxy")]
pub mod proxy;
#[cfg(feature = "proxy")]
pub mod ratelimit;
#[cfg(feature = "usage-pg")]
pub mod usage_pg;
#[cfg(feature = "usage-db")]
#[cfg(feature = "usage-db")]
pub mod usage_store;
