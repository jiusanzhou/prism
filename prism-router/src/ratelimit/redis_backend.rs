//! Redis-backed rate limit backend — distributed sliding window.
//!
//! Uses Redis Sorted Sets + Lua scripts for atomic sliding window counting.
//! Automatically falls back to [`LocalBackend`](super::local::LocalBackend)
//! when Redis is unavailable.

#[cfg(feature = "redis")]
mod inner {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use redis::aio::ConnectionManager;
    use redis::Script;
    use tokio::runtime::Handle;
    use tracing::{debug, warn};

    use super::super::backend::RateLimitBackend;
    use super::super::local::LocalBackend;

    /// Degradation policy when Redis is unavailable.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum DegradationPolicy {
        /// Allow all requests (fail-open).
        Allow,
        /// Deny all requests (fail-closed).
        Deny,
        /// Fall back to local in-memory counting.
        Local,
    }

    /// Redis configuration for the rate limit backend.
    #[derive(Debug, Clone)]
    pub struct RedisConfig {
        pub url: String,
        pub key_prefix: String,
        pub degradation_policy: DegradationPolicy,
    }

    impl Default for RedisConfig {
        fn default() -> Self {
            Self {
                url: "redis://127.0.0.1:6379".to_string(),
                key_prefix: "prism:rl".to_string(),
                degradation_policy: DegradationPolicy::Local,
            }
        }
    }

    /// Redis-backed rate limit backend.
    ///
    /// Uses Lua scripts for atomic operations:
    /// - `sliding_window.lua` — check + increment request count
    /// - `check_tokens.lua` — check token count
    /// - `record_tokens.lua` — increment token count
    pub struct RedisBackend {
        client: ConnectionManager,
        check_script: Script,
        token_check_script: Script,
        token_record_script: Script,
        key_prefix: String,
        degradation_policy: DegradationPolicy,
        fallback: LocalBackend,
        healthy: Arc<AtomicBool>,
    }

    #[allow(dead_code)]
    impl RedisBackend {
        /// Create a new Redis backend. Blocks briefly to establish connection.
        pub async fn new(config: &RedisConfig) -> Result<Self, String> {
            let client = redis::Client::open(config.url.as_str())
                .map_err(|e| format!("Redis client error: {}", e))?;

            let manager = client
                .get_connection_manager()
                .await
                .map_err(|e| format!("Redis connection error: {}", e))?;

            Ok(Self {
                client: manager,
                check_script: Script::new(include_str!("lua/sliding_window.lua")),
                token_check_script: Script::new(include_str!("lua/check_tokens.lua")),
                token_record_script: Script::new(include_str!("lua/record_tokens.lua")),
                key_prefix: config.key_prefix.clone(),
                degradation_policy: config.degradation_policy,
                fallback: LocalBackend::new(),
                healthy: Arc::new(AtomicBool::new(true)),
            })
        }

        fn request_key(&self, key: &str) -> String {
            format!("{}:{}", self.key_prefix, key)
        }

        fn token_key(&self, key: &str) -> String {
            format!("{}:{}:tokens", self.key_prefix, key)
        }

        fn now_secs() -> u64 {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        }

        fn mark_unhealthy(&self) {
            if self.healthy.swap(false, Ordering::Relaxed) {
                warn!("Redis rate limit backend is now unhealthy, degrading");
            }
        }

        fn mark_healthy(&self) {
            if !self.healthy.swap(true, Ordering::Relaxed) {
                debug!("Redis rate limit backend recovered");
            }
        }

        /// Perform the Redis check asynchronously.
        async fn check_async(
            &self,
            key: &str,
            window_secs: u64,
            max_requests: u64,
            max_tokens: Option<u64>,
        ) -> Result<(), u64> {
            let rkey = self.request_key(key);
            let now = Self::now_secs();

            // Check request count
            let result: Result<Vec<i64>, _> = self
                .check_script
                .key(&rkey)
                .arg(window_secs)
                .arg(max_requests)
                .arg(now)
                .invoke_async(&mut self.client.clone())
                .await;

            match result {
                Ok(vals) if vals.len() >= 3 => {
                    self.mark_healthy();
                    if vals[0] == 0 {
                        return Err(vals[2] as u64);
                    }
                    // Check token limit if configured
                    if let Some(max_tok) = max_tokens {
                        let tkey = self.token_key(key);
                        let tok_result: Result<Vec<i64>, _> = self
                            .token_check_script
                            .key(&tkey)
                            .arg(max_tok)
                            .invoke_async(&mut self.client.clone())
                            .await;

                        match tok_result {
                            Ok(vals) if vals.len() >= 2 && vals[0] == 0 => {
                                // Token limit exceeded — calculate retry_after
                                let retry = window_secs.saturating_sub(now.saturating_sub(
                                    now.saturating_sub(window_secs),
                                ));
                                return Err(retry);
                            }
                            Ok(_) => {} // tokens OK
                            Err(e) => {
                                warn!("Redis token check failed: {}", e);
                                // Continue — request count already passed
                            }
                        }
                    }
                    Ok(())
                }
                Ok(vals) => {
                    warn!("Redis script returned unexpected result: {:?}", vals);
                    self.mark_unhealthy();
                    Err(0) // Degrade
                }
                Err(e) => {
                    warn!("Redis check_and_increment failed: {}", e);
                    self.mark_unhealthy();
                    Err(0) // Signal degradation
                }
            }
        }

        /// Record tokens asynchronously.
        async fn record_tokens_async(&self, key: &str, tokens: u64, window_secs: u64) {
            let tkey = self.token_key(key);
            let result: Result<i64, _> = self
                .token_record_script
                .key(&tkey)
                .arg(tokens)
                .arg(window_secs)
                .invoke_async(&mut self.client.clone())
                .await;

            match result {
                Ok(_) => self.mark_healthy(),
                Err(e) => {
                    warn!("Redis record_tokens failed: {}", e);
                    self.mark_unhealthy();
                }
            }
        }
    }

    impl RateLimitBackend for RedisBackend {
        fn check_and_increment(
            &self,
            key: &str,
            window_secs: u64,
            max_requests: u64,
            max_tokens: Option<u64>,
        ) -> Result<(), u64> {
            // Try to run async Redis call from sync context
            let handle = match Handle::try_current() {
                Ok(h) => h,
                Err(_) => {
                    // No tokio runtime — fall back
                    return self
                        .fallback
                        .check_and_increment(key, window_secs, max_requests, max_tokens);
                }
            };

            // Use block_in_place to run async from sync trait method
            let result = tokio::task::block_in_place(|| {
                handle.block_on(self.check_async(key, window_secs, max_requests, max_tokens))
            });

            match result {
                Ok(()) => Ok(()),
                Err(retry) if !self.is_healthy() => {
                    // Redis failed — apply degradation policy
                    match self.degradation_policy {
                        DegradationPolicy::Allow => Ok(()),
                        DegradationPolicy::Deny => Err(retry),
                        DegradationPolicy::Local => self
                            .fallback
                            .check_and_increment(key, window_secs, max_requests, max_tokens),
                    }
                }
                Err(retry) => Err(retry), // Legitimate rate limit hit
            }
        }

        fn record_tokens(&self, key: &str, tokens: u64) {
            let handle = match Handle::try_current() {
                Ok(h) => h,
                Err(_) => {
                    self.fallback.record_tokens(key, tokens);
                    return;
                }
            };

            // Fire-and-forget with fallback
            // Use a reasonable default window for token TTL
            let tkey = self.token_key(key);
            let mut client = self.client.clone();
            let script = self.token_record_script.clone();
            let healthy = Arc::clone(&self.healthy);
            let fallback_key = key.to_string();
            let fallback = &self.fallback;

            // Also record locally for fallback
            fallback.record_tokens(key, tokens);

            tokio::task::block_in_place(|| {
                handle.block_on(async {
                    let result: Result<i64, _> = script
                        .key(&tkey)
                        .arg(tokens)
                        .arg(60u64) // default window for TTL
                        .invoke_async(&mut client)
                        .await;

                    if let Err(e) = result {
                        warn!("Redis record_tokens failed for {}: {}", fallback_key, e);
                        healthy.store(false, Ordering::Relaxed);
                    }
                });
            });
        }

        fn cleanup(&self, max_window_secs: u64) {
            // Redis handles TTL-based cleanup automatically via EXPIRE.
            // Clean up local fallback.
            self.fallback.cleanup(max_window_secs);
        }

        fn name(&self) -> &str {
            "redis"
        }

        fn is_healthy(&self) -> bool {
            self.healthy.load(Ordering::Relaxed)
        }
    }
}

#[cfg(feature = "redis")]
pub use inner::{DegradationPolicy, RedisBackend, RedisConfig};
