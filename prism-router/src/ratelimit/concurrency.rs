//! Concurrency limiter — limits the number of in-flight requests per key.
//!
//! Unlike rate limiting (counter-based, window-based), concurrency limiting uses
//! a gauge: increment on request start, decrement on request end.
//!
//! Uses a `ConcurrencyGuard` RAII pattern to ensure the counter is always decremented.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;

/// Concurrency limit configuration.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ConcurrencyRule {
    /// Rule name.
    pub name: String,
    /// Header names to build the concurrency key (empty = global).
    pub key_headers: Vec<String>,
    /// Maximum concurrent requests.
    pub max_concurrent: u64,
    /// Whether this rule is enabled.
    #[serde(default = "super::default_true")]
    pub enabled: bool,
    /// HTTP status code when rejected (default 429).
    #[serde(default = "super::default_429")]
    pub status_code: u16,
    /// Error message when rejected.
    #[serde(default = "default_concurrency_message")]
    pub message: String,
}

fn default_concurrency_message() -> String {
    "Too many concurrent requests".to_string()
}

/// Result when concurrency limit is exceeded.
#[derive(Debug)]
pub struct ConcurrencyDenied {
    pub rule_name: String,
    pub status_code: u16,
    pub message: String,
    pub current: u64,
    pub limit: u64,
}

/// RAII guard — decrements the counter when dropped.
#[derive(Debug)]
pub struct ConcurrencyGuard {
    counter: Arc<AtomicU64>,
}

impl Drop for ConcurrencyGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Concurrency limiter.
pub struct ConcurrencyLimiter {
    rules: Vec<ConcurrencyRule>,
    /// key → atomic counter
    counters: DashMap<String, Arc<AtomicU64>>,
}

impl ConcurrencyLimiter {
    pub fn new(rules: Vec<ConcurrencyRule>) -> Self {
        Self {
            rules,
            counters: DashMap::new(),
        }
    }

    /// Try to acquire a concurrency slot. Returns guards for all matched rules,
    /// or a denial if any rule is at its limit.
    ///
    /// The caller MUST hold the returned guards until the request completes.
    pub fn try_acquire(
        &self,
        headers: &[(String, String)],
    ) -> Result<Vec<ConcurrencyGuard>, ConcurrencyDenied> {
        let mut guards = Vec::new();

        for rule in &self.rules {
            if !rule.enabled {
                continue;
            }

            let key = Self::build_key(rule, headers);
            let counter = self
                .counters
                .entry(key)
                .or_insert_with(|| Arc::new(AtomicU64::new(0)))
                .clone();

            // Attempt to increment atomically, but only if under limit
            loop {
                let current = counter.load(Ordering::Relaxed);
                if current >= rule.max_concurrent {
                    // Rollback all previously acquired guards
                    drop(guards);
                    return Err(ConcurrencyDenied {
                        rule_name: rule.name.clone(),
                        status_code: rule.status_code,
                        message: rule.message.clone(),
                        current,
                        limit: rule.max_concurrent,
                    });
                }
                if counter
                    .compare_exchange(current, current + 1, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    guards.push(ConcurrencyGuard {
                        counter: Arc::clone(&counter),
                    });
                    break;
                }
                // CAS failed, retry
            }
        }

        Ok(guards)
    }

    /// Get the current concurrency count for a key.
    pub fn current(&self, rule_name: &str, headers: &[(String, String)]) -> u64 {
        if let Some(rule) = self.rules.iter().find(|r| r.name == rule_name) {
            let key = Self::build_key(rule, headers);
            self.counters
                .get(&key)
                .map(|c| c.load(Ordering::Relaxed))
                .unwrap_or(0)
        } else {
            0
        }
    }

    /// Get current rules.
    pub fn rules(&self) -> &[ConcurrencyRule] {
        &self.rules
    }

    /// Update rules (hot-reload).
    pub fn update_rules(&mut self, rules: Vec<ConcurrencyRule>) {
        self.rules = rules;
    }

    fn build_key(rule: &ConcurrencyRule, headers: &[(String, String)]) -> String {
        if rule.key_headers.is_empty() {
            return rule.name.clone();
        }
        let mut key = rule.name.clone();
        for header_name in &rule.key_headers {
            key.push(':');
            let value = headers
                .iter()
                .find(|(k, _)| k == header_name)
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            key.push_str(value);
        }
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(name: &str, key_headers: Vec<&str>, max_concurrent: u64) -> ConcurrencyRule {
        ConcurrencyRule {
            name: name.to_string(),
            key_headers: key_headers.into_iter().map(|s| s.to_string()).collect(),
            max_concurrent,
            enabled: true,
            status_code: 429,
            message: "Too many concurrent requests".to_string(),
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn test_basic_acquire_release() {
        let limiter = ConcurrencyLimiter::new(vec![make_rule("global", vec![], 2)]);
        let h = headers(&[]);

        let g1 = limiter.try_acquire(&h).unwrap();
        assert_eq!(limiter.current("global", &h), 1);

        let g2 = limiter.try_acquire(&h).unwrap();
        assert_eq!(limiter.current("global", &h), 2);

        // Third should fail
        assert!(limiter.try_acquire(&h).is_err());

        // Drop one guard
        drop(g1);
        assert_eq!(limiter.current("global", &h), 1);

        // Now should succeed
        let _g3 = limiter.try_acquire(&h).unwrap();
        assert_eq!(limiter.current("global", &h), 2);

        drop(g2);
        drop(_g3);
        assert_eq!(limiter.current("global", &h), 0);
    }

    #[test]
    fn test_per_key_isolation() {
        let limiter = ConcurrencyLimiter::new(vec![make_rule("per-model", vec!["x-model"], 1)]);
        let h_gpt4 = headers(&[("x-model", "gpt-4")]);
        let h_claude = headers(&[("x-model", "claude")]);

        let _g1 = limiter.try_acquire(&h_gpt4).unwrap();
        assert!(limiter.try_acquire(&h_gpt4).is_err());

        // Different key should work
        let _g2 = limiter.try_acquire(&h_claude).unwrap();
        assert!(limiter.try_acquire(&h_claude).is_err());
    }

    #[test]
    fn test_disabled_rule_skipped() {
        let mut rule = make_rule("disabled", vec![], 1);
        rule.enabled = false;
        let limiter = ConcurrencyLimiter::new(vec![rule]);
        let h = headers(&[]);

        // Should always succeed since rule is disabled
        for _ in 0..100 {
            let _g = limiter.try_acquire(&h).unwrap();
        }
    }

    #[test]
    fn test_denied_fields() {
        let limiter = ConcurrencyLimiter::new(vec![make_rule("test", vec![], 1)]);
        let h = headers(&[]);
        let _g = limiter.try_acquire(&h).unwrap();

        let denied = limiter.try_acquire(&h).unwrap_err();
        assert_eq!(denied.rule_name, "test");
        assert_eq!(denied.current, 1);
        assert_eq!(denied.limit, 1);
        assert_eq!(denied.status_code, 429);
    }

    #[test]
    fn test_multi_rule_rollback() {
        // Two rules: first allows 2, second allows 1
        let limiter = ConcurrencyLimiter::new(vec![
            make_rule("rule-a", vec![], 2),
            make_rule("rule-b", vec![], 1),
        ]);
        let h = headers(&[]);

        let _g1 = limiter.try_acquire(&h).unwrap();
        // rule-a: 1/2, rule-b: 1/1

        // Second acquire: rule-a allows but rule-b doesn't → should rollback rule-a
        let err = limiter.try_acquire(&h).unwrap_err();
        assert_eq!(err.rule_name, "rule-b");

        // rule-a should be back to 1 (rollback happened via guard drop)
        assert_eq!(limiter.current("rule-a", &h), 1);
        assert_eq!(limiter.current("rule-b", &h), 1);
    }
}
