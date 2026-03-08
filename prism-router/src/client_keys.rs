//! Client key manager — per-key, per-model quota enforcement for multi-tenant API access.
//!
//! Provides sliding-window QPM, daily token budgets, concurrent request limits,
//! and per-key model access control. Supports per-model quota overrides.
//! All state is in-memory using std atomics.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use crate::config::types::{ClientKeyConfig, ClientKeyQuotaConfig};

/// Shared reference to a resolved client key.
#[derive(Debug, Clone)]
pub struct ClientKeyRef {
    pub name: String,
    pub key: String,
}

/// Errors from client key validation.
#[derive(Debug, Clone)]
pub enum ClientKeyError {
    InvalidKey,
    ModelNotAllowed { model: String },
    RateLimited { retry_after_secs: u64 },
    TokenBudgetExhausted,
    ConcurrencyExhausted { current: u32, limit: u32 },
}

/// Runtime quota state (one per quota scope: global or per-model).
struct QuotaState {
    config: ClientKeyQuotaConfig,
    request_timestamps: RwLock<Vec<Instant>>,
    tokens_today: AtomicU64,
    token_day: AtomicU64,
    concurrent: AtomicU32,
}

impl QuotaState {
    fn new(config: ClientKeyQuotaConfig) -> Self {
        Self {
            config,
            request_timestamps: RwLock::new(Vec::new()),
            tokens_today: AtomicU64::new(0),
            token_day: AtomicU64::new(current_day_utc()),
            concurrent: AtomicU32::new(0),
        }
    }

    fn check_qpm(&self) -> Result<(), ClientKeyError> {
        let limit = self.config.max_requests_per_minute;
        if limit == 0 { return Ok(()); }

        let now = Instant::now();
        let window = Duration::from_secs(60);
        let mut ts = self.request_timestamps.write().unwrap();
        ts.retain(|&t| now.duration_since(t) < window);
        if ts.len() as u64 >= limit {
            let oldest = ts.first().copied().unwrap_or(now);
            let retry = window.checked_sub(now.duration_since(oldest))
                .unwrap_or(Duration::from_secs(1));
            return Err(ClientKeyError::RateLimited { retry_after_secs: retry.as_secs().max(1) });
        }
        ts.push(now);
        Ok(())
    }

    fn check_daily_tokens(&self) -> Result<(), ClientKeyError> {
        let limit = self.config.max_tokens_per_day;
        if limit == 0 { return Ok(()); }
        maybe_reset_daily(&self.tokens_today, &self.token_day);
        if self.tokens_today.load(Ordering::Relaxed) >= limit {
            return Err(ClientKeyError::TokenBudgetExhausted);
        }
        Ok(())
    }

    fn try_increment_concurrent(&self) -> Result<Option<ConcurrencyGuard>, ClientKeyError> {
        let max = self.config.max_concurrent;
        if max == 0 { return Ok(None); }
        let prev = self.concurrent.fetch_add(1, Ordering::Relaxed);
        if prev >= max {
            self.concurrent.fetch_sub(1, Ordering::Relaxed);
            return Err(ClientKeyError::ConcurrencyExhausted { current: prev, limit: max });
        }
        Ok(Some(ConcurrencyGuard { concurrent: &self.concurrent as *const AtomicU32 }))
    }

    fn record_tokens(&self, tokens: u64) {
        if tokens > 0 {
            maybe_reset_daily(&self.tokens_today, &self.token_day);
            self.tokens_today.fetch_add(tokens, Ordering::Relaxed);
        }
    }
}

/// Per-key runtime state.
struct KeyState {
    config: ClientKeyConfig,
    allowed_models: HashSet<String>,
    /// Global (fallback) quota state.
    global_quota: QuotaState,
    /// Per-model quota states. Only models with explicit overrides have entries.
    model_quotas: HashMap<String, QuotaState>,
}

impl KeyState {
    /// Get the appropriate QuotaState for a model (per-model if configured, else global).
    fn quota_for(&self, model: Option<&str>) -> &QuotaState {
        if let Some(m) = model {
            if let Some(mq) = self.model_quotas.get(m) {
                return mq;
            }
        }
        &self.global_quota
    }
}

/// Guard that decrements the concurrent counter on drop.
pub struct ConcurrencyGuard {
    concurrent: *const AtomicU32,
}

unsafe impl Send for ConcurrencyGuard {}
unsafe impl Sync for ConcurrencyGuard {}

impl Drop for ConcurrencyGuard {
    fn drop(&mut self) {
        unsafe { (*self.concurrent).fetch_sub(1, Ordering::Relaxed); }
    }
}

/// Manages per-client-key, per-model quota enforcement.
pub struct ClientKeyManager {
    keys: HashMap<String, KeyState>,
}

impl ClientKeyManager {
    pub fn new(configs: &[ClientKeyConfig]) -> Option<Self> {
        if configs.is_empty() { return None; }
        let mut keys = HashMap::new();
        for cfg in configs {
            let allowed: HashSet<String> = cfg.allowed_models.iter().cloned().collect();
            let model_quotas: HashMap<String, QuotaState> = cfg.model_quotas.iter()
                .map(|(model, q)| (model.clone(), QuotaState::new(q.clone())))
                .collect();
            keys.insert(cfg.key.clone(), KeyState {
                config: cfg.clone(),
                allowed_models: allowed,
                global_quota: QuotaState::new(cfg.quota.clone()),
                model_quotas,
            });
        }
        Some(Self { keys })
    }

    /// Lightweight check: key exists and is enabled. No quota enforcement.
    /// Use this in request_filter before model is known.
    pub fn validate_key_exists(&self, api_key: &str) -> Result<ClientKeyRef, ClientKeyError> {
        let state = self.keys.get(api_key).ok_or(ClientKeyError::InvalidKey)?;
        if !state.config.enabled { return Err(ClientKeyError::InvalidKey); }
        Ok(ClientKeyRef { name: state.config.name.clone(), key: api_key.to_string() })
    }

    /// Full validation: key + model ACL + QPM + daily tokens + concurrent.
    /// Call after model is known (post body-read).
    pub fn validate_key(
        &self,
        api_key: &str,
        model: Option<&str>,
    ) -> Result<(ClientKeyRef, Option<ConcurrencyGuard>), ClientKeyError> {
        let state = self.keys.get(api_key).ok_or(ClientKeyError::InvalidKey)?;
        if !state.config.enabled { return Err(ClientKeyError::InvalidKey); }

        // Model access control
        if let Some(m) = model {
            if !state.allowed_models.is_empty() && !state.allowed_models.contains(m) {
                return Err(ClientKeyError::ModelNotAllowed { model: m.to_string() });
            }
        }

        // Resolve quota scope (per-model or global)
        let quota = state.quota_for(model);
        quota.check_qpm()?;
        quota.check_daily_tokens()?;
        let guard = quota.try_increment_concurrent()?;

        Ok((ClientKeyRef { name: state.config.name.clone(), key: api_key.to_string() }, guard))
    }

    /// Record token usage for the appropriate quota scope.
    pub fn record_usage(&self, api_key: &str, model: Option<&str>, tokens_used: u64) {
        if tokens_used == 0 { return; }
        if let Some(state) = self.keys.get(api_key) {
            state.quota_for(model).record_tokens(tokens_used);
        }
    }

    /// Check if a model is allowed for the given key.
    pub fn check_model_access(&self, api_key: &str, model: &str) -> Result<(), ClientKeyError> {
        if let Some(state) = self.keys.get(api_key) {
            if !state.allowed_models.is_empty() && !state.allowed_models.contains(model) {
                return Err(ClientKeyError::ModelNotAllowed { model: model.to_string() });
            }
        }
        Ok(())
    }
}

fn current_day_utc() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() / 86400
}

fn maybe_reset_daily(tokens: &AtomicU64, day: &AtomicU64) {
    let today = current_day_utc();
    let stored = day.load(Ordering::Relaxed);
    if today != stored {
        if day.compare_exchange(stored, today, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            tokens.store(0, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{ClientKeyConfig, ClientKeyQuotaConfig};

    fn make_config(key: &str, name: &str) -> ClientKeyConfig {
        ClientKeyConfig {
            key: key.to_string(),
            name: name.to_string(),
            enabled: true,
            allowed_models: vec!["gpt-4o".into(), "claude-sonnet-4".into()],
            quota: ClientKeyQuotaConfig {
                max_requests_per_minute: 10,
                max_tokens_per_day: 100_000,
                max_concurrent: 2,
            },
            model_quotas: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn valid_key_passes() {
        let mgr = ClientKeyManager::new(&[make_config("sk-test", "Test")]).unwrap();
        let (r, g) = mgr.validate_key("sk-test", Some("gpt-4o")).unwrap();
        assert_eq!(r.name, "Test");
        assert!(g.is_some());
    }

    #[test]
    fn unknown_key_rejected() {
        let mgr = ClientKeyManager::new(&[make_config("sk-test", "Test")]).unwrap();
        assert!(matches!(mgr.validate_key("sk-bad", None), Err(ClientKeyError::InvalidKey)));
    }

    #[test]
    fn disabled_key_rejected() {
        let mut cfg = make_config("sk-d", "D");
        cfg.enabled = false;
        let mgr = ClientKeyManager::new(&[cfg]).unwrap();
        assert!(matches!(mgr.validate_key("sk-d", None), Err(ClientKeyError::InvalidKey)));
    }

    #[test]
    fn model_not_allowed() {
        let mgr = ClientKeyManager::new(&[make_config("sk-t", "T")]).unwrap();
        assert!(matches!(mgr.validate_key("sk-t", Some("llama")), Err(ClientKeyError::ModelNotAllowed { .. })));
    }

    #[test]
    fn empty_allowed_models_allows_all() {
        let mut cfg = make_config("sk-t", "T");
        cfg.allowed_models = vec![];
        let mgr = ClientKeyManager::new(&[cfg]).unwrap();
        assert!(mgr.validate_key("sk-t", Some("anything")).is_ok());
    }

    #[test]
    fn global_qpm_limit() {
        let mut cfg = make_config("sk-t", "T");
        cfg.quota.max_requests_per_minute = 3;
        cfg.quota.max_concurrent = 0;
        let mgr = ClientKeyManager::new(&[cfg]).unwrap();
        for _ in 0..3 { mgr.validate_key("sk-t", None).unwrap(); }
        assert!(matches!(mgr.validate_key("sk-t", None), Err(ClientKeyError::RateLimited { .. })));
    }

    #[test]
    fn per_model_qpm_independent() {
        let mut cfg = make_config("sk-t", "T");
        cfg.allowed_models = vec![];
        cfg.quota.max_requests_per_minute = 100; // global: high
        cfg.quota.max_concurrent = 0;
        cfg.model_quotas.insert("expensive-model".into(), ClientKeyQuotaConfig {
            max_requests_per_minute: 2,
            max_tokens_per_day: 0,
            max_concurrent: 0,
        });
        cfg.model_quotas.insert("cheap-model".into(), ClientKeyQuotaConfig {
            max_requests_per_minute: 5,
            max_tokens_per_day: 0,
            max_concurrent: 0,
        });
        let mgr = ClientKeyManager::new(&[cfg]).unwrap();

        // expensive-model: 2 RPM
        mgr.validate_key("sk-t", Some("expensive-model")).unwrap();
        mgr.validate_key("sk-t", Some("expensive-model")).unwrap();
        assert!(matches!(
            mgr.validate_key("sk-t", Some("expensive-model")),
            Err(ClientKeyError::RateLimited { .. })
        ));

        // cheap-model: still fine (independent counter)
        for _ in 0..5 { mgr.validate_key("sk-t", Some("cheap-model")).unwrap(); }
        assert!(matches!(
            mgr.validate_key("sk-t", Some("cheap-model")),
            Err(ClientKeyError::RateLimited { .. })
        ));

        // unknown-model falls back to global (100 RPM), still fine
        mgr.validate_key("sk-t", Some("unknown-model")).unwrap();
    }

    #[test]
    fn per_model_daily_tokens() {
        let mut cfg = make_config("sk-t", "T");
        cfg.allowed_models = vec![];
        cfg.quota.max_tokens_per_day = 1_000_000;
        cfg.quota.max_concurrent = 0;
        cfg.model_quotas.insert("gpt-4o".into(), ClientKeyQuotaConfig {
            max_requests_per_minute: 0,
            max_tokens_per_day: 1000,
            max_concurrent: 0,
        });
        let mgr = ClientKeyManager::new(&[cfg]).unwrap();

        mgr.record_usage("sk-t", Some("gpt-4o"), 1000);
        assert!(matches!(
            mgr.validate_key("sk-t", Some("gpt-4o")),
            Err(ClientKeyError::TokenBudgetExhausted)
        ));

        // Global still has budget
        assert!(mgr.validate_key("sk-t", Some("other-model")).is_ok());
    }

    #[test]
    fn per_model_concurrency() {
        let mut cfg = make_config("sk-t", "T");
        cfg.allowed_models = vec![];
        cfg.quota.max_concurrent = 100;
        cfg.model_quotas.insert("limited".into(), ClientKeyQuotaConfig {
            max_requests_per_minute: 0,
            max_tokens_per_day: 0,
            max_concurrent: 1,
        });
        let mgr = ClientKeyManager::new(&[cfg]).unwrap();

        let (_r, g1) = mgr.validate_key("sk-t", Some("limited")).unwrap();
        assert!(matches!(
            mgr.validate_key("sk-t", Some("limited")),
            Err(ClientKeyError::ConcurrencyExhausted { .. })
        ));

        // Other model unaffected
        assert!(mgr.validate_key("sk-t", Some("other")).is_ok());

        drop(g1);
        assert!(mgr.validate_key("sk-t", Some("limited")).is_ok());
    }

    #[test]
    fn concurrency_guard_drop() {
        let mgr = ClientKeyManager::new(&[make_config("sk-t", "T")]).unwrap();
        let (_, g1) = mgr.validate_key("sk-t", None).unwrap();
        let (_, g2) = mgr.validate_key("sk-t", None).unwrap();
        assert!(matches!(mgr.validate_key("sk-t", None), Err(ClientKeyError::ConcurrencyExhausted { .. })));
        drop(g1);
        assert!(mgr.validate_key("sk-t", None).is_ok());
        drop(g2);
    }

    #[test]
    fn record_usage_unknown_key_noop() {
        let mgr = ClientKeyManager::new(&[make_config("sk-t", "T")]).unwrap();
        mgr.record_usage("sk-bad", None, 500);
    }

    #[test]
    fn empty_config_returns_none() {
        assert!(ClientKeyManager::new(&[]).is_none());
    }

    #[test]
    fn config_deserialization_with_model_quotas() {
        let yaml = r#"
client_keys:
  - key: "sk-alpha"
    name: "Alpha"
    quota:
      max_requests_per_minute: 100
      max_tokens_per_day: 5000000
    model_quotas:
      gpt-4o:
        max_requests_per_minute: 20
        max_tokens_per_day: 1000000
        max_concurrent: 5
      deepseek-r1:
        max_requests_per_minute: 50
        max_concurrent: 10
"#;
        let config: crate::config::types::PrismConfig = serde_yaml::from_str(yaml).unwrap();
        let alpha = &config.client_keys[0];
        assert_eq!(alpha.model_quotas.len(), 2);
        assert_eq!(alpha.model_quotas["gpt-4o"].max_requests_per_minute, 20);
        assert_eq!(alpha.model_quotas["gpt-4o"].max_tokens_per_day, 1_000_000);
        assert_eq!(alpha.model_quotas["gpt-4o"].max_concurrent, 5);
        assert_eq!(alpha.model_quotas["deepseek-r1"].max_requests_per_minute, 50);
        assert_eq!(alpha.model_quotas["deepseek-r1"].max_tokens_per_day, 0); // default
    }
}
