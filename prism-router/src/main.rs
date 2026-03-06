mod api;
pub mod codec;
mod config;
pub mod provider;
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
mod health;
#[cfg(feature = "proxy")]
mod protocol;
#[cfg(feature = "proxy")]
mod proxy;
#[cfg(feature = "proxy")]
pub mod ratelimit;
#[cfg(feature = "proxy")]
mod circuit_breaker;

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
    // Initialize tracing (with optional OpenTelemetry)
    init_tracing();

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
        .route("/admin/config/validate", post(api::validate_config_handler))
        .route("/health", get(api::health_handler))
        .route("/readyz", get(api::readyz_handler))
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

                // Auth plugin — must be first in chain (before routing/usage)
                if config.auth.enabled && !config.auth.keys.is_empty() {
                    use crate::codec::auth::ApiKeyAuthPlugin;
                    let auth_plugin = ApiKeyAuthPlugin::with_keys(config.auth.keys.clone());
                    global_chain.add(Box::new(auth_plugin));
                    info!("  Auth → enabled ({} keys)", config.auth.keys.len());
                }

                // Retry plugin
                {
                    use crate::codec::retry::RetryPlugin;
                    global_chain.add(Box::new(RetryPlugin::new()));
                }

                global_chain.add(Box::new(UsageExtractorPlugin::new()));
                // Streaming quota enforcement — checks token usage during streaming
                {
                    use crate::codec::streaming_quota::StreamingQuotaPlugin;
                    global_chain.add(Box::new(StreamingQuotaPlugin::new()));
                }
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
                        additional_windows: vec![],
                        message: "Rate limit exceeded".to_string(),
                    })
                    .collect();
                info!("  Rate limiter → {} rules", rules.len());
                Some(Arc::new(RateLimiter::new(rules)))
            } else {
                None
            };

            // Build concurrency limiter from config
            let concurrency_limiter = if !config.concurrency_limits.is_empty() {
                use crate::ratelimit::concurrency::{ConcurrencyLimiter, ConcurrencyRule};
                let rules: Vec<ConcurrencyRule> = config
                    .concurrency_limits
                    .iter()
                    .map(|c| ConcurrencyRule {
                        name: c.name.clone(),
                        key_headers: c.key_headers.clone(),
                        max_concurrent: c.max_concurrent,
                        enabled: c.enabled,
                        status_code: 429,
                        message: "Too many concurrent requests".to_string(),
                    })
                    .collect();
                info!("  Concurrency limiter → {} rules", rules.len());
                Some(Arc::new(ConcurrencyLimiter::new(rules)))
            } else {
                None
            };

            // Build circuit breaker
            let circuit_breaker = {
                use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
                let cb = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default()));
                // Apply per-provider overrides from config
                for (provider_id, provider_cfg) in &config.providers {
                    if provider_cfg.circuit_breaker_threshold != 5
                        || provider_cfg.circuit_breaker_recovery_secs != 30
                    {
                        cb.set_provider_config(
                            provider_id,
                            CircuitBreakerConfig {
                                threshold: provider_cfg.circuit_breaker_threshold,
                                recovery_secs: provider_cfg.circuit_breaker_recovery_secs,
                            },
                        );
                    }
                }
                info!("  Circuit breaker → enabled");
                cb
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
            prism_proxy = prism_proxy.with_timeouts(
                config.server.request_timeout_secs,
                config.server.keepalive_secs,
            );
            if let Some(concurrency) = concurrency_limiter {
                prism_proxy = prism_proxy.with_concurrency_limiter(concurrency);
            }
            // Build health checker from registry endpoints
            let health_checker = {
                use crate::health::{HealthChecker, HealthCheckConfig, EndpointInfo};
                let hc_config = HealthCheckConfig::default();
                let hc = std::sync::Arc::new(HealthChecker::new(hc_config));

                // Collect endpoint info from registry for active probing
                let router_guard = state.router.load();
                let endpoints: Vec<EndpointInfo> = router_guard.registry.endpoints.iter()
                    .map(|(id, ep)| {
                        let provider = router_guard.registry.providers.get(&ep.provider_id);
                        let base_url = ep.base_url.clone()
                            .or_else(|| provider.map(|p| p.base_url.clone()))
                            .unwrap_or_default();
                        let auth_header = provider.and_then(|p| {
                            match &p.auth {
                                prism_core::types::AuthConfig::ApiKey { key, header } if !key.is_empty() => {
                                    Some((header.clone(), key.clone()))
                                }
                                _ => None,
                            }
                        });
                        EndpointInfo {
                            id: id.clone(),
                            provider_id: ep.provider_id.clone(),
                            base_url,
                            auth_header,
                        }
                    })
                    .collect();

                info!("  Health checker → {} endpoints", endpoints.len());

                // Spawn background health check loop
                let hc_clone = std::sync::Arc::clone(&hc);
                let (_hc_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
                hc_clone.spawn_checker(
                    endpoints,
                    std::time::Duration::from_secs(30),
                    shutdown_rx,
                );

                // _hc_shutdown_tx is intentionally leaked (kept alive) —
                // health checker runs for the lifetime of the process.
                // It will be cleaned up on process exit.
                std::mem::forget(_hc_shutdown_tx);

                hc
            };

            prism_proxy = prism_proxy
                .with_circuit_breaker(circuit_breaker)
                .with_max_request_body(config.server.max_request_body_bytes)
                .with_auth(config.auth.clone())
                .with_health_checker(health_checker);
            let prism_proxy = prism_proxy;

            // Pingora runs in its own thread
            let shutdown_timeout = config.server.shutdown_timeout_secs;
            let proxy_handle = std::thread::spawn(move || {
                let mut server = pingora::prelude::Server::new(None).unwrap();
                server.bootstrap();

                let mut proxy_service =
                    pingora::proxy::http_proxy_service(&server.configuration, prism_proxy);
                proxy_service.add_tcp(&proxy_addr_str);
                server.add_service(proxy_service);

                // Pingora's run_forever() blocks; graceful shutdown is handled
                // at the process level via SIGTERM → Pingora intercepts signals.
                // The shutdown_timeout_secs config is available for future
                // drain period customization.
                let _ = shutdown_timeout;
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


fn init_tracing() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "prism_router=info,tower_http=info".into());

    #[cfg(feature = "otel")]
    {
        use opentelemetry::trace::TracerProvider;
        use opentelemetry_sdk::runtime;
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        let otlp_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:4317".to_string());

        if let Ok(exporter) = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&otlp_endpoint)
            .build()
        {
            let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
                .with_batch_exporter(exporter, runtime::Tokio)
                .build();
            let tracer = provider.tracer("prism-router");
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer().with_target(false))
                .with(otel_layer)
                .init();

            info!("OpenTelemetry tracing enabled → {}", otlp_endpoint);
        } else {
            // OTLP init failed, fall back to plain tracing
            tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_target(false)
                .init();
            warn!("OpenTelemetry init failed, using plain tracing");
        }
    }

    #[cfg(not(feature = "otel"))]
    {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_target(false)
            .init();
    }
}
