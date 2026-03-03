use std::net::SocketAddr;
use std::path::PathBuf;

use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post, put, delete},
    Router,
};
use clap::Parser;
use rust_embed::Embed;
use tower_http::cors::CorsLayer;
use tracing::info;

mod api;

#[derive(Embed)]
#[folder = "frontend/dist"]
struct Assets;

#[derive(Clone)]
pub struct AppState {
    pub config_path: PathBuf,
}

#[derive(Parser)]
#[command(name = "prism-admin", about = "Prism Admin UI")]
struct Cli {
    /// Config file path (same as prism-router's config)
    #[arg(short, long, default_value = "config.yaml")]
    config: PathBuf,

    /// Listen address
    #[arg(long, default_value = "0.0.0.0:8088")]
    listen: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "prism_admin=info".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let state = AppState {
        config_path: cli.config,
    };

    let app = Router::new()
        // API routes
        .route("/api/config", get(api::config::get_config))
        .route("/api/config", put(api::config::update_config))
        .route("/api/config/validate", post(api::config::validate_config))
        .route("/api/providers", get(api::providers::list_providers))
        .route("/api/providers/{id}", get(api::providers::get_provider))
        .route("/api/providers/{id}", put(api::providers::update_provider))
        .route("/api/providers/{id}", delete(api::providers::delete_provider))
        .route("/api/models", get(api::models::list_models))
        .route("/api/variants", get(api::variants::list_variants))
        .route("/api/rate-limits", get(api::rate_limits::list_rate_limits))
        .route("/api/status", get(api::status::get_status))
        // Frontend SPA — catch-all
        .fallback(static_handler)
        .layer(CorsLayer::very_permissive())
        .with_state(state);

    let addr: SocketAddr = cli.listen.parse().expect("Invalid listen address");
    info!("Prism Admin UI → http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// Serve embedded frontend assets (SPA fallback to index.html).
async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // Try exact file match
    if let Some(content) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return Response::builder()
            .header(header::CONTENT_TYPE, mime.as_ref())
            .body(axum::body::Body::from(content.data.to_vec()))
            .unwrap();
    }

    // SPA fallback: serve index.html for all non-asset routes
    match Assets::get("index.html") {
        Some(content) => Response::builder()
            .header(header::CONTENT_TYPE, "text/html")
            .body(axum::body::Body::from(content.data.to_vec()))
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/html")
            .body(axum::body::Body::from(
                "<h1>Prism Admin</h1><p>Frontend not built. Run <code>cd frontend && pnpm build</code></p>",
            ))
            .unwrap(),
    }
}
