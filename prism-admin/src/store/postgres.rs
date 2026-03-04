use async_trait::async_trait;
use prism_router::config::types::*;
use sqlx::PgPool;

use super::{Store, StoreError, StoreResult};

pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub async fn new(database_url: &str) -> StoreResult<Self> {
        let pool = PgPool::connect(database_url)
            .await
            .map_err(|e| StoreError::Internal(format!("pg connect: {}", e)))?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> StoreResult<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS prism_providers (
                id TEXT PRIMARY KEY,
                config JSONB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prism_accounts (
                id TEXT PRIMARY KEY,
                config JSONB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prism_models (
                id TEXT PRIMARY KEY,
                config JSONB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prism_variants (
                id TEXT PRIMARY KEY,
                config JSONB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prism_pools (
                id TEXT PRIMARY KEY,
                config JSONB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prism_rate_limits (
                name TEXT PRIMARY KEY,
                config JSONB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prism_plugins (
                name TEXT PRIMARY KEY,
                config JSONB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prism_settings (
                key TEXT PRIMARY KEY,
                value JSONB NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(format!("migration: {}", e)))?;
        Ok(())
    }
}

#[async_trait]
impl Store for PostgresStore {
    // Providers
    async fn list_providers(&self) -> StoreResult<Vec<(String, ProviderConfig)>> {
        let rows: Vec<(String, serde_json::Value)> =
            sqlx::query_as("SELECT id, config FROM prism_providers")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(id, val)| {
                let config: ProviderConfig = serde_json::from_value(val)
                    .map_err(|e| StoreError::Internal(format!("deserialize provider '{}': {}", id, e)))?;
                Ok((id, config))
            })
            .collect()
    }

    async fn get_provider(&self, id: &str) -> StoreResult<ProviderConfig> {
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT config FROM prism_providers WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((val,)) => serde_json::from_value(val)
                .map_err(|e| StoreError::Internal(format!("deserialize provider: {}", e))),
            None => Err(StoreError::NotFound(format!("provider '{}'", id))),
        }
    }

    async fn upsert_provider(&self, id: &str, config: ProviderConfig) -> StoreResult<()> {
        let val = serde_json::to_value(&config)
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO prism_providers (id, config) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET config = $2",
        )
        .bind(id)
        .bind(val)
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
        let rows: Vec<(serde_json::Value,)> =
            sqlx::query_as("SELECT config FROM prism_accounts")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(val,)| {
                serde_json::from_value(val)
                    .map_err(|e| StoreError::Internal(format!("deserialize account: {}", e)))
            })
            .collect()
    }

    async fn get_account(&self, id: &str) -> StoreResult<AccountConfig> {
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT config FROM prism_accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((val,)) => serde_json::from_value(val)
                .map_err(|e| StoreError::Internal(format!("deserialize account: {}", e))),
            None => Err(StoreError::NotFound(format!("account '{}'", id))),
        }
    }

    async fn upsert_account(&self, id: &str, config: AccountConfig) -> StoreResult<()> {
        let val = serde_json::to_value(&config)
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO prism_accounts (id, config) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET config = $2",
        )
        .bind(id)
        .bind(val)
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
        let rows: Vec<(serde_json::Value,)> =
            sqlx::query_as("SELECT config FROM prism_models")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(val,)| {
                serde_json::from_value(val)
                    .map_err(|e| StoreError::Internal(format!("deserialize model: {}", e)))
            })
            .collect()
    }

    async fn get_model(&self, id: &str) -> StoreResult<ModelConfig> {
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT config FROM prism_models WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((val,)) => serde_json::from_value(val)
                .map_err(|e| StoreError::Internal(format!("deserialize model: {}", e))),
            None => Err(StoreError::NotFound(format!("model '{}'", id))),
        }
    }

    async fn upsert_model(&self, id: &str, config: ModelConfig) -> StoreResult<()> {
        let val = serde_json::to_value(&config)
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO prism_models (id, config) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET config = $2",
        )
        .bind(id)
        .bind(val)
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
        let rows: Vec<(serde_json::Value,)> =
            sqlx::query_as("SELECT config FROM prism_variants")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(val,)| {
                serde_json::from_value(val)
                    .map_err(|e| StoreError::Internal(format!("deserialize variant: {}", e)))
            })
            .collect()
    }

    async fn get_variant(&self, id: &str) -> StoreResult<VariantConfig> {
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT config FROM prism_variants WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((val,)) => serde_json::from_value(val)
                .map_err(|e| StoreError::Internal(format!("deserialize variant: {}", e))),
            None => Err(StoreError::NotFound(format!("variant '{}'", id))),
        }
    }

    async fn upsert_variant(&self, id: &str, config: VariantConfig) -> StoreResult<()> {
        let val = serde_json::to_value(&config)
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO prism_variants (id, config) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET config = $2",
        )
        .bind(id)
        .bind(val)
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
        let rows: Vec<(serde_json::Value,)> =
            sqlx::query_as("SELECT config FROM prism_pools")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(val,)| {
                serde_json::from_value(val)
                    .map_err(|e| StoreError::Internal(format!("deserialize pool: {}", e)))
            })
            .collect()
    }

    async fn upsert_pool(&self, id: &str, config: PoolConfig) -> StoreResult<()> {
        let val = serde_json::to_value(&config)
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO prism_pools (id, config) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET config = $2",
        )
        .bind(id)
        .bind(val)
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
        let rows: Vec<(serde_json::Value,)> =
            sqlx::query_as("SELECT config FROM prism_rate_limits")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(val,)| {
                serde_json::from_value(val)
                    .map_err(|e| StoreError::Internal(format!("deserialize rate_limit: {}", e)))
            })
            .collect()
    }

    async fn upsert_rate_limit(&self, name: &str, config: RateLimitConfig) -> StoreResult<()> {
        let val = serde_json::to_value(&config)
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO prism_rate_limits (name, config) VALUES ($1, $2) ON CONFLICT (name) DO UPDATE SET config = $2",
        )
        .bind(name)
        .bind(val)
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
        let rows: Vec<(serde_json::Value,)> =
            sqlx::query_as("SELECT config FROM prism_plugins")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        rows.into_iter()
            .map(|(val,)| {
                serde_json::from_value(val)
                    .map_err(|e| StoreError::Internal(format!("deserialize plugin: {}", e)))
            })
            .collect()
    }

    async fn upsert_plugin(&self, name: &str, config: PluginConfig) -> StoreResult<()> {
        let val = serde_json::to_value(&config)
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO prism_plugins (name, config) VALUES ($1, $2) ON CONFLICT (name) DO UPDATE SET config = $2",
        )
        .bind(name)
        .bind(val)
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
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT value FROM prism_settings WHERE key = 'auth'")
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((val,)) => serde_json::from_value(val)
                .map_err(|e| StoreError::Internal(format!("deserialize auth: {}", e))),
            None => Ok(AuthConfig::default()),
        }
    }

    async fn set_auth(&self, config: AuthConfig) -> StoreResult<()> {
        let val = serde_json::to_value(&config)
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO prism_settings (key, value) VALUES ('auth', $1) ON CONFLICT (key) DO UPDATE SET value = $1",
        )
        .bind(val)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Internal(e.to_string()))?;
        Ok(())
    }

    // Access Log (singleton)
    async fn get_access_log(&self) -> StoreResult<AccessLogConfig> {
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as("SELECT value FROM prism_settings WHERE key = 'access_log'")
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| StoreError::Internal(e.to_string()))?;
        match row {
            Some((val,)) => serde_json::from_value(val)
                .map_err(|e| StoreError::Internal(format!("deserialize access_log: {}", e))),
            None => Ok(AccessLogConfig::default()),
        }
    }

    async fn set_access_log(&self, config: AccessLogConfig) -> StoreResult<()> {
        let val = serde_json::to_value(&config)
            .map_err(|e| StoreError::Internal(e.to_string()))?;
        sqlx::query(
            "INSERT INTO prism_settings (key, value) VALUES ('access_log', $1) ON CONFLICT (key) DO UPDATE SET value = $1",
        )
        .bind(val)
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
