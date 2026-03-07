use async_trait::async_trait;
use prism_router::config::types::*;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::RwLock;

use crate::cluster::{Cluster, ProxyInstance, ProxyStatus};
use super::{Store, StoreError, StoreResult};

/// Supported configuration file formats for FileStore.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileFormat {
    Yaml,
    Json,
    Toml,
    /// Auto-detect from file extension.
    Auto,
}

impl FileFormat {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "yaml" | "yml" => Some(Self::Yaml),
            "json" | "jsonc" => Some(Self::Json),
            "toml" => Some(Self::Toml),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    /// Detect format from file extension.
    pub fn from_extension(path: &std::path::Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some("json") | Some("jsonc") => Self::Json,
            Some("toml") => Self::Toml,
            Some("yaml") | Some("yml") => Self::Yaml,
            _ => Self::Yaml, // default fallback
        }
    }
}

impl std::fmt::Display for FileFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Yaml => write!(f, "yaml"),
            Self::Json => write!(f, "json"),
            Self::Toml => write!(f, "toml"),
            Self::Auto => write!(f, "auto"),
        }
    }
}

/// File-based config store. Supports YAML, JSON, and TOML formats.
/// Clusters and proxies are kept in-memory only (not persisted to the config file).
pub struct FileStore {
    path: PathBuf,
    format: FileFormat,
    lock: RwLock<()>,
    clusters: RwLock<HashMap<String, Cluster>>,
    proxies: RwLock<HashMap<String, ProxyInstance>>,
}

impl FileStore {
    pub fn new(path: PathBuf, format: FileFormat) -> Self {
        let resolved = if format == FileFormat::Auto {
            FileFormat::from_extension(&path)
        } else {
            format
        };
        Self {
            path,
            format: resolved,
            lock: RwLock::new(()),
            clusters: RwLock::new(HashMap::new()),
            proxies: RwLock::new(HashMap::new()),
        }
    }

    /// Resolved format (never Auto).
    pub fn format(&self) -> FileFormat {
        self.format
    }

    async fn load(&self) -> StoreResult<PrismConfig> {
        let path = self.path.clone();
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| StoreError::Internal(format!("read {}: {}", path.display(), e)))?;

        match self.format {
            FileFormat::Yaml | FileFormat::Auto => {
                serde_yaml::from_str(&content)
                    .map_err(|e| StoreError::Internal(format!("parse YAML: {}", e)))
            }
            FileFormat::Json => {
                let stripped = strip_jsonc_comments(&content);
                serde_json::from_str(&stripped)
                    .map_err(|e| StoreError::Internal(format!("parse JSON: {}", e)))
            }
            FileFormat::Toml => {
                toml::from_str(&content)
                    .map_err(|e| StoreError::Internal(format!("parse TOML: {}", e)))
            }
        }
    }

    async fn save(&self, config: &PrismConfig) -> StoreResult<()> {
        let serialized = match self.format {
            FileFormat::Yaml | FileFormat::Auto => {
                serde_yaml::to_string(config)
                    .map_err(|e| StoreError::Internal(format!("serialize YAML: {}", e)))?
            }
            FileFormat::Json => {
                serde_json::to_string_pretty(config)
                    .map_err(|e| StoreError::Internal(format!("serialize JSON: {}", e)))?
            }
            FileFormat::Toml => {
                toml::to_string_pretty(config)
                    .map_err(|e| StoreError::Internal(format!("serialize TOML: {}", e)))?
            }
        };
        let path = self.path.clone();
        tokio::fs::write(&path, serialized)
            .await
            .map_err(|e| StoreError::Internal(format!("write {}: {}", path.display(), e)))
    }

    async fn modify<F>(&self, f: F) -> StoreResult<()>
    where
        F: FnOnce(&mut PrismConfig),
    {
        let _guard = self.lock.write().await;
        let mut config = self.load().await?;
        f(&mut config);
        self.save(&config).await
    }
}

/// Strip single-line (//) and multi-line (/* */) comments for JSONC support.
fn strip_jsonc_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;

    while let Some(c) = chars.next() {
        if in_string {
            result.push(c);
            if c == '\\' {
                if let Some(&next) = chars.peek() {
                    result.push(next);
                    chars.next();
                }
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            result.push(c);
        } else if c == '/' {
            match chars.peek() {
                Some(&'/') => {
                    chars.next();
                    for nc in chars.by_ref() {
                        if nc == '\n' {
                            result.push('\n');
                            break;
                        }
                    }
                }
                Some(&'*') => {
                    chars.next();
                    loop {
                        match chars.next() {
                            Some('*') if chars.peek() == Some(&'/') => {
                                chars.next();
                                break;
                            }
                            Some('\n') => result.push('\n'),
                            None => break,
                            _ => {}
                        }
                    }
                }
                _ => result.push(c),
            }
        } else {
            result.push(c);
        }
    }
    result
}

#[async_trait]
impl Store for FileStore {
    // Providers
    async fn list_providers(&self) -> StoreResult<Vec<(String, ProviderConfig)>> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        Ok(config.providers.into_iter().collect())
    }

    async fn get_provider(&self, id: &str) -> StoreResult<ProviderConfig> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        config
            .providers
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("provider '{}'", id)))
    }

    async fn upsert_provider(&self, id: &str, provider: ProviderConfig) -> StoreResult<()> {
        let id = id.to_string();
        self.modify(|config| {
            config.providers.insert(id, provider);
        })
        .await
    }

    async fn delete_provider(&self, id: &str) -> StoreResult<()> {
        let id = id.to_string();
        self.modify(|config| {
            config.providers.remove(&id);
        })
        .await
    }

    // Accounts
    async fn list_accounts(&self) -> StoreResult<Vec<AccountConfig>> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        Ok(config.accounts)
    }

    async fn get_account(&self, id: &str) -> StoreResult<AccountConfig> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        config
            .accounts
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| StoreError::NotFound(format!("account '{}'", id)))
    }

    async fn upsert_account(&self, id: &str, account: AccountConfig) -> StoreResult<()> {
        let id = id.to_string();
        self.modify(|config| {
            if let Some(pos) = config.accounts.iter().position(|a| a.id == id) {
                config.accounts[pos] = account;
            } else {
                config.accounts.push(account);
            }
        })
        .await
    }

    async fn delete_account(&self, id: &str) -> StoreResult<()> {
        let id = id.to_string();
        self.modify(|config| {
            config.accounts.retain(|a| a.id != id);
        })
        .await
    }

    // Models
    async fn list_models(&self) -> StoreResult<Vec<ModelConfig>> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        Ok(config.models)
    }

    async fn get_model(&self, id: &str) -> StoreResult<ModelConfig> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        config
            .models
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| StoreError::NotFound(format!("model '{}'", id)))
    }

    async fn upsert_model(&self, id: &str, model: ModelConfig) -> StoreResult<()> {
        let id = id.to_string();
        self.modify(|config| {
            if let Some(pos) = config.models.iter().position(|m| m.id == id) {
                config.models[pos] = model;
            } else {
                config.models.push(model);
            }
        })
        .await
    }

    async fn delete_model(&self, id: &str) -> StoreResult<()> {
        let id = id.to_string();
        self.modify(|config| {
            config.models.retain(|m| m.id != id);
        })
        .await
    }

    // Variants
    async fn list_variants(&self) -> StoreResult<Vec<VariantConfig>> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        Ok(config.variants)
    }

    async fn get_variant(&self, id: &str) -> StoreResult<VariantConfig> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        config
            .variants
            .into_iter()
            .find(|v| v.id == id)
            .ok_or_else(|| StoreError::NotFound(format!("variant '{}'", id)))
    }

    async fn upsert_variant(&self, id: &str, variant: VariantConfig) -> StoreResult<()> {
        let id = id.to_string();
        self.modify(|config| {
            if let Some(pos) = config.variants.iter().position(|v| v.id == id) {
                config.variants[pos] = variant;
            } else {
                config.variants.push(variant);
            }
        })
        .await
    }

    async fn delete_variant(&self, id: &str) -> StoreResult<()> {
        let id = id.to_string();
        self.modify(|config| {
            config.variants.retain(|v| v.id != id);
        })
        .await
    }

    // Pools
    async fn list_pools(&self) -> StoreResult<Vec<PoolConfig>> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        Ok(config.pools)
    }

    async fn upsert_pool(&self, id: &str, pool: PoolConfig) -> StoreResult<()> {
        let id = id.to_string();
        self.modify(|config| {
            if let Some(pos) = config.pools.iter().position(|p| p.id == id) {
                config.pools[pos] = pool;
            } else {
                config.pools.push(pool);
            }
        })
        .await
    }

    async fn delete_pool(&self, id: &str) -> StoreResult<()> {
        let id = id.to_string();
        self.modify(|config| {
            config.pools.retain(|p| p.id != id);
        })
        .await
    }

    // Rate Limits
    async fn list_rate_limits(&self) -> StoreResult<Vec<RateLimitConfig>> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        Ok(config.rate_limits)
    }

    async fn upsert_rate_limit(&self, name: &str, rule: RateLimitConfig) -> StoreResult<()> {
        let name = name.to_string();
        self.modify(|config| {
            if let Some(pos) = config.rate_limits.iter().position(|r| r.name == name) {
                config.rate_limits[pos] = rule;
            } else {
                config.rate_limits.push(rule);
            }
        })
        .await
    }

    async fn delete_rate_limit(&self, name: &str) -> StoreResult<()> {
        let name = name.to_string();
        self.modify(|config| {
            config.rate_limits.retain(|r| r.name != name);
        })
        .await
    }

    // Plugins
    async fn list_plugins(&self) -> StoreResult<Vec<PluginConfig>> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        Ok(config.plugins)
    }

    async fn upsert_plugin(&self, name: &str, plugin: PluginConfig) -> StoreResult<()> {
        let name = name.to_string();
        self.modify(|config| {
            if let Some(pos) = config.plugins.iter().position(|p| p.name == name) {
                config.plugins[pos] = plugin;
            } else {
                config.plugins.push(plugin);
            }
        })
        .await
    }

    async fn delete_plugin(&self, name: &str) -> StoreResult<()> {
        let name = name.to_string();
        self.modify(|config| {
            config.plugins.retain(|p| p.name != name);
        })
        .await
    }

    // Auth config (singleton)
    async fn get_auth(&self) -> StoreResult<AuthConfig> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        Ok(config.auth)
    }

    async fn set_auth(&self, auth: AuthConfig) -> StoreResult<()> {
        self.modify(|config| {
            config.auth = auth;
        })
        .await
    }

    // Access Log config (singleton)
    async fn get_access_log(&self) -> StoreResult<AccessLogConfig> {
        let _guard = self.lock.read().await;
        let config = self.load().await?;
        Ok(config.access_log)
    }

    async fn set_access_log(&self, access_log: AccessLogConfig) -> StoreResult<()> {
        self.modify(|config| {
            config.access_log = access_log;
        })
        .await
    }

    // Full config snapshot
    async fn snapshot(&self) -> StoreResult<PrismConfig> {
        let _guard = self.lock.read().await;
        self.load().await
    }

    // Bulk import
    async fn import_config(&self, config: PrismConfig) -> StoreResult<()> {
        let _guard = self.lock.write().await;
        self.save(&config).await
    }

    // Clusters (in-memory)
    async fn list_clusters(&self) -> StoreResult<Vec<Cluster>> {
        let clusters = self.clusters.read().await;
        Ok(clusters.values().cloned().collect())
    }

    async fn get_cluster(&self, id: &str) -> StoreResult<Cluster> {
        let clusters = self.clusters.read().await;
        clusters
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(format!("cluster '{}'", id)))
    }

    async fn upsert_cluster(&self, id: &str, cluster: Cluster) -> StoreResult<()> {
        let mut clusters = self.clusters.write().await;
        clusters.insert(id.to_string(), cluster);
        Ok(())
    }

    async fn delete_cluster(&self, id: &str) -> StoreResult<()> {
        let proxies = self.proxies.read().await;
        let has_proxies = proxies.values().any(|p| p.cluster_id == id);
        if has_proxies {
            return Err(StoreError::Conflict(format!(
                "cluster '{}' still has proxies",
                id
            )));
        }
        drop(proxies);

        let mut clusters = self.clusters.write().await;
        clusters.remove(id);
        Ok(())
    }

    // Proxy instances (in-memory)
    async fn list_proxies(&self, cluster_id: Option<&str>) -> StoreResult<Vec<ProxyInstance>> {
        let proxies = self.proxies.read().await;
        match cluster_id {
            Some(cid) => Ok(proxies.values().filter(|p| p.cluster_id == cid).cloned().collect()),
            None => Ok(proxies.values().cloned().collect()),
        }
    }

    async fn register_proxy(&self, proxy: ProxyInstance) -> StoreResult<()> {
        let clusters = self.clusters.read().await;
        if !clusters.contains_key(&proxy.cluster_id) {
            return Err(StoreError::NotFound(format!(
                "cluster '{}'",
                proxy.cluster_id
            )));
        }
        drop(clusters);

        let mut proxies = self.proxies.write().await;
        proxies.insert(proxy.instance_id.clone(), proxy);
        Ok(())
    }

    async fn heartbeat_proxy(&self, instance_id: &str, config_version: u64) -> StoreResult<()> {
        let mut proxies = self.proxies.write().await;
        match proxies.get_mut(instance_id) {
            Some(proxy) => {
                proxy.last_heartbeat = chrono::Utc::now();
                proxy.config_version = config_version;
                proxy.status = ProxyStatus::Online;
                Ok(())
            }
            None => Err(StoreError::NotFound(format!("proxy '{}'", instance_id))),
        }
    }

    async fn unregister_proxy(&self, instance_id: &str) -> StoreResult<()> {
        let mut proxies = self.proxies.write().await;
        proxies.remove(instance_id);
        Ok(())
    }

    // File store has no scoping — returns the global config snapshot
    async fn cluster_config(&self, _cluster_id: &str) -> StoreResult<PrismConfig> {
        self.snapshot().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_jsonc_single_line() {
        let input = r#"{
  // This is a comment
  "key": "value"
}"#;
        let stripped = strip_jsonc_comments(input);
        assert!(!stripped.contains("//"));
        assert!(stripped.contains(r#""key": "value""#));
    }

    #[test]
    fn test_strip_jsonc_multi_line() {
        let input = r#"{
  /* multi
     line
     comment */
  "key": "value"
}"#;
        let stripped = strip_jsonc_comments(input);
        assert!(!stripped.contains("/*"));
        assert!(stripped.contains(r#""key": "value""#));
    }

    #[test]
    fn test_strip_jsonc_in_string() {
        let input = r#"{"url": "http://example.com/path"}"#;
        let stripped = strip_jsonc_comments(input);
        assert_eq!(stripped, input);
    }

    #[test]
    fn test_file_format_from_extension() {
        assert_eq!(FileFormat::from_extension(std::path::Path::new("c.yaml")), FileFormat::Yaml);
        assert_eq!(FileFormat::from_extension(std::path::Path::new("c.yml")), FileFormat::Yaml);
        assert_eq!(FileFormat::from_extension(std::path::Path::new("c.json")), FileFormat::Json);
        assert_eq!(FileFormat::from_extension(std::path::Path::new("c.jsonc")), FileFormat::Json);
        assert_eq!(FileFormat::from_extension(std::path::Path::new("c.toml")), FileFormat::Toml);
        assert_eq!(FileFormat::from_extension(std::path::Path::new("c.txt")), FileFormat::Yaml);
    }

    #[test]
    fn test_file_format_from_str() {
        assert_eq!(FileFormat::from_str_opt("yaml"), Some(FileFormat::Yaml));
        assert_eq!(FileFormat::from_str_opt("yml"), Some(FileFormat::Yaml));
        assert_eq!(FileFormat::from_str_opt("json"), Some(FileFormat::Json));
        assert_eq!(FileFormat::from_str_opt("jsonc"), Some(FileFormat::Json));
        assert_eq!(FileFormat::from_str_opt("toml"), Some(FileFormat::Toml));
        assert_eq!(FileFormat::from_str_opt("auto"), Some(FileFormat::Auto));
        assert_eq!(FileFormat::from_str_opt("xml"), None);
    }
}
