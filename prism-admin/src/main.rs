use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Router,
};
use clap::Parser;
use rust_embed::Embed;
use tower_http::cors::CorsLayer;
use tracing::info;

mod api;
pub mod cluster;
pub mod config_source;
pub mod push;
pub mod store;

#[derive(Embed)]
#[folder = "frontend/dist"]
struct Assets;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn store::Store>,
    pub store_name: String,
}

#[derive(Parser)]
#[command(name = "prism-admin", about = "Prism Admin UI")]
struct Cli {
    /// Router config file (read-only reference, optional initial import source)
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Listen address
    #[arg(long, default_value = "0.0.0.0:8088")]
    listen: String,

    /// Store URI. Examples:
    ///   file:///path/to/data.yaml       (YAML file)
    ///   file:///path/to/data.json       (JSON file)
    ///   file:///path/to/data.toml       (TOML file)
    ///   sqlite:///path/to/prism.db      (SQLite)
    ///   sqlite::memory:                 (SQLite in-memory)
    ///   postgres://user:pass@host/db    (PostgreSQL)
    #[arg(long, default_value = "file://./prism-admin.yaml")]
    store: String,
}

/// Parse a store URI and return (Arc<dyn Store>, store_type_name).
async fn parse_store_uri(uri: &str) -> (Arc<dyn store::Store>, String) {
    // file:// — local file store
    if let Some(path_str) = uri.strip_prefix("file://") {
        let path = PathBuf::from(path_str);
        let fmt = store::file::FileFormat::from_extension(&path);
        info!("Store: file ({}) → {}", fmt, path.display());
        return (
            Arc::new(store::file::FileStore::new(path, fmt)),
            "file".to_string(),
        );
    }

    // postgres:// or postgresql://
    #[cfg(feature = "postgres")]
    if uri.starts_with("postgres://") || uri.starts_with("postgresql://") {
        info!("Store: postgres → {}", uri);
        let pg = store::postgres::PostgresStore::new(uri)
            .await
            .expect("failed to connect to PostgreSQL");
        return (Arc::new(pg), "postgres".to_string());
    }

    // sqlite://
    #[cfg(feature = "sqlite")]
    if let Some(db_path) = uri.strip_prefix("sqlite://") {
        info!("Store: sqlite → {}", db_path);
        let sq = store::sqlite::SqliteStore::new(db_path)
            .await
            .expect("failed to connect to SQLite");
        return (Arc::new(sq), "sqlite".to_string());
    }

    // Bare path fallback — treat as file://
    if !uri.contains("://") {
        let path = PathBuf::from(uri);
        let fmt = store::file::FileFormat::from_extension(&path);
        info!("Store: file ({}, bare path) → {}", fmt, path.display());
        return (
            Arc::new(store::file::FileStore::new(path, fmt)),
            "file".to_string(),
        );
    }

    eprintln!(
        "Unknown store URI: '{}'\n\nSupported schemes:\n  file://   — local file (yaml/json/toml)\n  sqlite:// — SQLite database\n  postgres:// — PostgreSQL database\n\nExamples:\n  file:///etc/prism/data.yaml\n  sqlite:///var/lib/prism/admin.db\n  postgres://prism:secret@localhost/prism",
        uri,
    );
    std::process::exit(1);
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

    let (backend, store_name) = parse_store_uri(&cli.store).await;

    if let Some(ref config_path) = cli.config {
        info!("Router config (reference): {}", config_path.display());
    }

    let state = AppState {
        store: backend,
        store_name,
    };

    let app = Router::new()
        .route("/api/config", get(api::config::get_config))
        .route("/api/config", put(api::config::update_config))
        .route("/api/config/validate", post(api::config::validate_config))
        .route("/api/providers", get(api::providers::list_providers))
        .route("/api/providers/{id}", get(api::providers::get_provider))
        .route("/api/providers/{id}", put(api::providers::update_provider))
        .route("/api/providers/{id}", delete(api::providers::delete_provider))
        .route("/api/provider-types", get(api::providers::list_provider_types))
        .route("/api/accounts", get(api::accounts::list_accounts))
        .route("/api/accounts", post(api::accounts::create_account))
        .route("/api/accounts/{id}", get(api::accounts::get_account))
        .route("/api/accounts/{id}", put(api::accounts::update_account))
        .route("/api/accounts/{id}", delete(api::accounts::delete_account))
        .route("/api/models", get(api::models::list_models))
        .route("/api/models", post(api::models::create_model))
        .route("/api/models/{id}", put(api::models::update_model))
        .route("/api/models/{id}", delete(api::models::delete_model))
        .route("/api/variants", get(api::variants::list_variants))
        .route("/api/variants", post(api::variants::create_variant))
        .route("/api/variants/{id}", put(api::variants::update_variant))
        .route("/api/variants/{id}", delete(api::variants::delete_variant))
        .route("/api/rate-limits", get(api::rate_limits::list_rate_limits))
        .route("/api/rate-limits", post(api::rate_limits::create_rate_limit))
        .route("/api/rate-limits/{name}", put(api::rate_limits::update_rate_limit))
        .route("/api/rate-limits/{name}", delete(api::rate_limits::delete_rate_limit))
        .route("/api/pools", get(api::pools::list_pools))
        .route("/api/pools", post(api::pools::create_pool))
        .route("/api/pools/{id}", put(api::pools::update_pool))
        .route("/api/pools/{id}", delete(api::pools::delete_pool))
        .route("/api/plugins", get(api::plugins::list_plugins))
        .route("/api/plugins", post(api::plugins::create_plugin))
        .route("/api/plugins/{name}", put(api::plugins::update_plugin))
        .route("/api/plugins/{name}", delete(api::plugins::delete_plugin))
        .route("/api/auth", get(api::auth::get_auth))
        .route("/api/auth", put(api::auth::update_auth))
        // Client Keys
        .route("/api/client-keys", get(api::client_keys::list_client_keys))
        .route("/api/client-keys", post(api::client_keys::create_client_key))
        .route("/api/client-keys/{name}", get(api::client_keys::get_client_key))
        .route("/api/client-keys/{name}", put(api::client_keys::update_client_key))
        .route("/api/client-keys/{name}", delete(api::client_keys::delete_client_key))
        .route("/api/access-log", get(api::access_log::get_access_log))
        .route("/api/access-log", put(api::access_log::update_access_log))
        .route("/api/status", get(api::status::get_status))
        .route("/api/proxy-fetch", post(api::proxy_fetch::proxy_fetch))
        // Clusters
        .route("/api/clusters", get(api::clusters::list_clusters))
        .route("/api/clusters", post(api::clusters::create_cluster))
        .route("/api/clusters/{id}", get(api::clusters::get_cluster))
        .route("/api/clusters/{id}", put(api::clusters::update_cluster))
        .route("/api/clusters/{id}", delete(api::clusters::delete_cluster))
        .route("/api/clusters/{id}/config", get(api::clusters::get_cluster_config))
        .route("/api/clusters/{id}/sync", post(api::clusters::sync_cluster))
        // Proxies
        .route("/api/proxies", get(api::proxies::list_proxies))
        .route("/api/proxies/register", post(api::proxies::register_proxy))
        .route("/api/proxies/heartbeat", post(api::proxies::heartbeat_proxy))
        .route("/api/proxies/{instance_id}", delete(api::proxies::delete_proxy))
        // Routing
        .route("/api/routing", get(api::routing::get_routing))
        .route("/api/routing", put(api::routing::update_routing))
        .fallback(static_handler)
        .layer(CorsLayer::very_permissive())
        .with_state(state);

    let addr: SocketAddr = cli.listen.parse().expect("Invalid listen address");
    info!("Prism Admin UI → http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    if let Some(content) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return Response::builder()
            .header(header::CONTENT_TYPE, mime.as_ref())
            .body(axum::body::Body::from(content.data.to_vec()))
            .unwrap();
    }

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
