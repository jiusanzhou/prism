//! Usage Store — persists token usage records to SQLite for auditing and billing.
//!
//! Wraps `rusqlite::Connection` behind a `Mutex` for thread safety.
//! All writes use `spawn_blocking` to avoid blocking the Tokio runtime.

use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, warn};

/// A single usage record persisted to SQLite.
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
#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    pub client_key: Option<String>,
    pub model: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Aggregated usage summary row.
#[derive(Debug, Serialize)]
pub struct UsageSummary {
    pub client_key: String,
    pub model: String,
    pub total_requests: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub avg_latency_ms: f64,
}

/// SQLite-backed usage store.
pub struct UsageStore {
    conn: Mutex<Connection>,
    retention_days: u32,
}

impl UsageStore {
    /// Open (or create) the SQLite database at `path` and initialize the schema.
    pub fn new(path: &str, retention_days: u32) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("Failed to open usage DB: {e}"))?;

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(|e| format!("Failed to set pragmas: {e}"))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_records (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                client_key TEXT NOT NULL,
                model TEXT NOT NULL,
                variant TEXT NOT NULL DEFAULT '',
                provider TEXT NOT NULL DEFAULT '',
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                latency_ms INTEGER NOT NULL DEFAULT 0,
                status_code INTEGER NOT NULL DEFAULT 0,
                request_id TEXT NOT NULL DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_usage_timestamp ON usage_records(timestamp);
            CREATE INDEX IF NOT EXISTS idx_usage_client_key ON usage_records(client_key);
            CREATE INDEX IF NOT EXISTS idx_usage_model ON usage_records(model);",
        )
        .map_err(|e| format!("Failed to create usage schema: {e}"))?;

        debug!(path, retention_days, "Usage store initialized");

        Ok(Self {
            conn: Mutex::new(conn),
            retention_days,
        })
    }

    /// Insert a usage record. Call from `spawn_blocking`.
    pub fn record(&self, rec: &UsageRecord) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Lock poisoned: {e}"))?;
        conn.execute(
            "INSERT INTO usage_records (timestamp, client_key, model, variant, provider, input_tokens, output_tokens, total_tokens, latency_ms, status_code, request_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                rec.timestamp,
                rec.client_key,
                rec.model,
                rec.variant,
                rec.provider,
                rec.input_tokens,
                rec.output_tokens,
                rec.total_tokens,
                rec.latency_ms,
                rec.status_code,
                rec.request_id,
            ],
        )
        .map_err(|e| format!("Failed to insert usage record: {e}"))?;
        Ok(())
    }

    /// Query aggregated usage by client_key/model within a time range.
    pub fn query_usage(&self, query: &UsageQuery) -> Result<Vec<UsageSummary>, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Lock poisoned: {e}"))?;

        let mut sql = String::from(
            "SELECT client_key, model, COUNT(*) as total_requests,
                    SUM(input_tokens) as total_input_tokens,
                    SUM(output_tokens) as total_output_tokens,
                    SUM(total_tokens) as total_tokens,
                    AVG(latency_ms) as avg_latency_ms
             FROM usage_records WHERE 1=1",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(ref ck) = query.client_key {
            sql.push_str(&format!(" AND client_key = ?{}", param_values.len() + 1));
            param_values.push(Box::new(ck.clone()));
        }
        if let Some(ref m) = query.model {
            sql.push_str(&format!(" AND model = ?{}", param_values.len() + 1));
            param_values.push(Box::new(m.clone()));
        }
        if let Some(ref from) = query.from {
            sql.push_str(&format!(" AND timestamp >= ?{}", param_values.len() + 1));
            param_values.push(Box::new(from.clone()));
        }
        if let Some(ref to) = query.to {
            sql.push_str(&format!(" AND timestamp <= ?{}", param_values.len() + 1));
            param_values.push(Box::new(to.clone()));
        }

        sql.push_str(" GROUP BY client_key, model ORDER BY total_tokens DESC");

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(UsageSummary {
                    client_key: row.get(0)?,
                    model: row.get(1)?,
                    total_requests: row.get(2)?,
                    total_input_tokens: row.get(3)?,
                    total_output_tokens: row.get(4)?,
                    total_tokens: row.get(5)?,
                    avg_latency_ms: row.get(6)?,
                })
            })
            .map_err(|e| format!("Failed to query usage: {e}"))?;

        let mut results = Vec::new();
        for row in rows {
            match row {
                Ok(summary) => results.push(summary),
                Err(e) => warn!("Skipping malformed usage row: {e}"),
            }
        }
        Ok(results)
    }

    /// Purge records older than the configured retention period.
    pub fn cleanup(&self) -> Result<u64, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| format!("Lock poisoned: {e}"))?;
        let cutoff = chrono::Utc::now() - chrono::Duration::days(self.retention_days as i64);
        let cutoff_str = cutoff.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

        let deleted = conn
            .execute(
                "DELETE FROM usage_records WHERE timestamp < ?1",
                params![cutoff_str],
            )
            .map_err(|e| format!("Failed to cleanup usage records: {e}"))?;

        debug!(
            deleted,
            retention_days = self.retention_days,
            "Usage cleanup"
        );
        Ok(deleted as u64)
    }
}

/// Spawn a background task that runs cleanup periodically.
pub fn spawn_cleanup_task(store: std::sync::Arc<UsageStore>, interval_hours: u64) {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(interval_hours * 3600));
        loop {
            interval.tick().await;
            let store = store.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || store.cleanup()).await {
                error!("Usage cleanup task failed: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> UsageStore {
        UsageStore::new(":memory:", 90).unwrap()
    }

    fn sample_record(client_key: &str, model: &str) -> UsageRecord {
        UsageRecord {
            id: None,
            timestamp: chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string(),
            client_key: client_key.to_string(),
            model: model.to_string(),
            variant: format!("{model}@provider"),
            provider: "provider".to_string(),
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            latency_ms: 200,
            status_code: 200,
            request_id: "req-1".to_string(),
        }
    }

    #[test]
    fn test_record_and_query() {
        let store = temp_store();
        store.record(&sample_record("sk-test", "gpt-4o")).unwrap();
        store.record(&sample_record("sk-test", "gpt-4o")).unwrap();
        store
            .record(&sample_record("sk-other", "claude-sonnet-4"))
            .unwrap();

        let results = store
            .query_usage(&UsageQuery {
                client_key: None,
                model: None,
                from: None,
                to: None,
            })
            .unwrap();
        assert_eq!(results.len(), 2);

        // Filter by client_key
        let results = store
            .query_usage(&UsageQuery {
                client_key: Some("sk-test".to_string()),
                model: None,
                from: None,
                to: None,
            })
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].total_requests, 2);
        assert_eq!(results[0].total_input_tokens, 200);

        // Filter by model
        let results = store
            .query_usage(&UsageQuery {
                client_key: None,
                model: Some("claude-sonnet-4".to_string()),
                from: None,
                to: None,
            })
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].client_key, "sk-other");
    }

    #[test]
    fn test_cleanup_keeps_recent() {
        let store = temp_store();
        store.record(&sample_record("sk-test", "gpt-4o")).unwrap();

        let deleted = store.cleanup().unwrap();
        assert_eq!(deleted, 0); // record is recent

        let results = store
            .query_usage(&UsageQuery {
                client_key: None,
                model: None,
                from: None,
                to: None,
            })
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_cleanup_purges_old() {
        let store = temp_store();

        // Insert a record with an old timestamp
        let mut rec = sample_record("sk-test", "gpt-4o");
        rec.timestamp = "2020-01-01T00:00:00.000Z".to_string();
        store.record(&rec).unwrap();

        let deleted = store.cleanup().unwrap();
        assert_eq!(deleted, 1);
    }

    #[test]
    fn test_query_with_time_range() {
        let store = temp_store();

        let mut rec = sample_record("sk-test", "gpt-4o");
        rec.timestamp = "2025-06-01T00:00:00.000Z".to_string();
        store.record(&rec).unwrap();

        rec.timestamp = "2025-06-15T00:00:00.000Z".to_string();
        store.record(&rec).unwrap();

        let results = store
            .query_usage(&UsageQuery {
                client_key: None,
                model: None,
                from: Some("2025-06-10T00:00:00.000Z".to_string()),
                to: None,
            })
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].total_requests, 1);
    }

    #[test]
    fn test_empty_query() {
        let store = temp_store();
        let results = store
            .query_usage(&UsageQuery {
                client_key: None,
                model: None,
                from: None,
                to: None,
            })
            .unwrap();
        assert!(results.is_empty());
    }
}
