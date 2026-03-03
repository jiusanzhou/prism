mod api;
pub mod codec;
mod config;
mod config_watcher;
pub mod metrics;
pub mod plugin_loader;
pub mod plugin_manager;
mod state;
pub mod unified_plugin;

#[cfg(feature = "grpc")]
pub mod grpc;

#[cfg(feature = "grpc")]
mod gen {
    #[allow(clippy::all, warnings)]
    pub mod prism_router_v1 {
        include!("gen/prism.router.v1.rs");
    }
}

#[cfg(feature = "proxy")]
mod access_log;
#[cfg(feature = "proxy")]
mod protocol;
#[cfg(feature = "proxy")]
mod proxy;
#[cfg(feature = "proxy")]
pub mod ratelimit;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::routing::{get, post};
use axum::Router;
use clap::Parser;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info};

use crate::config::{build_registry, load_config};
use crate::state::AppState;

/// Prism Router — LLM routing decision service with optional proxy gateway
#[derive(Parser, Debug)]
#[command(name = "prism-router", version, about)]
struct Cli {
    /// Path to the YAML configuration file
    #[arg(short, long, default_value = "configs/prism.yaml")]
    config: PathBuf,

    /// HTTP listen address (overrides config file)
    #[arg(short, long)]
    listen: Option<String>,

    /// gRPC listen address (overrides config file)
    #[cfg_attr(feature = "grpc", arg(long))]
    #[cfg(feature = "grpc")]
    grpc_listen: Option<String>,

    /// Proxy listen address (overrides config file)
    #[cfg_attr(feature = "proxy", arg(long))]
    #[cfg(feature = "proxy")]
    proxy_listen: Option<String>,

    /// Disable proxy even if compiled with proxy feature
    #[cfg_attr(feature = "proxy", arg(long))]
    #[cfg(feature = "proxy")]
    no_proxy: bool,
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "prism_router=info,tower_http=info".into()),
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

    // Determine HTTP listen address
    let http_addr_str = cli.listen.unwrap_or_else(|| {
        if config.server.listen != "0.0.0.0:8080" {
            config.server.listen.clone()
        } else {
            format!("0.0.0.0:{}", config.server.http_port)
        }
    });

    // Build registry and router
    let registry = build_registry(&config);
    let router = prism_core::router::PrismRouter::new(registry);
    let state = AppState::new(router, cli.config.clone());

    // Start config watcher
    let _config_watcher = config_watcher::spawn_config_watcher(
        state.clone(),
        config_watcher::WatcherConfig {
            config_path: cli.config.clone(),
            debounce: Duration::from_secs(2),
            ..Default::default()
        },
    );

    // Start metrics flush background task (every 5 seconds)
    let _metrics_flush = metrics::spawn_metrics_flush(
        Arc::clone(&state.metrics),
        state.data_layer(),
        Duration::from_secs(5),
    );

    // Build Axum HTTP router
    let app = Router::new()
        .route("/v1/route", post(api::route_handler))
        .route("/v1/outcomes", post(api::outcomes_handler))
        .route("/admin/reload", post(api::reload_handler))
        .route("/health", get(api::health_handler))
        .route("/version", get(api::version_handler))
        .route("/metrics", get(api::metrics_handler))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    // Parse HTTP address
    let http_addr: SocketAddr = http_addr_str.parse().unwrap_or_else(|e| {
        error!("Invalid HTTP listen address '{}': {}", http_addr_str, e);
        std::process::exit(1);
    });

    // ── Collect features info for startup banner ──
    let mut features = vec!["http"];
    #[cfg(feature = "grpc")]
    {
        features.push("grpc");
    }
    #[cfg(feature = "proxy")]
    {
        features.push("proxy");
    }

    info!(
        "🚀 Prism Router starting — features: [{}]",
        features.join(", ")
    );

    // ── Start HTTP server ──
    let http_listener = tokio::net::TcpListener::bind(http_addr)
        .await
        .unwrap_or_else(|e| {
            error!("Failed to bind HTTP to {}: {}", http_addr, e);
            std::process::exit(1);
        });
    info!("  HTTP  → {}", http_addr);
    let http_server = axum::serve(http_listener, app).with_graceful_shutdown(shutdown_signal());

    // ── Start gRPC server (if feature enabled) ──
    #[cfg(feature = "grpc")]
    let grpc_handle = {
        use crate::gen::prism_router_v1::router_service_server::RouterServiceServer;
        use crate::grpc::GrpcRouterService;

        let grpc_addr_str = cli
            .grpc_listen
            .unwrap_or_else(|| format!("0.0.0.0:{}", config.server.grpc_port));
        let grpc_addr: SocketAddr = grpc_addr_str.parse().unwrap_or_else(|e| {
            error!("Invalid gRPC listen address '{}': {}", grpc_addr_str, e);
            std::process::exit(1);
        });
        info!("  gRPC  → {}", grpc_addr);

        let grpc_service = GrpcRouterService::new(state.clone());
        let grpc_server = tonic::transport::Server::builder()
            .add_service(RouterServiceServer::new(grpc_service))
            .serve_with_shutdown(grpc_addr, shutdown_signal());

        tokio::spawn(async move {
            if let Err(e) = grpc_server.await {
                error!("gRPC server error: {}", e);
            }
        })
    };

    // ── Start Proxy server (if feature enabled and not disabled) ──
    #[cfg(feature = "proxy")]
    let _proxy_handle = {
        let run_proxy = !cli.no_proxy;
        if run_proxy {
            use crate::proxy::PrismProxy;
            use std::sync::Arc;

            let proxy_addr_str = cli
                .proxy_listen
                .unwrap_or_else(|| format!("0.0.0.0:{}", config.server.proxy_port));
            info!("  Proxy → {}", proxy_addr_str);

            // Build codec registry with built-in plugins
            let codec_registry = {
                use crate::codec::chain::ProxyPluginChain;
                use crate::codec::registry::CodecRegistry;
                use crate::codec::usage_extractor::UsageExtractorPlugin;
                use crate::codec::quota_recorder::QuotaRecorderPlugin;

                let mut registry = CodecRegistry::new();
                let mut global_chain = ProxyPluginChain::new();
                global_chain.add(Box::new(UsageExtractorPlugin::new()));
                global_chain.add(Box::new(QuotaRecorderPlugin::new()));
                registry.set_global_chain(global_chain);
                Arc::new(registry)
            };

            // Build rate limiter from config
            let rate_limiter = if !config.rate_limits.is_empty() {
                use crate::ratelimit::{RateLimitRule, RateLimiter};

                let rules: Vec<RateLimitRule> = config
                    .rate_limits
                    .iter()
                    .map(|rc| RateLimitRule {
                        name: rc.name.clone(),
                        key_headers: rc.key_headers.clone(),
                        window_secs: rc.window_secs,
                        max_requests: Some(rc.max_requests),
                        max_tokens: rc.max_tokens,
                        quotas: vec![],
                        overrides: rc
                            .overrides
                            .iter()
                            .map(|o| crate::ratelimit::RateLimitOverride {
                                match_headers: o.match_headers.clone(),
                                quotas: vec![],
                                max_requests: o.max_requests,
                                max_tokens: o.max_tokens,
                            })
                            .collect(),
                        enabled: rc.enabled,
                        status_code: 429,
                        message: "Rate limit exceeded".to_string(),
                    })
                    .collect();
                info!("  Rate limiter → {} rules", rules.len());
                Some(Arc::new(RateLimiter::new(rules)))
            } else {
                None
            };

            // Build access logger from config
            let access_logger = if config.access_log.enabled {
                use crate::access_log::AccessLogger;
                match AccessLogger::new(config.access_log.clone()) {
                    Ok(logger) => {
                        info!("  Access log → enabled (format: {:?})", config.access_log.format);
                        Some(Arc::new(logger))
                    }
                    Err(e) => {
                        error!("Failed to initialize access logger: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            // Assemble proxy
            let mut prism_proxy = PrismProxy::with_codec_registry(
                Arc::clone(&state.router),
                Arc::clone(&state.metrics),
                codec_registry,
            );
            if let Some(limiter) = rate_limiter {
                prism_proxy = prism_proxy.with_rate_limiter(limiter);
            }
            if let Some(logger) = access_logger {
                prism_proxy = prism_proxy.with_access_logger(logger);
            }
            let prism_proxy = prism_proxy;

            // Pingora runs in its own thread (blocking)
            let proxy_handle = std::thread::spawn(move || {
                let mut server = pingora::prelude::Server::new(None).unwrap();
                server.bootstrap();

                let mut proxy_service =
                    pingora::proxy::http_proxy_service(&server.configuration, prism_proxy);
                proxy_service.add_tcp(&proxy_addr_str);
                server.add_service(proxy_service);
                server.run_forever();
            });
            Some(proxy_handle)
        } else {
            info!("  Proxy → disabled (--no-proxy)");
            None
        }
    };

    // ── Run HTTP server (main task) ──
    if let Err(e) = http_server.await {
        error!("HTTP server error: {}", e);
    }

    // Wait for gRPC to finish
    #[cfg(feature = "grpc")]
    {
        let _ = grpc_handle.await;
    }

    info!("Prism Router stopped");
}

/// Wait for shutdown signals (Ctrl+C or SIGTERM).
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => { info!("Received Ctrl+C, shutting down..."); }
        _ = terminate => { info!("Received SIGTERM, shutting down..."); }
    }
}
