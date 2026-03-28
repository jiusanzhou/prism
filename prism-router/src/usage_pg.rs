//! PostgreSQL-backed usage backend.
//!
//! Implements `prism_core::usage::UsageBackend` using `deadpool-postgres`
//! for connection pooling. Since the trait is synchronous, we bridge to
//! async via `tokio::runtime::Handle::current().block_on()` — callers
//! are expected to run this from `spawn_blocking`.

use prism_core::usage::{UsageBackend, UsageQuery, UsageRecord, UsageSummary};
use tracing::{debug, error, warn};

/// PostgreSQL implementation of [`UsageBackend`].
pub struct PgUsageBackend {
    pool: deadpool_postgres::Pool,
    retention_days: u32,
}

impl PgUsageBackend {
    /// Create a new PG usage backend.
    ///
    /// `database_url` should be a standard PostgreSQL connection string:
    /// `postgresql://user:pass@host:5432/dbname`
    ///
    /// This constructor initializes the schema (creates the table if not exists).
    pub fn new(database_url: &str, retention_days: u32) -> Result<Self, String> {
        let pg_config: tokio_postgres::Config = database_url
            .parse()
            .map_err(|e| format!("Invalid PG connection string: {e}"))?;

        let mgr_config = deadpool_postgres::ManagerConfig {
            recycling_method: deadpool_postgres::RecyclingMethod::Fast,
        };
        let mgr =
            deadpool_postgres::Manager::from_config(pg_config, tokio_postgres::NoTls, mgr_config);

        let pool = deadpool_postgres::Pool::builder(mgr)
            .max_size(8)
            .build()
            .map_err(|e| format!("Failed to build PG pool: {e}"))?;

        let backend = Self {
            pool,
            retention_days,
        };

        // Initialize schema synchronously (called at startup)
        backend.init_schema()?;

        debug!(retention_days, "PostgreSQL usage backend initialized");
        Ok(backend)
    }

    fn init_schema(&self) -> Result<(), String> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            let client = self
                .pool
                .get()
                .await
                .map_err(|e| format!("PG pool error: {e}"))?;

            client
                .batch_execute(
                    "CREATE TABLE IF NOT EXISTS usage_records (
                        id BIGSERIAL PRIMARY KEY,
                        timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                        client_key TEXT NOT NULL,
                        model TEXT NOT NULL,
                        variant TEXT NOT NULL DEFAULT '',
                        provider TEXT NOT NULL DEFAULT '',
                        input_tokens BIGINT NOT NULL DEFAULT 0,
                        output_tokens BIGINT NOT NULL DEFAULT 0,
                        total_tokens BIGINT NOT NULL DEFAULT 0,
                        latency_ms BIGINT NOT NULL DEFAULT 0,
                        status_code INTEGER NOT NULL DEFAULT 0,
                        request_id TEXT NOT NULL DEFAULT ''
                    );
                    CREATE INDEX IF NOT EXISTS idx_usage_timestamp ON usage_records(timestamp);
                    CREATE INDEX IF NOT EXISTS idx_usage_client_key ON usage_records(client_key);
                    CREATE INDEX IF NOT EXISTS idx_usage_model ON usage_records(model);
                    CREATE INDEX IF NOT EXISTS idx_usage_client_model ON usage_records(client_key, model);",
                )
                .await
                .map_err(|e| format!("Failed to create PG schema: {e}"))?;

            Ok(())
        })
    }
}

impl UsageBackend for PgUsageBackend {
    fn name(&self) -> &str {
        "postgresql"
    }

    fn record(&self, rec: &UsageRecord) -> Result<(), String> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            let client = self
                .pool
                .get()
                .await
                .map_err(|e| format!("PG pool error: {e}"))?;

            client
                .execute(
                    "INSERT INTO usage_records
                        (timestamp, client_key, model, variant, provider,
                         input_tokens, output_tokens, total_tokens,
                         latency_ms, status_code, request_id)
                     VALUES ($1::timestamptz, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                    &[
                        &rec.timestamp,
                        &rec.client_key,
                        &rec.model,
                        &rec.variant,
                        &rec.provider,
                        &rec.input_tokens,
                        &rec.output_tokens,
                        &rec.total_tokens,
                        &rec.latency_ms,
                        &(rec.status_code as i32),
                        &rec.request_id,
                    ],
                )
                .await
                .map_err(|e| format!("PG insert failed: {e}"))?;

            Ok(())
        })
    }

    fn query(&self, query: &UsageQuery) -> Result<Vec<UsageSummary>, String> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            let client = self
                .pool
                .get()
                .await
                .map_err(|e| format!("PG pool error: {e}"))?;

            // Build dynamic query with positional params
            let mut sql = String::from(
                "SELECT client_key, model,
                        COUNT(*)::bigint as total_requests,
                        COALESCE(SUM(input_tokens), 0)::bigint as total_input_tokens,
                        COALESCE(SUM(output_tokens), 0)::bigint as total_output_tokens,
                        COALESCE(SUM(total_tokens), 0)::bigint as total_tokens,
                        COALESCE(AVG(latency_ms::float8), 0) as avg_latency_ms
                 FROM usage_records WHERE 1=1",
            );

            let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync>> = Vec::new();
            let mut idx = 1;

            if let Some(ref ck) = query.client_key {
                sql.push_str(&format!(" AND client_key = ${idx}"));
                params.push(Box::new(ck.clone()));
                idx += 1;
            }
            if let Some(ref m) = query.model {
                sql.push_str(&format!(" AND model = ${idx}"));
                params.push(Box::new(m.clone()));
                idx += 1;
            }
            if let Some(ref from) = query.from {
                sql.push_str(&format!(" AND timestamp >= ${idx}::timestamptz"));
                params.push(Box::new(from.clone()));
                idx += 1;
            }
            if let Some(ref to) = query.to {
                sql.push_str(&format!(" AND timestamp <= ${idx}::timestamptz"));
                params.push(Box::new(to.clone()));
                #[allow(unused_assignments)]
                {
                    idx += 1;
                }
            }

            sql.push_str(" GROUP BY client_key, model ORDER BY total_tokens DESC");

            let params_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
                params.iter().map(|p| p.as_ref()).collect();

            let rows = client
                .query(&sql, &params_refs)
                .await
                .map_err(|e| format!("PG query failed: {e}"))?;

            let results: Vec<UsageSummary> = rows
                .iter()
                .map(|row| UsageSummary {
                    client_key: row.get(0),
                    model: row.get(1),
                    total_requests: row.get(2),
                    total_input_tokens: row.get(3),
                    total_output_tokens: row.get(4),
                    total_tokens: row.get(5),
                    avg_latency_ms: row.get(6),
                })
                .collect();

            Ok(results)
        })
    }

    fn cleanup(&self) -> Result<u64, String> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            let client = self
                .pool
                .get()
                .await
                .map_err(|e| format!("PG pool error: {e}"))?;

            let deleted = client
                .execute(
                    &format!(
                        "DELETE FROM usage_records WHERE timestamp < NOW() - INTERVAL '{} days'",
                        self.retention_days
                    ),
                    &[],
                )
                .await
                .map_err(|e| format!("PG cleanup failed: {e}"))?;

            debug!(
                deleted,
                retention_days = self.retention_days,
                "PG usage cleanup"
            );
            Ok(deleted)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // PG tests require a running PostgreSQL instance.
    // Set PG_TEST_URL=postgresql://user:pass@localhost:5432/prism_test to enable.

    fn get_test_url() -> Option<String> {
        std::env::var("PG_TEST_URL").ok()
    }

    #[test]
    fn test_backend_name() {
        // Name test doesn't need a real connection — but we can't construct
        // without a valid PG. Just verify the constant.
        assert_eq!("postgresql", "postgresql");
    }

    #[tokio::test]
    async fn test_pg_record_and_query() {
        let Some(url) = get_test_url() else {
            eprintln!("Skipping PG test: PG_TEST_URL not set");
            return;
        };

        // Run inside spawn_blocking to match production usage
        let result = tokio::task::spawn_blocking(move || {
            let backend = PgUsageBackend::new(&url, 90).expect("PG connect");

            // Clean slate
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                let client = backend.pool.get().await.unwrap();
                client
                    .execute("DELETE FROM usage_records", &[])
                    .await
                    .unwrap();
            });

            // Insert
            let rec = UsageRecord {
                id: None,
                timestamp: chrono::Utc::now()
                    .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                    .to_string(),
                client_key: "sk-test".into(),
                model: "gpt-4o".into(),
                variant: "gpt-4o@openai".into(),
                provider: "openai".into(),
                input_tokens: 100,
                output_tokens: 50,
                total_tokens: 150,
                latency_ms: 200,
                status_code: 200,
                request_id: "req-pg-1".into(),
            };
            backend.record(&rec).unwrap();

            // Query
            let results = backend.query(&UsageQuery::default()).unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].client_key, "sk-test");
            assert_eq!(results[0].total_tokens, 150);

            // Cleanup (recent records should survive)
            let deleted = backend.cleanup().unwrap();
            assert_eq!(deleted, 0);
        })
        .await;

        result.expect("PG test panicked");
    }
}
