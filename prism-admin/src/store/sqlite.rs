use async_trait::async_trait;
use prism_router::config::types::*;
use sqlx::SqlitePool;

use super::{Store, StoreError, StoreResult};
use crate::cluster::{Cluster, ProxyInstance, ProxyStatus};

pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub async fn new(database_url: &str) -> StoreResult<Self> {
        let pool = SqlitePool::connect(database_url)
            .await
            .map_err(|e| StoreError::Internal(format!("sqlite connect: {}", e)))?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> StoreResult<()> {
        for stmt in &[
            "CREATE TABLE IF NOT EXISTS prism_providers (id TEXT PRIMARY KEY, config TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS prism_accounts (id TEXT PRIMARY KEY, config TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS prism_models (id TEXT PRIMARY KEY, config TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS prism_variants (id TEXT PRIMARY KEY, config TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS prism_pools (id TEXT PRIMARY KEY, config TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS prism_rate_limits (name TEXT PRIMARY KEY, config TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS prism_plugins (name TEXT PRIMARY KEY, config TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS prism_client_keys (name TEXT PRIMARY KEY, config TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS prism_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS prism_clusters (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT DEFAULT '',
                tags TEXT DEFAULT '{}',
                created_at TEXT DEFAULT (datetime('now'))
            )",
            "CREATE TABLE IF NOT EXISTS prism_proxies (
                instance_id TEXT PRIMARY KEY,
                cluster_id TEXT NOT NULL REFERENCES prism_clusters(id),
                address TEXT NOT NULL,
                version TEXT NOT NULL,
                status TEXT DEFAULT 'online',
                last_heartbeat TEXT DEFAULT (datetime('now')),
                registered_at TEXT DEFAULT (datetime('now')),
                config_version INTEGER DEFAULT 0,
                metadata TEXT DEFAULT '{}'
            )",
            "CREATE TABLE IF NOT EXISTS prism_config_versions (
                version INTEGER PRIMARY KEY AUTOINCREMENT,
                cluster_id TEXT NOT NULL,
                config TEXT NOT NULL,
                created_at TEXT DEFAULT (datetime('now'))
            )",
        ] {
            sqlx::query(stmt)
                .execute(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(format!("migration: {}", e)))?;
        }

        Ok(())
    }

    fn parse_json<T: serde::de::DeserializeOwned>(json_str: &str, ctx: &str) -> StoreResult<T> {
        serde_json::from_str(json_str)
            .map_err(|e| StoreError::Internal(format!("deserialize {}: {}", ctx, e)))
    }

    fn to_json<T: serde::Serialize>(val: &T) -> StoreResult<String> {
        serde_json::to_string(val).map_err(|e| StoreError::Internal(e.to_string()))
    }
}

#[async_trait]
impl Store for SqliteStore {
    // Providers
    async fn list_providers(&self) -> StoreResult<Vec<(String, ProviderConfig)>> {
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, config FROM prism_providers")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(id, json)| {
                let config = Self::parse_json(&json, &format!("provider '{}'", id))?;
                Ok((id, config))
            })
            .collect()
    }

    async fn get_provider(&self, id: &str) -> StoreResult<ProviderConfig> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT config FROM prism_providers WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((json,)) => Self::parse_json(&json, "provider"),
            None => Err(StoreError::NotFound(format!("provider '{}'", id))),
        }
    }

    async fn upsert_provider(&self, id: &str, config: ProviderConfig) -> StoreResult<()> {
        let json = Self::to_json(&config)?;
        sqlx::query(
            "INSERT INTO prism_providers (id, config) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET config = $2",
        )
        .bind(id)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn delete_provider(&self, id: &str) -> StoreResult<()> {
        sqlx::query("DELETE FROM prism_providers WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    // Accounts
    async fn list_accounts(&self) -> StoreResult<Vec<AccountConfig>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT config FROM prism_accounts")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(json,)| Self::parse_json(&json, "account"))
            .collect()
    }

    async fn get_account(&self, id: &str) -> StoreResult<AccountConfig> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT config FROM prism_accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((json,)) => Self::parse_json(&json, "account"),
            None => Err(StoreError::NotFound(format!("account '{}'", id))),
        }
    }

    async fn upsert_account(&self, id: &str, config: AccountConfig) -> StoreResult<()> {
        let json = Self::to_json(&config)?;
        sqlx::query(
            "INSERT INTO prism_accounts (id, config) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET config = $2",
        )
        .bind(id)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn delete_account(&self, id: &str) -> StoreResult<()> {
        sqlx::query("DELETE FROM prism_accounts WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    // Models
    async fn list_models(&self) -> StoreResult<Vec<ModelConfig>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT config FROM prism_models")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(json,)| Self::parse_json(&json, "model"))
            .collect()
    }

    async fn get_model(&self, id: &str) -> StoreResult<ModelConfig> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT config FROM prism_models WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((json,)) => Self::parse_json(&json, "model"),
            None => Err(StoreError::NotFound(format!("model '{}'", id))),
        }
    }

    async fn upsert_model(&self, id: &str, config: ModelConfig) -> StoreResult<()> {
        let json = Self::to_json(&config)?;
        sqlx::query(
            "INSERT INTO prism_models (id, config) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET config = $2",
        )
        .bind(id)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn delete_model(&self, id: &str) -> StoreResult<()> {
        sqlx::query("DELETE FROM prism_models WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    // Variants
    async fn list_variants(&self) -> StoreResult<Vec<VariantConfig>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT config FROM prism_variants")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(json,)| Self::parse_json(&json, "variant"))
            .collect()
    }

    async fn get_variant(&self, id: &str) -> StoreResult<VariantConfig> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT config FROM prism_variants WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((json,)) => Self::parse_json(&json, "variant"),
            None => Err(StoreError::NotFound(format!("variant '{}'", id))),
        }
    }

    async fn upsert_variant(&self, id: &str, config: VariantConfig) -> StoreResult<()> {
        let json = Self::to_json(&config)?;
        sqlx::query(
            "INSERT INTO prism_variants (id, config) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET config = $2",
        )
        .bind(id)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn delete_variant(&self, id: &str) -> StoreResult<()> {
        sqlx::query("DELETE FROM prism_variants WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    // Pools
    async fn list_pools(&self) -> StoreResult<Vec<PoolConfig>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT config FROM prism_pools")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(json,)| Self::parse_json(&json, "pool"))
            .collect()
    }

    async fn upsert_pool(&self, id: &str, config: PoolConfig) -> StoreResult<()> {
        let json = Self::to_json(&config)?;
        sqlx::query(
            "INSERT INTO prism_pools (id, config) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET config = $2",
        )
        .bind(id)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn delete_pool(&self, id: &str) -> StoreResult<()> {
        sqlx::query("DELETE FROM prism_pools WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    // Rate Limits
    async fn list_rate_limits(&self) -> StoreResult<Vec<RateLimitConfig>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT config FROM prism_rate_limits")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(json,)| Self::parse_json(&json, "rate_limit"))
            .collect()
    }

    async fn upsert_rate_limit(&self, name: &str, config: RateLimitConfig) -> StoreResult<()> {
        let json = Self::to_json(&config)?;
        sqlx::query(
            "INSERT INTO prism_rate_limits (name, config) VALUES ($1, $2) ON CONFLICT (name) DO UPDATE SET config = $2",
        )
        .bind(name)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn delete_rate_limit(&self, name: &str) -> StoreResult<()> {
        sqlx::query("DELETE FROM prism_rate_limits WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    // Plugins
    async fn list_plugins(&self) -> StoreResult<Vec<PluginConfig>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT config FROM prism_plugins")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(json,)| Self::parse_json(&json, "plugin"))
            .collect()
    }

    async fn upsert_plugin(&self, name: &str, config: PluginConfig) -> StoreResult<()> {
        let json = Self::to_json(&config)?;
        sqlx::query(
            "INSERT INTO prism_plugins (name, config) VALUES ($1, $2) ON CONFLICT (name) DO UPDATE SET config = $2",
        )
        .bind(name)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn delete_plugin(&self, name: &str) -> StoreResult<()> {
        sqlx::query("DELETE FROM prism_plugins WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    // Client Keys
    async fn list_client_keys(&self) -> StoreResult<Vec<ClientKeyConfig>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT config FROM prism_client_keys")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(json,)| Self::parse_json(&json, "client_key"))
            .collect()
    }

    async fn get_client_key(&self, name: &str) -> StoreResult<ClientKeyConfig> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT config FROM prism_client_keys WHERE name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((json,)) => Self::parse_json(&json, "client_key"),
            None => Err(StoreError::NotFound(format!("client key '{}'", name))),
        }
    }

    async fn upsert_client_key(&self, name: &str, config: ClientKeyConfig) -> StoreResult<()> {
        let json = Self::to_json(&config)?;
        sqlx::query(
            "INSERT INTO prism_client_keys (name, config) VALUES ($1, $2) ON CONFLICT (name) DO UPDATE SET config = $2",
        )
        .bind(name)
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn delete_client_key(&self, name: &str) -> StoreResult<()> {
        sqlx::query("DELETE FROM prism_client_keys WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    // Auth (singleton)
    async fn get_auth(&self) -> StoreResult<AuthConfig> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM prism_settings WHERE key = 'auth'")
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((json,)) => Self::parse_json(&json, "auth"),
            None => Ok(AuthConfig::default()),
        }
    }

    async fn set_auth(&self, config: AuthConfig) -> StoreResult<()> {
        let json = Self::to_json(&config)?;
        sqlx::query(
            "INSERT INTO prism_settings (key, value) VALUES ('auth', $1) ON CONFLICT (key) DO UPDATE SET value = $1",
        )
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    // Access Log (singleton)
    async fn get_access_log(&self) -> StoreResult<AccessLogConfig> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM prism_settings WHERE key = 'access_log'")
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((json,)) => Self::parse_json(&json, "access_log"),
            None => Ok(AccessLogConfig::default()),
        }
    }

    async fn set_access_log(&self, config: AccessLogConfig) -> StoreResult<()> {
        let json = Self::to_json(&config)?;
        sqlx::query(
            "INSERT INTO prism_settings (key, value) VALUES ('access_log', $1) ON CONFLICT (key) DO UPDATE SET value = $1",
        )
        .bind(json)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    // Full config snapshot
    async fn snapshot(&self) -> StoreResult<PrismConfig> {
        let providers = self.list_providers().await?.into_iter().collect();
        let accounts = self.list_accounts().await?;
        let models = self.list_models().await?;
        let variants = self.list_variants().await?;
        let pools = self.list_pools().await?;
        let rate_limits = self.list_rate_limits().await?;
        let plugins = self.list_plugins().await?;
        let client_keys = self.list_client_keys().await?;
        let auth = self.get_auth().await?;
        let access_log = self.get_access_log().await?;

        Ok(PrismConfig {
            server: ServerConfig::default(),
            providers,
            models,
            variants,
            pools,
            routing: RoutingConfig::default(),
            rate_limits,
            plugins,
            discoveries: Vec::new(),
            codec: CodecConfig::default(),
            access_log,
            auth,
            concurrency_limits: Vec::new(),
            accounts,
            client_keys,
        })
    }

    // Bulk import
    async fn import_config(&self, config: PrismConfig) -> StoreResult<()> {
        sqlx::query("DELETE FROM prism_providers")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_accounts")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_models")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_variants")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_pools")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_rate_limits")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_plugins")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_client_keys")
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        for (id, provider) in &config.providers {
            self.upsert_provider(id, provider.clone()).await?;
        }
        for account in &config.accounts {
            self.upsert_account(&account.id, account.clone()).await?;
        }
        for model in &config.models {
            self.upsert_model(&model.id, model.clone()).await?;
        }
        for variant in &config.variants {
            self.upsert_variant(&variant.id, variant.clone()).await?;
        }
        for pool in &config.pools {
            self.upsert_pool(&pool.id, pool.clone()).await?;
        }
        for rule in &config.rate_limits {
            self.upsert_rate_limit(&rule.name, rule.clone()).await?;
        }
        for plugin in &config.plugins {
            self.upsert_plugin(&plugin.name, plugin.clone()).await?;
        }
        for client_key in &config.client_keys {
            self.upsert_client_key(&client_key.name, client_key.clone())
                .await?;
        }
        self.set_auth(config.auth).await?;
        self.set_access_log(config.access_log).await?;
        Ok(())
    }

    // Clusters
    async fn list_clusters(&self) -> StoreResult<Vec<Cluster>> {
        let rows: Vec<(String, String, String, String, String)> =
            sqlx::query_as("SELECT id, name, COALESCE(description, ''), COALESCE(tags, '{}'), created_at FROM prism_clusters ORDER BY created_at")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|(id, name, description, tags, created_at)| {
                let tags: std::collections::HashMap<String, String> =
                    serde_json::from_str(&tags).unwrap_or_default();
                let created_at = chrono::DateTime::parse_from_rfc3339(&created_at)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());
                Cluster {
                    id,
                    name,
                    description,
                    tags,
                    created_at,
                }
            })
            .collect())
    }

    async fn get_cluster(&self, id: &str) -> StoreResult<Cluster> {
        let row: Option<(String, String, String, String, String)> =
            sqlx::query_as("SELECT id, name, COALESCE(description, ''), COALESCE(tags, '{}'), created_at FROM prism_clusters WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((id, name, description, tags, created_at)) => {
                let tags: std::collections::HashMap<String, String> =
                    serde_json::from_str(&tags).unwrap_or_default();
                let created_at = chrono::DateTime::parse_from_rfc3339(&created_at)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());
                Ok(Cluster {
                    id,
                    name,
                    description,
                    tags,
                    created_at,
                })
            }
            None => Err(StoreError::NotFound(format!("cluster '{}'", id))),
        }
    }

    async fn upsert_cluster(&self, id: &str, cluster: Cluster) -> StoreResult<()> {
        let tags = Self::to_json(&cluster.tags)?;
        let created_at = cluster.created_at.to_rfc3339();
        sqlx::query(
            "INSERT INTO prism_clusters (id, name, description, tags, created_at) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (id) DO UPDATE SET name = $2, description = $3, tags = $4",
        )
        .bind(id)
        .bind(&cluster.name)
        .bind(&cluster.description)
        .bind(tags)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn delete_cluster(&self, id: &str) -> StoreResult<()> {
        let count: (i32,) =
            sqlx::query_as("SELECT COUNT(*) FROM prism_proxies WHERE cluster_id = $1")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        if count.0 > 0 {
            return Err(StoreError::Conflict(format!(
                "cluster '{}' still has {} proxies",
                id, count.0
            )));
        }
        sqlx::query("DELETE FROM prism_clusters WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    // Proxy instances
    async fn list_proxies(&self, cluster_id: Option<&str>) -> StoreResult<Vec<ProxyInstance>> {
        let rows: Vec<(String, String, String, String, String, String, String, i64, String)> = match cluster_id {
            Some(cid) => {
                sqlx::query_as(
                    "SELECT instance_id, cluster_id, address, version, COALESCE(status, 'online'), last_heartbeat, registered_at, COALESCE(config_version, 0), COALESCE(metadata, '{}') FROM prism_proxies WHERE cluster_id = $1 ORDER BY registered_at"
                )
                .bind(cid)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?
            }
            None => {
                sqlx::query_as(
                    "SELECT instance_id, cluster_id, address, version, COALESCE(status, 'online'), last_heartbeat, registered_at, COALESCE(config_version, 0), COALESCE(metadata, '{}') FROM prism_proxies ORDER BY registered_at"
                )
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?
            }
        };
        Ok(rows
            .into_iter()
            .map(
                |(
                    instance_id,
                    cluster_id,
                    address,
                    version,
                    status,
                    last_heartbeat,
                    registered_at,
                    config_version,
                    metadata,
                )| {
                    let status = match status.as_str() {
                        "offline" => ProxyStatus::Offline,
                        "degraded" => ProxyStatus::Degraded,
                        _ => ProxyStatus::Online,
                    };
                    let metadata: std::collections::HashMap<String, String> =
                        serde_json::from_str(&metadata).unwrap_or_default();
                    let last_heartbeat = chrono::DateTime::parse_from_rfc3339(&last_heartbeat)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now());
                    let registered_at = chrono::DateTime::parse_from_rfc3339(&registered_at)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now());
                    ProxyInstance {
                        instance_id,
                        cluster_id,
                        address,
                        version,
                        status,
                        last_heartbeat,
                        registered_at,
                        config_version: config_version as u64,
                        metadata,
                    }
                },
            )
            .collect())
    }

    async fn register_proxy(&self, proxy: ProxyInstance) -> StoreResult<()> {
        let status_str = match &proxy.status {
            ProxyStatus::Online => "online",
            ProxyStatus::Offline => "offline",
            ProxyStatus::Degraded => "degraded",
        };
        let metadata = Self::to_json(&proxy.metadata)?;
        let last_heartbeat = proxy.last_heartbeat.to_rfc3339();
        let registered_at = proxy.registered_at.to_rfc3339();
        sqlx::query(
            "INSERT INTO prism_proxies (instance_id, cluster_id, address, version, status, last_heartbeat, registered_at, config_version, metadata) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (instance_id) DO UPDATE SET cluster_id = $2, address = $3, version = $4, status = $5, last_heartbeat = $6, config_version = $8, metadata = $9",
        )
        .bind(&proxy.instance_id)
        .bind(&proxy.cluster_id)
        .bind(&proxy.address)
        .bind(&proxy.version)
        .bind(status_str)
        .bind(last_heartbeat)
        .bind(registered_at)
        .bind(proxy.config_version as i64)
        .bind(metadata)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn heartbeat_proxy(&self, instance_id: &str, config_version: u64) -> StoreResult<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE prism_proxies SET last_heartbeat = $2, config_version = $3, status = 'online' WHERE instance_id = $1",
        )
        .bind(instance_id)
        .bind(now)
        .bind(config_version as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("proxy '{}'", instance_id)));
        }
        Ok(())
    }

    async fn unregister_proxy(&self, instance_id: &str) -> StoreResult<()> {
        sqlx::query("DELETE FROM prism_proxies WHERE instance_id = $1")
            .bind(instance_id)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn cluster_config(&self, _cluster_id: &str) -> StoreResult<PrismConfig> {
        self.snapshot().await
    }
}
