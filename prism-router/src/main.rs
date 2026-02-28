mod api;
mod config;
pub mod grpc;
mod state;

/// Generated protobuf / gRPC code.
mod gen {
    #[allow(clippy::all, warnings)]
    pub mod prism_router_v1 {
        include!("gen/prism.router.v1.rs");
    }
}

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::routing::{get, post};
use axum::Router;
use clap::Parser;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{info, error};

use crate::config::{build_registry, load_config};
use crate::gen::prism_router_v1::router_service_server::RouterServiceServer;
use crate::grpc::GrpcRouterService;
use crate::state::AppState;

/// Prism Router — LLM routing decision service
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
    #[arg(long)]
    grpc_listen: Option<String>,
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

    // Determine gRPC listen address
    let grpc_addr_str = cli.grpc_listen.unwrap_or_else(|| {
        format!("0.0.0.0:{}", config.server.grpc_port)
    });

    // Build registry and router
    let registry = build_registry(&config);
    let router = prism_core::router::PrismRouter::new(registry);
    let state = AppState::new(router);

    // Build Axum HTTP router
    let app = Router::new()
        .route("/v1/route", post(api::route_handler))
        .route("/v1/outcomes", post(api::outcomes_handler))
        .route("/health", get(api::health_handler))
        .route("/version", get(api::version_handler))
        .route("/metrics", get(api::metrics_handler))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    // Parse addresses
    let http_addr: SocketAddr = http_addr_str.parse().unwrap_or_else(|e| {
        error!("Invalid HTTP listen address '{}': {}", http_addr_str, e);
        std::process::exit(1);
    });

    let grpc_addr: SocketAddr = grpc_addr_str.parse().unwrap_or_else(|e| {
        error!("Invalid gRPC listen address '{}': {}", grpc_addr_str, e);
        std::process::exit(1);
    });

    info!("🚀 Prism Router starting — HTTP on {}, gRPC on {}", http_addr, grpc_addr);

    // Build gRPC server
    let grpc_service = GrpcRouterService::new(state);
    let grpc_server = tonic::transport::Server::builder()
        .add_service(RouterServiceServer::new(grpc_service))
        .serve_with_shutdown(grpc_addr, shutdown_signal());

    // Build HTTP server
    let http_listener = tokio::net::TcpListener::bind(http_addr).await.unwrap_or_else(|e| {
        error!("Failed to bind HTTP to {}: {}", http_addr, e);
        std::process::exit(1);
    });
    let http_server = axum::serve(http_listener, app)
        .with_graceful_shutdown(shutdown_signal());

    // Run both servers concurrently
    tokio::select! {
        result = http_server => {
            if let Err(e) = result {
                error!("HTTP server error: {}", e);
            }
        }
        result = grpc_server => {
            if let Err(e) = result {
                error!("gRPC server error: {}", e);
            }
        }
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
