mod config;
mod protocol;
mod proxy;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use pingora::prelude::*;
use tracing::{error, info};

use crate::config::{build_registry, load_config};
use crate::proxy::PrismProxy;

/// Prism Proxy — LLM API proxy gateway with intelligent routing
#[derive(Parser, Debug)]
#[command(name = "prism-proxy", version, about)]
struct Cli {
    /// Path to the YAML configuration file
    #[arg(short, long, default_value = "configs/prism.yaml")]
    config: PathBuf,

    /// Listen address (overrides config file)
    #[arg(short, long)]
    listen: Option<String>,

    /// Run as daemon
    #[arg(short, long)]
    daemon: bool,
}

fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "prism_proxy=info".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    // Load configuration
    let config = match load_config(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    let listen_addr = cli.listen.unwrap_or_else(|| {
        if config.server.listen != "0.0.0.0:8080" {
            config.server.listen.clone()
        } else {
            format!("0.0.0.0:{}", config.server.http_port)
        }
    });

    // Build registry and router
    let registry = build_registry(&config);
    let router = prism_core::router::PrismRouter::new(registry);
    let router = Arc::new(router);

    info!("🚀 Prism Proxy starting on {}", listen_addr);

    // Create Pingora server
    let mut server = Server::new(None).unwrap();
    server.bootstrap();

    // Create and register the proxy service
    let prism_proxy = PrismProxy::new(Arc::clone(&router));

    let mut proxy_service = pingora::proxy::http_proxy_service(
        &server.configuration,
        prism_proxy,
    );
    proxy_service.add_tcp(&listen_addr);

    server.add_service(proxy_service);

    info!("Prism Proxy listening on {}", listen_addr);

    server.run_forever();
}
