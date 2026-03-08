//! Client key manager — per-key quota enforcement for multi-tenant API access.
//!
//! Provides sliding-window QPM, daily token budgets, concurrent request limits,
//! and per-key model access control. All state is in-memory using std atomics.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use crate::config::types::ClientKeyConfig;

/// Shared reference to a resolved client key.
#[derive(Debug, Clone)]
pub struct ClientKeyRef {
    pub name: String,
    pub key: String,
}

/// Errors from client key validation.
#[derive(Debug, Clone)]
pub enum ClientKeyError {
    /// Key not found or disabled.
    InvalidKey,
    /// Model not in allowed_models list.
    ModelNotAllowed { model: String },
    /// QPM limit exceeded.
    RateLimited { retry_after_secs: u64 },
    /// Daily token budget exceeded.
    TokenBudgetExhausted,
    /// Concurrent request limit exceeded.
    ConcurrencyExhausted { current: u32, limit: u32 },
}

/// Per-key runtime state for quota tracking.
struct KeyState {
    config: ClientKeyConfig,
    allowed_models: HashSet<String>,
    /// Sliding window: timestamps of requests in the current minute.
    request_timestamps: RwLock<Vec<Instant>>,
    /// Daily token accumulator.
    tokens_today: AtomicU64,
    /// Day marker (days since UNIX epoch, UTC).
    token_day: AtomicU64,
    /// Current concurrent requests.
    concurrent: AtomicU32,
}

/// Guard that decrements the concurrent counter on drop.
pub struct ConcurrencyGuard {
    concurrent: *const AtomicU32,
}

// SAFETY: AtomicU32 is Send+Sync, and the pointer lifetime is tied to
// the Arc<ClientKeyManager> which outlives all guards.
unsafe impl Send for ConcurrencyGuard {}
unsafe impl Sync for ConcurrencyGuard {}

impl Drop for ConcurrencyGuard {
    fn drop(&mut self) {
        // SAFETY: The ClientKeyManager (and its KeyState) lives in an Arc
        // that outlives any request using this guard.
        unsafe {
            (*self.concurrent).fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Manages per-client-key quota enforcement.
pub struct ClientKeyManager {
    keys: HashMap<String, KeyState>,
}

impl ClientKeyManager {
    /// Build from config. Returns None if client_keys is empty.
    pub fn new(configs: &[ClientKeyConfig]) -> Option<Self> {
        if configs.is_empty() {
            return None;
        }
        let mut keys = HashMap::new();
        for cfg in configs {
            let allowed: HashSet<String> = cfg.allowed_models.iter().cloned().collect();
            keys.insert(
                cfg.key.clone(),
                KeyState {
                    config: cfg.clone(),
                    allowed_models: allowed,
                    request_timestamps: RwLock::new(Vec::new()),
                    tokens_today: AtomicU64::new(0),
                    token_day: AtomicU64::new(current_day_utc()),
                    concurrent: AtomicU32::new(0),
                },
            );
        }
        Some(Self { keys })
    }

    /// Validate a client key for the given model.
    /// On success, returns a `ClientKeyRef` and a `ConcurrencyGuard`.
    /// The guard MUST be held for the request lifetime (drop decrements concurrent).
    pub fn validate_key(
        &self,
        api_key: &str,
        model: Option<&str>,
    ) -> Result<(ClientKeyRef, Option<ConcurrencyGuard>), ClientKeyError> {
        let state = self.keys.get(api_key).ok_or(ClientKeyError::InvalidKey)?;

        // Enabled check
        if !state.config.enabled {
            return Err(ClientKeyError::InvalidKey);
        }

        // Model access control
        if let Some(model_name) = model {
            if !state.allowed_models.is_empty() && !state.allowed_models.contains(model_name) {
                return Err(ClientKeyError::ModelNotAllowed {
                    model: model_name.to_string(),
                });
            }
        }

        // QPM sliding window check
        let qpm_limit = state.config.quota.max_requests_per_minute;
        if qpm_limit > 0 {
            let now = Instant::now();
            let window = Duration::from_secs(60);
            let mut timestamps = state.request_timestamps.write().unwrap();
            // Evict entries older than 60s
            timestamps.retain(|&t| now.duration_since(t) < window);
            if timestamps.len() as u64 >= qpm_limit {
                // Estimate retry-after from oldest entry
                let oldest = timestamps.first().copied().unwrap_or(now);
                let retry_after = window
                    .checked_sub(now.duration_since(oldest))
                    .unwrap_or(Duration::from_secs(1));
                return Err(ClientKeyError::RateLimited {
                    retry_after_secs: retry_after.as_secs().max(1),
                });
            }
            timestamps.push(now);
        }

        // Daily token budget check
        let daily_limit = state.config.quota.max_tokens_per_day;
        if daily_limit > 0 {
            maybe_reset_daily(&state.tokens_today, &state.token_day);
            let used = state.tokens_today.load(Ordering::Relaxed);
            if used >= daily_limit {
                return Err(ClientKeyError::TokenBudgetExhausted);
            }
        }

        // Concurrent request limit
        let max_concurrent = state.config.quota.max_concurrent;
        let guard = if max_concurrent > 0 {
            let prev = state.concurrent.fetch_add(1, Ordering::Relaxed);
            if prev >= max_concurrent {
                state.concurrent.fetch_sub(1, Ordering::Relaxed);
                return Err(ClientKeyError::ConcurrencyExhausted {
                    current: prev,
                    limit: max_concurrent,
                });
            }
            Some(ConcurrencyGuard {
                concurrent: &state.concurrent as *const AtomicU32,
            })
        } else {
            None
        };

        Ok((
            ClientKeyRef {
                name: state.config.name.clone(),
                key: api_key.to_string(),
            },
            guard,
        ))
    }

    /// Record token usage after a response completes.
    pub fn record_usage(&self, api_key: &str, tokens_used: u64) {
        if tokens_used == 0 {
            return;
        }
        if let Some(state) = self.keys.get(api_key) {
            maybe_reset_daily(&state.tokens_today, &state.token_day);
            state.tokens_today.fetch_add(tokens_used, Ordering::Relaxed);
        }
    }

    /// Check if a model is allowed for the given key.
    /// Returns Ok(()) if the model is allowed, Err if not.
    pub fn check_model_access(&self, api_key: &str, model: &str) -> Result<(), ClientKeyError> {
        if let Some(state) = self.keys.get(api_key) {
            if !state.allowed_models.is_empty() && !state.allowed_models.contains(model) {
                return Err(ClientKeyError::ModelNotAllowed {
                    model: model.to_string(),
                });
            }
        }
        Ok(())
    }
}

/// Current day as days-since-epoch (UTC).
fn current_day_utc() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs() / 86400
}

/// Reset daily counters if the day has rolled over.
fn maybe_reset_daily(tokens: &AtomicU64, day: &AtomicU64) {
    let today = current_day_utc();
    let stored_day = day.load(Ordering::Relaxed);
    if today != stored_day {
        // CAS to prevent double-reset
        if day
            .compare_exchange(stored_day, today, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
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
            allowed_models: vec!["gpt-4o".to_string(), "claude-sonnet-4".to_string()],
            quota: ClientKeyQuotaConfig {
                max_requests_per_minute: 10,
                max_tokens_per_day: 100_000,
                max_concurrent: 2,
            },
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn test_valid_key_passes() {
        let mgr = ClientKeyManager::new(&[make_config("sk-test", "Test")]).unwrap();
        let (key_ref, guard) = mgr.validate_key("sk-test", Some("gpt-4o")).unwrap();
        assert_eq!(key_ref.name, "Test");
        assert!(guard.is_some());
    }

    #[test]
    fn test_unknown_key_rejected() {
        let mgr = ClientKeyManager::new(&[make_config("sk-test", "Test")]).unwrap();
        assert!(matches!(
            mgr.validate_key("sk-unknown", None),
            Err(ClientKeyError::InvalidKey)
        ));
    }

    #[test]
    fn test_disabled_key_rejected() {
        let mut cfg = make_config("sk-disabled", "Disabled");
        cfg.enabled = false;
        let mgr = ClientKeyManager::new(&[cfg]).unwrap();
        assert!(matches!(
            mgr.validate_key("sk-disabled", None),
            Err(ClientKeyError::InvalidKey)
        ));
    }

    #[test]
    fn test_model_not_allowed() {
        let mgr = ClientKeyManager::new(&[make_config("sk-test", "Test")]).unwrap();
        assert!(matches!(
            mgr.validate_key("sk-test", Some("llama-3")),
            Err(ClientKeyError::ModelNotAllowed { .. })
        ));
    }

    #[test]
    fn test_empty_allowed_models_allows_all() {
        let mut cfg = make_config("sk-test", "Test");
        cfg.allowed_models = vec![];
        let mgr = ClientKeyManager::new(&[cfg]).unwrap();
        assert!(mgr.validate_key("sk-test", Some("any-model")).is_ok());
    }

    #[test]
    fn test_qpm_limit() {
        let mut cfg = make_config("sk-test", "Test");
        cfg.quota.max_requests_per_minute = 3;
        cfg.quota.max_concurrent = 0; // disable concurrency limit for this test
        let mgr = ClientKeyManager::new(&[cfg]).unwrap();

        // First 3 should pass
        for _ in 0..3 {
            assert!(mgr.validate_key("sk-test", None).is_ok());
        }
        // 4th should fail with rate limit
        assert!(matches!(
            mgr.validate_key("sk-test", None),
            Err(ClientKeyError::RateLimited { .. })
        ));
    }

    #[test]
    fn test_daily_token_budget() {
        let mut cfg = make_config("sk-test", "Test");
        cfg.quota.max_tokens_per_day = 1000;
        cfg.quota.max_concurrent = 0;
        let mgr = ClientKeyManager::new(&[cfg]).unwrap();

        // Record usage up to the limit
        mgr.record_usage("sk-test", 1000);

        // Should now be rejected
        assert!(matches!(
            mgr.validate_key("sk-test", None),
            Err(ClientKeyError::TokenBudgetExhausted)
        ));
    }

    #[test]
    fn test_concurrency_limit() {
        let mut cfg = make_config("sk-test", "Test");
        cfg.quota.max_concurrent = 2;
        let mgr = ClientKeyManager::new(&[cfg]).unwrap();

        let (_ref1, guard1) = mgr.validate_key("sk-test", None).unwrap();
        let (_ref2, guard2) = mgr.validate_key("sk-test", None).unwrap();

        // 3rd should be denied
        assert!(matches!(
            mgr.validate_key("sk-test", None),
            Err(ClientKeyError::ConcurrencyExhausted { .. })
        ));

        // Drop one guard, should allow again
        drop(guard1);
        assert!(mgr.validate_key("sk-test", None).is_ok());

        drop(guard2);
    }

    #[test]
    fn test_record_usage_unknown_key_noop() {
        let mgr = ClientKeyManager::new(&[make_config("sk-test", "Test")]).unwrap();
        // Should not panic
        mgr.record_usage("sk-nonexistent", 500);
    }

    #[test]
    fn test_empty_config_returns_none() {
        assert!(ClientKeyManager::new(&[]).is_none());
    }

    #[test]
    fn test_config_deserialization() {
        let yaml = r#"
client_keys:
  - key: "sk-team-alpha-xxx"
    name: "Team Alpha"
    enabled: true
    allowed_models: ["gpt-4o", "claude-sonnet-4"]
    quota:
      max_requests_per_minute: 100
      max_tokens_per_day: 1000000
      max_concurrent: 10
    metadata:
      team: "alpha"
      tier: "business"
  - key: "sk-team-beta-yyy"
    name: "Team Beta"
"#;
        let config: crate::config::types::PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.client_keys.len(), 2);

        let alpha = &config.client_keys[0];
        assert_eq!(alpha.key, "sk-team-alpha-xxx");
        assert_eq!(alpha.name, "Team Alpha");
        assert!(alpha.enabled);
        assert_eq!(alpha.allowed_models, vec!["gpt-4o", "claude-sonnet-4"]);
        assert_eq!(alpha.quota.max_requests_per_minute, 100);
        assert_eq!(alpha.quota.max_tokens_per_day, 1_000_000);
        assert_eq!(alpha.quota.max_concurrent, 10);
        assert_eq!(alpha.metadata.get("team"), Some(&"alpha".to_string()));

        // Beta has defaults
        let beta = &config.client_keys[1];
        assert_eq!(beta.name, "Team Beta");
        assert!(beta.enabled); // default_true
        assert!(beta.allowed_models.is_empty());
        assert_eq!(beta.quota.max_requests_per_minute, 0);
    }
}
