//! Redis-backed rate limit backend — distributed sliding window.

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
    use super::super::quota::{Quota, QuotaDenied, QuotaMetric};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum DegradationPolicy {
        Allow,
        Deny,
        Local,
    }

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

    pub struct RedisBackend {
        client: ConnectionManager,
        request_script: Script,
        key_prefix: String,
        degradation_policy: DegradationPolicy,
        fallback: LocalBackend,
        healthy: Arc<AtomicBool>,
    }

    #[allow(dead_code)]
    impl RedisBackend {
        pub async fn new(config: &RedisConfig) -> Result<Self, String> {
            let client = redis::Client::open(config.url.as_str())
                .map_err(|e| format!("Redis client error: {}", e))?;
            let manager = client
                .get_connection_manager()
                .await
                .map_err(|e| format!("Redis connection error: {}", e))?;

            Ok(Self {
                client: manager,
                request_script: Script::new(include_str!("lua/sliding_window.lua")),
                key_prefix: config.key_prefix.clone(),
                degradation_policy: config.degradation_policy,
                fallback: LocalBackend::new(),
                healthy: Arc::new(AtomicBool::new(true)),
            })
        }

        fn metric_key(&self, key: &str, metric: &QuotaMetric) -> String {
            format!("{}:{}:{}", self.key_prefix, key, metric.key_suffix())
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

        /// Check all quotas asynchronously.
        async fn check_quotas_async(
            &self,
            key: &str,
            window_secs: u64,
            quotas: &[Quota],
        ) -> Result<(), QuotaDenied> {
            let now = Self::now_secs();

            for quota in quotas {
                let rkey = self.metric_key(key, &quota.metric);

                if quota.metric.is_pre_check() {
                    // Requests: atomic check + increment via Lua
                    let result: Result<Vec<i64>, _> = self
                        .request_script
                        .key(&rkey)
                        .arg(window_secs)
                        .arg(quota.limit)
                        .arg(now)
                        .invoke_async(&mut self.client.clone())
                        .await;

                    match result {
                        Ok(vals) if vals.len() >= 3 => {
                            self.mark_healthy();
                            if vals[0] == 0 {
                                return Err(QuotaDenied {
                                    metric: quota.metric.clone(),
                                    current: vals[1] as u64,
                                    limit: quota.limit,
                                    retry_after_secs: vals[2] as u64,
                                });
                            }
                        }
                        Ok(_) | Err(_) => {
                            self.mark_unhealthy();
                            return Err(QuotaDenied {
                                metric: quota.metric.clone(),
                                current: 0,
                                limit: quota.limit,
                                retry_after_secs: 0,
                            });
                        }
                    }
                } else {
                    // Post-record metrics: GET current value and compare
                    let result: Result<Option<u64>, _> = redis::cmd("GET")
                        .arg(&rkey)
                        .query_async(&mut self.client.clone())
                        .await;

                    match result {
                        Ok(Some(current)) => {
                            self.mark_healthy();
                            if current >= quota.limit {
                                let retry = window_secs; // approximate
                                return Err(QuotaDenied {
                                    metric: quota.metric.clone(),
                                    current,
                                    limit: quota.limit,
                                    retry_after_secs: retry,
                                });
                            }
                        }
                        Ok(None) => {
                            self.mark_healthy();
                            // No value yet — within limit
                        }
                        Err(e) => {
                            warn!("Redis GET failed for {}: {}", rkey, e);
                            self.mark_unhealthy();
                            return Err(QuotaDenied {
                                metric: quota.metric.clone(),
                                current: 0,
                                limit: quota.limit,
                                retry_after_secs: 0,
                            });
                        }
                    }
                }
            }

            Ok(())
        }

        /// Record a metric value asynchronously.
        async fn record_metric_async(
            &self,
            key: &str,
            metric: &QuotaMetric,
            value: u64,
            window_secs: u64,
        ) {
            let rkey = self.metric_key(key, metric);

            // Use INCRBY + EXPIRE for post-record metrics
            let result: Result<u64, _> = redis::cmd("INCRBY")
                .arg(&rkey)
                .arg(value)
                .query_async(&mut self.client.clone())
                .await;

            match result {
                Ok(new_val) => {
                    self.mark_healthy();
                    // Set TTL if this is the first increment
                    if new_val == value {
                        let _: Result<(), _> = redis::cmd("EXPIRE")
                            .arg(&rkey)
                            .arg(window_secs + 1)
                            .query_async(&mut self.client.clone())
                            .await;
                    }
                }
                Err(e) => {
                    warn!("Redis INCRBY failed for {}: {}", rkey, e);
                    self.mark_unhealthy();
                }
            }
        }
    }

    impl RateLimitBackend for RedisBackend {
        fn check_quotas(
            &self,
            key: &str,
            window_secs: u64,
            quotas: &[Quota],
        ) -> Result<(), QuotaDenied> {
            let handle = match Handle::try_current() {
                Ok(h) => h,
                Err(_) => return self.fallback.check_quotas(key, window_secs, quotas),
            };

            let result = tokio::task::block_in_place(|| {
                handle.block_on(self.check_quotas_async(key, window_secs, quotas))
            });

            match result {
                Ok(()) => Ok(()),
                Err(denied) if !self.is_healthy() => match self.degradation_policy {
                    DegradationPolicy::Allow => Ok(()),
                    DegradationPolicy::Deny => Err(denied),
                    DegradationPolicy::Local => {
                        self.fallback.check_quotas(key, window_secs, quotas)
                    }
                },
                Err(denied) => Err(denied),
            }
        }

        fn record_metric(&self, key: &str, metric: &QuotaMetric, value: u64) {
            // Also record locally for fallback
            self.fallback.record_metric(key, metric, value);

            let handle = match Handle::try_current() {
                Ok(h) => h,
                Err(_) => return,
            };

            let rkey = self.metric_key(key, metric);
            let mut client = self.client.clone();
            let healthy = Arc::clone(&self.healthy);

            tokio::task::block_in_place(|| {
                handle.block_on(async {
                    let result: Result<u64, _> = redis::cmd("INCRBY")
                        .arg(&rkey)
                        .arg(value)
                        .query_async(&mut client)
                        .await;

                    match result {
                        Ok(new_val) => {
                            if !healthy.load(Ordering::Relaxed) {
                                healthy.store(true, Ordering::Relaxed);
                            }
                            if new_val == value {
                                let _: Result<(), _> = redis::cmd("EXPIRE")
                                    .arg(&rkey)
                                    .arg(61u64) // default window + 1
                                    .query_async(&mut client)
                                    .await;
                            }
                        }
                        Err(e) => {
                            warn!("Redis record_metric failed: {}", e);
                            healthy.store(false, Ordering::Relaxed);
                        }
                    }
                });
            });
        }

        fn cleanup(&self, max_window_secs: u64) {
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
