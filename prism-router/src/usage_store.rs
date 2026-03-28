//! SQLite-backed usage backend.
//!
//! Implements `prism_core::usage::UsageBackend` with WAL mode, connection pooling
//! via `Mutex<Connection>`, and configurable retention.

use std::sync::Mutex;

use prism_core::usage::{UsageBackend, UsageQuery, UsageRecord, UsageSummary};
use rusqlite::{params, Connection};
use tracing::{debug, error, warn};

/// SQLite implementation of [`UsageBackend`].
pub struct SqliteUsageBackend {
    conn: Mutex<Connection>,
    retention_days: u32,
}

impl SqliteUsageBackend {
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

        debug!(path, retention_days, "SQLite usage backend initialized");

        Ok(Self {
            conn: Mutex::new(conn),
            retention_days,
        })
    }
}

impl UsageBackend for SqliteUsageBackend {
    fn name(&self) -> &str {
        "sqlite"
    }

    fn record(&self, rec: &UsageRecord) -> Result<(), String> {
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

    fn query(&self, query: &UsageQuery) -> Result<Vec<UsageSummary>, String> {
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

    fn cleanup(&self) -> Result<u64, String> {
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
pub fn spawn_cleanup_task(backend: std::sync::Arc<dyn UsageBackend>, interval_hours: u64) {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(interval_hours * 3600));
        loop {
            interval.tick().await;
            let backend = backend.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || backend.cleanup()).await {
                error!("Usage cleanup task failed: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> SqliteUsageBackend {
        SqliteUsageBackend::new(":memory:", 90).unwrap()
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
    fn test_backend_name() {
        let store = temp_store();
        assert_eq!(store.name(), "sqlite");
    }

    #[test]
    fn test_record_and_query() {
        let store = temp_store();
        store.record(&sample_record("sk-test", "gpt-4o")).unwrap();
        store.record(&sample_record("sk-test", "gpt-4o")).unwrap();
        store
            .record(&sample_record("sk-other", "claude-sonnet-4"))
            .unwrap();

        let results = store.query(&UsageQuery::default()).unwrap();
        assert_eq!(results.len(), 2);

        // Filter by client_key
        let results = store
            .query(&UsageQuery {
                client_key: Some("sk-test".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].total_requests, 2);
        assert_eq!(results[0].total_input_tokens, 200);

        // Filter by model
        let results = store
            .query(&UsageQuery {
                model: Some("claude-sonnet-4".to_string()),
                ..Default::default()
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
        assert_eq!(deleted, 0);

        let results = store.query(&UsageQuery::default()).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_cleanup_purges_old() {
        let store = temp_store();

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
            .query(&UsageQuery {
                from: Some("2025-06-10T00:00:00.000Z".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].total_requests, 1);
    }

    #[test]
    fn test_empty_query() {
        let store = temp_store();
        let results = store.query(&UsageQuery::default()).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_trait_object() {
        let store = temp_store();
        let backend: std::sync::Arc<dyn UsageBackend> = std::sync::Arc::new(store);
        backend.record(&sample_record("sk-test", "gpt-4o")).unwrap();
        let results = backend.query(&UsageQuery::default()).unwrap();
        assert_eq!(results.len(), 1);
    }
}
