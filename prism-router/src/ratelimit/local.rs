//! Local in-memory rate limit backend — DashMap sliding window.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;

use super::backend::RateLimitBackend;
use super::quota::{Quota, QuotaDenied, QuotaMetric};

/// Per-key window state.
struct WindowState {
    window_start: u64,
    counters: HashMap<QuotaMetric, u64>,
}

impl WindowState {
    fn new(now: u64) -> Self {
        Self {
            window_start: now,
            counters: HashMap::new(),
        }
    }

    fn get(&self, metric: &QuotaMetric) -> u64 {
        self.counters.get(metric).copied().unwrap_or(0)
    }

    fn increment(&mut self, metric: &QuotaMetric, value: u64) {
        *self.counters.entry(metric.clone()).or_insert(0) += value;
    }

    fn reset(&mut self, now: u64) {
        self.window_start = now;
        self.counters.clear();
    }
}

/// Local in-memory rate limit backend.
pub struct LocalBackend {
    windows: DashMap<String, WindowState>,
}

impl LocalBackend {
    pub fn new() -> Self {
        Self {
            windows: DashMap::new(),
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

impl Default for LocalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimitBackend for LocalBackend {
    fn check_quotas(
        &self,
        key: &str,
        window_secs: u64,
        quotas: &[Quota],
    ) -> Result<(), QuotaDenied> {
        let now = Self::now_secs();

        let mut entry = self
            .windows
            .entry(key.to_string())
            .or_insert_with(|| WindowState::new(now));
        let state = entry.value_mut();

        // Reset window if expired
        if now - state.window_start >= window_secs {
            state.reset(now);
        }

        // Check all quotas
        for quota in quotas {
            let current = state.get(&quota.metric);

            if quota.metric.is_pre_check() {
                // Requests: check THEN increment
                if current >= quota.limit {
                    let retry_after = window_secs.saturating_sub(now - state.window_start);
                    return Err(QuotaDenied {
                        metric: quota.metric.clone(),
                        current,
                        limit: quota.limit,
                        retry_after_secs: retry_after,
                    });
                }
            } else {
                // Post-record metrics: just check current value
                if current >= quota.limit {
                    let retry_after = window_secs.saturating_sub(now - state.window_start);
                    return Err(QuotaDenied {
                        metric: quota.metric.clone(),
                        current,
                        limit: quota.limit,
                        retry_after_secs: retry_after,
                    });
                }
            }
        }

        // All passed — increment request counter
        if quotas.iter().any(|q| q.metric.is_pre_check()) {
            state.increment(&QuotaMetric::Requests, 1);
        }

        Ok(())
    }

    fn record_metric(&self, key: &str, metric: &QuotaMetric, value: u64) {
        if let Some(mut entry) = self.windows.get_mut(key) {
            entry.value_mut().increment(metric, value);
        }
    }

    fn cleanup(&self, max_window_secs: u64) {
        let now = Self::now_secs();
        self.windows
            .retain(|_, state| now - state.window_start < max_window_secs);
    }

    fn name(&self) -> &str {
        "local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_name() {
        let backend = LocalBackend::new();
        assert_eq!(backend.name(), "local");
    }

    #[test]
    fn test_backend_is_healthy() {
        let backend = LocalBackend::new();
        assert!(backend.is_healthy());
    }

    #[test]
    fn test_requests_within_limit() {
        let backend = LocalBackend::new();
        let quotas = vec![Quota {
            metric: QuotaMetric::Requests,
            limit: 5,
        }];
        for _ in 0..5 {
            assert!(backend.check_quotas("test", 60, &quotas).is_ok());
        }
    }

    #[test]
    fn test_requests_exceeds_limit() {
        let backend = LocalBackend::new();
        let quotas = vec![Quota {
            metric: QuotaMetric::Requests,
            limit: 3,
        }];
        for _ in 0..3 {
            assert!(backend.check_quotas("test", 60, &quotas).is_ok());
        }
        let err = backend.check_quotas("test", 60, &quotas).unwrap_err();
        assert_eq!(err.metric, QuotaMetric::Requests);
        assert_eq!(err.current, 3);
        assert_eq!(err.limit, 3);
        assert!(err.retry_after_secs > 0);
    }

    #[test]
    fn test_independent_keys() {
        let backend = LocalBackend::new();
        let quotas = vec![Quota {
            metric: QuotaMetric::Requests,
            limit: 2,
        }];
        assert!(backend.check_quotas("a", 60, &quotas).is_ok());
        assert!(backend.check_quotas("a", 60, &quotas).is_ok());
        assert!(backend.check_quotas("a", 60, &quotas).is_err());
        assert!(backend.check_quotas("b", 60, &quotas).is_ok());
    }

    #[test]
    fn test_window_expiry() {
        let backend = LocalBackend::new();
        let quotas = vec![Quota {
            metric: QuotaMetric::Requests,
            limit: 1,
        }];
        assert!(backend.check_quotas("test", 0, &quotas).is_ok());
        // window_secs=0, next check resets
        assert!(backend.check_quotas("test", 0, &quotas).is_ok());
    }

    #[test]
    fn test_token_quota() {
        let backend = LocalBackend::new();
        let quotas = vec![
            Quota {
                metric: QuotaMetric::Requests,
                limit: 100,
            },
            Quota {
                metric: QuotaMetric::TotalTokens,
                limit: 1000,
            },
        ];

        assert!(backend.check_quotas("test", 60, &quotas).is_ok());
        backend.record_metric("test", &QuotaMetric::TotalTokens, 999);
        assert!(backend.check_quotas("test", 60, &quotas).is_ok());
        backend.record_metric("test", &QuotaMetric::TotalTokens, 1);
        let err = backend.check_quotas("test", 60, &quotas).unwrap_err();
        assert_eq!(err.metric, QuotaMetric::TotalTokens);
        assert_eq!(err.current, 1000);
        assert_eq!(err.limit, 1000);
    }

    #[test]
    fn test_multi_metric_quota() {
        let backend = LocalBackend::new();
        let quotas = vec![
            Quota {
                metric: QuotaMetric::Requests,
                limit: 100,
            },
            Quota {
                metric: QuotaMetric::InputTokens,
                limit: 500,
            },
            Quota {
                metric: QuotaMetric::OutputTokens,
                limit: 300,
            },
            Quota {
                metric: QuotaMetric::CostMicros,
                limit: 10000,
            },
        ];

        assert!(backend.check_quotas("test", 60, &quotas).is_ok());

        // Record metrics
        backend.record_metric("test", &QuotaMetric::InputTokens, 200);
        backend.record_metric("test", &QuotaMetric::OutputTokens, 100);
        backend.record_metric("test", &QuotaMetric::CostMicros, 5000);
        assert!(backend.check_quotas("test", 60, &quotas).is_ok());

        // Push output_tokens over limit
        backend.record_metric("test", &QuotaMetric::OutputTokens, 200);
        let err = backend.check_quotas("test", 60, &quotas).unwrap_err();
        assert_eq!(err.metric, QuotaMetric::OutputTokens);
        assert_eq!(err.current, 300);
        assert_eq!(err.limit, 300);
    }

    #[test]
    fn test_custom_metric() {
        let backend = LocalBackend::new();
        let quotas = vec![Quota {
            metric: QuotaMetric::Custom("images".to_string()),
            limit: 10,
        }];

        assert!(backend.check_quotas("test", 60, &quotas).is_ok());
        backend.record_metric("test", &QuotaMetric::Custom("images".to_string()), 10);
        let err = backend.check_quotas("test", 60, &quotas).unwrap_err();
        assert_eq!(err.metric, QuotaMetric::Custom("images".to_string()));
    }

    #[test]
    fn test_cleanup() {
        let backend = LocalBackend::new();
        let quotas = vec![Quota {
            metric: QuotaMetric::Requests,
            limit: 10,
        }];
        assert!(backend.check_quotas("test", 60, &quotas).is_ok());
        backend.cleanup(60);
    }

    #[test]
    fn test_no_quotas_always_passes() {
        let backend = LocalBackend::new();
        for _ in 0..100 {
            assert!(backend.check_quotas("test", 60, &[]).is_ok());
        }
    }
}
