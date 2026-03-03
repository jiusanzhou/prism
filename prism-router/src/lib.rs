//! prism-router library — re-exports for integration tests and embedding.

pub mod api;
pub mod codec;
pub mod config;
pub mod config_watcher;
pub mod metrics;
pub mod plugin_loader;
pub mod plugin_manager;
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
pub mod protocol;
#[cfg(feature = "proxy")]
pub mod proxy;
#[cfg(feature = "proxy")]
pub mod ratelimit;
#[cfg(feature = "proxy")]
pub mod usage;
