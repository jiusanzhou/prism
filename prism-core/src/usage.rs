//! Usage tracking abstraction — pluggable backend for persisting token usage.
//!
//! Defines the core types (`UsageRecord`, `UsageQuery`, `UsageSummary`) and the
//! `UsageBackend` trait. Implementations live in downstream crates:
//! - `SqliteUsageBackend` (prism-router, feature `usage-db`)
//! - Future: PostgreSQL, ClickHouse, etc.

use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────
// Core types
// ──────────────────────────────────────────────

/// A single usage record capturing one request's resource consumption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub id: Option<i64>,
    pub timestamp: String,
    pub client_key: String,
    pub model: String,
    pub variant: String,
    pub provider: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub latency_ms: i64,
    pub status_code: i64,
    pub request_id: String,
}

/// Query parameters for usage aggregation.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UsageQuery {
    pub client_key: Option<String>,
    pub model: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Aggregated usage summary — one row per (client_key, model) pair.
#[derive(Debug, Clone, Serialize)]
pub struct UsageSummary {
    pub client_key: String,
    pub model: String,
    pub total_requests: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub avg_latency_ms: f64,
}

// ──────────────────────────────────────────────
// Backend trait
// ──────────────────────────────────────────────

/// Pluggable storage backend for usage records.
///
/// All methods are synchronous — callers should wrap in `spawn_blocking`
/// when called from async context (the trait is `Send + Sync` so the
/// backend can be shared across threads).
pub trait UsageBackend: Send + Sync {
    /// Backend name (for logging / debugging).
    fn name(&self) -> &str;

    /// Persist a single usage record.
    fn record(&self, rec: &UsageRecord) -> Result<(), String>;

    /// Query aggregated usage. Returns one row per (client_key, model) pair.
    fn query(&self, query: &UsageQuery) -> Result<Vec<UsageSummary>, String>;

    /// Purge records older than the backend's configured retention period.
    /// Returns the number of records deleted.
    fn cleanup(&self) -> Result<u64, String>;
}
