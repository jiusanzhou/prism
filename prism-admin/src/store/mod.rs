use async_trait::async_trait;
use prism_router::config::types::*;
use std::fmt;

pub mod file;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "sqlite")]
pub mod sqlite;

#[derive(Debug)]
pub enum StoreError {
    NotFound(String),
    Conflict(String),
    Internal(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::NotFound(msg) => write!(f, "not found: {}", msg),
            StoreError::Conflict(msg) => write!(f, "conflict: {}", msg),
            StoreError::Internal(msg) => write!(f, "internal error: {}", msg),
        }
    }
}

impl std::error::Error for StoreError {}

pub type StoreResult<T> = Result<T, StoreError>;

#[async_trait]
pub trait Store: Send + Sync {
    // Providers
    async fn list_providers(&self) -> StoreResult<Vec<(String, ProviderConfig)>>;
    async fn get_provider(&self, id: &str) -> StoreResult<ProviderConfig>;
    async fn upsert_provider(&self, id: &str, config: ProviderConfig) -> StoreResult<()>;
    async fn delete_provider(&self, id: &str) -> StoreResult<()>;

    // Accounts
    async fn list_accounts(&self) -> StoreResult<Vec<AccountConfig>>;
    async fn get_account(&self, id: &str) -> StoreResult<AccountConfig>;
    async fn upsert_account(&self, id: &str, config: AccountConfig) -> StoreResult<()>;
    async fn delete_account(&self, id: &str) -> StoreResult<()>;

    // Models
    async fn list_models(&self) -> StoreResult<Vec<ModelConfig>>;
    async fn get_model(&self, id: &str) -> StoreResult<ModelConfig>;
    async fn upsert_model(&self, id: &str, config: ModelConfig) -> StoreResult<()>;
    async fn delete_model(&self, id: &str) -> StoreResult<()>;

    // Variants
    async fn list_variants(&self) -> StoreResult<Vec<VariantConfig>>;
    async fn get_variant(&self, id: &str) -> StoreResult<VariantConfig>;
    async fn upsert_variant(&self, id: &str, config: VariantConfig) -> StoreResult<()>;
    async fn delete_variant(&self, id: &str) -> StoreResult<()>;

    // Pools
    async fn list_pools(&self) -> StoreResult<Vec<PoolConfig>>;
    async fn upsert_pool(&self, id: &str, config: PoolConfig) -> StoreResult<()>;
    async fn delete_pool(&self, id: &str) -> StoreResult<()>;

    // Rate Limits
    async fn list_rate_limits(&self) -> StoreResult<Vec<RateLimitConfig>>;
    async fn upsert_rate_limit(&self, name: &str, config: RateLimitConfig) -> StoreResult<()>;
    async fn delete_rate_limit(&self, name: &str) -> StoreResult<()>;

    // Plugins
    async fn list_plugins(&self) -> StoreResult<Vec<PluginConfig>>;
    async fn upsert_plugin(&self, name: &str, config: PluginConfig) -> StoreResult<()>;
    async fn delete_plugin(&self, name: &str) -> StoreResult<()>;

    // Auth config (singleton)
    async fn get_auth(&self) -> StoreResult<AuthConfig>;
    async fn set_auth(&self, config: AuthConfig) -> StoreResult<()>;

    // Access Log config (singleton)
    async fn get_access_log(&self) -> StoreResult<AccessLogConfig>;
    async fn set_access_log(&self, config: AccessLogConfig) -> StoreResult<()>;

    // Full config snapshot
    async fn snapshot(&self) -> StoreResult<PrismConfig>;

    // Bulk import from a full config (for PUT /api/config)
    async fn import_config(&self, config: PrismConfig) -> StoreResult<()>;
}
