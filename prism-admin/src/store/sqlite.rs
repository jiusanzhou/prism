use async_trait::async_trait;
use prism_router::config::types::*;
use sqlx::SqlitePool;

use super::{Store, StoreError, StoreResult};

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
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS prism_providers (
                id TEXT PRIMARY KEY,
                config TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(format!("migration: {}", e)))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS prism_accounts (
                id TEXT PRIMARY KEY,
                config TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(format!("migration: {}", e)))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS prism_models (
                id TEXT PRIMARY KEY,
                config TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(format!("migration: {}", e)))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS prism_variants (
                id TEXT PRIMARY KEY,
                config TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(format!("migration: {}", e)))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS prism_pools (
                id TEXT PRIMARY KEY,
                config TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(format!("migration: {}", e)))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS prism_rate_limits (
                name TEXT PRIMARY KEY,
                config TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(format!("migration: {}", e)))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS prism_plugins (
                name TEXT PRIMARY KEY,
                config TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(format!("migration: {}", e)))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS prism_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(format!("migration: {}", e)))?;

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
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, config FROM prism_providers")
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
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT config FROM prism_accounts")
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
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT config FROM prism_models")
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
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT config FROM prism_variants")
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
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT config FROM prism_pools")
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
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT config FROM prism_rate_limits")
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
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT config FROM prism_plugins")
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
        })
    }

    // Bulk import
    async fn import_config(&self, config: PrismConfig) -> StoreResult<()> {
        // Clear all tables
        sqlx::query("DELETE FROM prism_providers").execute(&self.pool).await.map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_accounts").execute(&self.pool).await.map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_models").execute(&self.pool).await.map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_variants").execute(&self.pool).await.map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_pools").execute(&self.pool).await.map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_rate_limits").execute(&self.pool).await.map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query("DELETE FROM prism_plugins").execute(&self.pool).await.map_err(|e| StoreError::Internal(e.to_string()))?;

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
        self.set_auth(config.auth).await?;
        self.set_access_log(config.access_log).await?;
        Ok(())
    }
}
