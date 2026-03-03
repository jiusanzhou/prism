//! Rate limiting module — pluggable backend + extensible quota metrics.
//!
//! Provides [`RateLimiter`] with configurable counting backends via
//! [`RateLimitBackend`](backend::RateLimitBackend) and multi-metric quotas via
//! [`QuotaMetric`](quota::QuotaMetric).
//!
//! Built-in backends:
//! - [`LocalBackend`](local::LocalBackend) — in-memory DashMap (default)

pub mod backend;
pub mod local;
pub mod quota;
#[cfg(feature = "redis")]
pub mod redis_backend;

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use backend::RateLimitBackend;
pub use local::LocalBackend;
pub use quota::{Quota, QuotaDenied, QuotaMetric};
#[cfg(feature = "redis")]
pub use redis_backend::{DegradationPolicy, RedisBackend, RedisConfig};

// ════════════════════════════════════════════════
// Configuration
// ════════════════════════════════════════════════

fn default_true() -> bool {
    true
}

fn default_429() -> u16 {
    429
}

fn default_rate_limit_message() -> String {
    "Rate limit exceeded".to_string()
}

/// Differential override for a rate limit rule.
///
/// When the request headers match all entries in `match_headers`, the override's
/// quotas (or legacy `max_requests`/`max_tokens`) replace the rule's defaults.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitOverride {
    /// Match condition: header_name → expected_value.
    /// All entries must match for this override to apply.
    pub match_headers: HashMap<String, String>,
    /// Override quotas — metrics present here replace the rule's defaults.
    #[serde(default)]
    pub quotas: Vec<Quota>,
    /// Legacy: Override max requests (None = keep rule default).
    #[serde(default)]
    pub max_requests: Option<u64>,
    /// Legacy: Override max tokens (None = keep rule default).
    #[serde(default)]
    pub max_tokens: Option<u64>,
}

/// A single rate limit rule.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitRule {
    /// Rule name (used in logs and metrics).
    pub name: String,
    /// Header names to extract and combine into the rate limit key.
    /// Empty = global rate limit (all requests share one key).
    pub key_headers: Vec<String>,
    /// Time window in seconds.
    pub window_secs: u64,
    /// Multi-metric quotas — the new way to configure limits.
    #[serde(default)]
    pub quotas: Vec<Quota>,
    /// Legacy: Maximum requests allowed within the window.
    #[serde(default)]
    pub max_requests: Option<u64>,
    /// Legacy: Maximum tokens allowed within the window.
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// Differential overrides — most-specific match wins.
    #[serde(default)]
    pub overrides: Vec<RateLimitOverride>,
    /// Whether this rule is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// HTTP status code when triggered (default 429).
    #[serde(default = "default_429")]
    pub status_code: u16,
    /// Error message when triggered.
    #[serde(default = "default_rate_limit_message")]
    pub message: String,
}

impl RateLimitRule {
    /// Return effective quotas — merges new `quotas` field with legacy `max_requests`/`max_tokens`.
    ///
    /// If `quotas` already contains a `Requests` entry, `max_requests` is ignored.
    /// If `quotas` already contains a `TotalTokens` entry, `max_tokens` is ignored.
    pub fn effective_quotas(&self) -> Vec<Quota> {
        let mut result = self.quotas.clone();
        if !result.iter().any(|q| q.metric == QuotaMetric::Requests) {
            if let Some(max) = self.max_requests {
                result.push(Quota {
                    metric: QuotaMetric::Requests,
                    limit: max,
                });
            }
        }
        if !result.iter().any(|q| q.metric == QuotaMetric::TotalTokens) {
            if let Some(max) = self.max_tokens {
                result.push(Quota {
                    metric: QuotaMetric::TotalTokens,
                    limit: max,
                });
            }
        }
        result
    }
}

impl RateLimitOverride {
    /// Return effective override quotas — merges new `quotas` with legacy fields.
    pub fn effective_quotas(&self) -> Vec<Quota> {
        let mut result = self.quotas.clone();
        if !result.iter().any(|q| q.metric == QuotaMetric::Requests) {
            if let Some(max) = self.max_requests {
                result.push(Quota {
                    metric: QuotaMetric::Requests,
                    limit: max,
                });
            }
        }
        if !result.iter().any(|q| q.metric == QuotaMetric::TotalTokens) {
            if let Some(max) = self.max_tokens {
                result.push(Quota {
                    metric: QuotaMetric::TotalTokens,
                    limit: max,
                });
            }
        }
        result
    }
}

// ════════════════════════════════════════════════
// Denied result
// ════════════════════════════════════════════════

/// Returned when a request is denied by a rate limit rule.
#[derive(Debug)]
pub struct RateLimitDenied {
    pub rule_name: String,
    pub status_code: u16,
    pub message: String,
    pub retry_after_secs: Option<u64>,
    /// Which metric triggered the denial.
    pub metric: QuotaMetric,
    /// Current value of the metric.
    pub current: u64,
    /// Configured limit.
    pub limit: u64,
}

// ════════════════════════════════════════════════
// Rate limiter
// ════════════════════════════════════════════════

/// The rate limiter — holds all rules and a pluggable counting backend.
pub struct RateLimiter {
    rules: Vec<RateLimitRule>,
    backend: Arc<dyn RateLimitBackend>,
}

impl RateLimiter {
    /// Create a rate limiter with a specific backend.
    pub fn with_backend(rules: Vec<RateLimitRule>, backend: Arc<dyn RateLimitBackend>) -> Self {
        Self { rules, backend }
    }

    /// Create a rate limiter with the default local (in-memory) backend.
    pub fn new(rules: Vec<RateLimitRule>) -> Self {
        Self::with_backend(rules, Arc::new(LocalBackend::new()))
    }

    /// Get the backend name (for diagnostics).
    pub fn backend_name(&self) -> &str {
        self.backend.name()
    }

    /// Check whether the backend is healthy.
    pub fn is_backend_healthy(&self) -> bool {
        self.backend.is_healthy()
    }

    /// Check all enabled rules against the given request headers.
    /// Returns the first rule that is violated, or `Ok(())` if all pass.
    pub fn check(&self, headers: &[(String, String)]) -> Result<(), RateLimitDenied> {
        for rule in &self.rules {
            if !rule.enabled {
                continue;
            }

            let key = Self::build_key(rule, headers);
            let quotas = Self::resolve_quotas(rule, headers);

            if quotas.is_empty() {
                continue;
            }

            if let Err(denied) = self.backend.check_quotas(&key, rule.window_secs, &quotas) {
                return Err(RateLimitDenied {
                    rule_name: rule.name.clone(),
                    status_code: rule.status_code,
                    message: rule.message.clone(),
                    retry_after_secs: Some(denied.retry_after_secs),
                    metric: denied.metric,
                    current: denied.current,
                    limit: denied.limit,
                });
            }
        }

        Ok(())
    }

    /// Resolve effective quotas by checking overrides.
    /// Most-specific match wins (by match_headers count). Ties broken by declaration order.
    /// Override quotas replace matching metrics from the rule's defaults.
    pub(crate) fn resolve_quotas(
        rule: &RateLimitRule,
        headers: &[(String, String)],
    ) -> Vec<Quota> {
        let base_quotas = rule.effective_quotas();

        let mut best: Option<&RateLimitOverride> = None;
        let mut best_specificity: usize = 0;

        for ovr in &rule.overrides {
            let all_match = ovr.match_headers.iter().all(|(name, expected)| {
                headers
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("")
                    == expected.as_str()
            });

            if all_match && ovr.match_headers.len() > best_specificity {
                best_specificity = ovr.match_headers.len();
                best = Some(ovr);
            }
        }

        if let Some(ovr) = best {
            let ovr_quotas = ovr.effective_quotas();
            // Merge: override quotas replace matching metrics, keep rest from base
            let mut merged = Vec::new();
            for bq in &base_quotas {
                if let Some(oq) = ovr_quotas.iter().find(|q| q.metric == bq.metric) {
                    merged.push(oq.clone());
                } else {
                    merged.push(bq.clone());
                }
            }
            // Add override-only quotas not in base
            for oq in &ovr_quotas {
                if !base_quotas.iter().any(|bq| bq.metric == oq.metric) {
                    merged.push(oq.clone());
                }
            }
            merged
        } else {
            base_quotas
        }
    }

    /// Record a metric value for a specific rule + headers combination.
    pub fn record(
        &self,
        rule_name: &str,
        headers: &[(String, String)],
        metric: &QuotaMetric,
        value: u64,
    ) {
        if let Some(rule) = self.rules.iter().find(|r| r.name == rule_name) {
            let key = Self::build_key(rule, headers);
            self.backend.record_metric(&key, metric, value);
        }
    }

    /// Legacy: Record token usage. Delegates to `record` with `TotalTokens`.
    pub fn record_tokens(&self, rule_name: &str, headers: &[(String, String)], tokens: u64) {
        self.record(rule_name, headers, &QuotaMetric::TotalTokens, tokens);
    }

    /// Build the rate limit key: `rule_name:header_val1:header_val2:...`
    pub(crate) fn build_key(rule: &RateLimitRule, headers: &[(String, String)]) -> String {
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

    /// Replace the current rules (hot-reload).
    pub fn update_rules(&mut self, rules: Vec<RateLimitRule>) {
        self.rules = rules;
    }

    /// Get a reference to the current rules.
    pub fn rules(&self) -> &[RateLimitRule] {
        &self.rules
    }

    /// Clean up expired window entries.
    pub fn cleanup(&self) {
        let max_window = self.rules.iter().map(|r| r.window_secs).max().unwrap_or(60);
        self.backend.cleanup(max_window);
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ──

    fn make_rule(
        name: &str,
        key_headers: Vec<&str>,
        window_secs: u64,
        max_requests: u64,
    ) -> RateLimitRule {
        RateLimitRule {
            name: name.to_string(),
            key_headers: key_headers.into_iter().map(|s| s.to_string()).collect(),
            window_secs,
            max_requests: Some(max_requests),
            max_tokens: None,
            quotas: vec![],
            overrides: vec![],
            enabled: true,
            status_code: 429,
            message: "Rate limit exceeded".to_string(),
        }
    }

    fn make_quota_rule(
        name: &str,
        key_headers: Vec<&str>,
        window_secs: u64,
        quotas: Vec<Quota>,
    ) -> RateLimitRule {
        RateLimitRule {
            name: name.to_string(),
            key_headers: key_headers.into_iter().map(|s| s.to_string()).collect(),
            window_secs,
            max_requests: None,
            max_tokens: None,
            quotas,
            overrides: vec![],
            enabled: true,
            status_code: 429,
            message: "Rate limit exceeded".to_string(),
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // ── Legacy compat tests (max_requests / max_tokens) ──

    #[test]
    fn test_effective_quotas_legacy() {
        let rule = make_rule("test", vec![], 60, 100);
        let quotas = rule.effective_quotas();
        assert_eq!(quotas.len(), 1);
        assert_eq!(quotas[0].metric, QuotaMetric::Requests);
        assert_eq!(quotas[0].limit, 100);
    }

    #[test]
    fn test_effective_quotas_legacy_with_tokens() {
        let mut rule = make_rule("test", vec![], 60, 100);
        rule.max_tokens = Some(50000);
        let quotas = rule.effective_quotas();
        assert_eq!(quotas.len(), 2);
        assert!(quotas.iter().any(|q| q.metric == QuotaMetric::Requests && q.limit == 100));
        assert!(quotas.iter().any(|q| q.metric == QuotaMetric::TotalTokens && q.limit == 50000));
    }

    #[test]
    fn test_effective_quotas_new_format() {
        let rule = make_quota_rule("test", vec![], 60, vec![
            Quota { metric: QuotaMetric::Requests, limit: 100 },
            Quota { metric: QuotaMetric::InputTokens, limit: 30000 },
        ]);
        let quotas = rule.effective_quotas();
        assert_eq!(quotas.len(), 2);
        assert_eq!(quotas[0].metric, QuotaMetric::Requests);
        assert_eq!(quotas[1].metric, QuotaMetric::InputTokens);
    }

    #[test]
    fn test_effective_quotas_new_overrides_legacy() {
        // quotas has Requests, so max_requests should be ignored
        let mut rule = make_quota_rule("test", vec![], 60, vec![
            Quota { metric: QuotaMetric::Requests, limit: 200 },
        ]);
        rule.max_requests = Some(100); // should be ignored
        rule.max_tokens = Some(50000); // should be added (no TotalTokens in quotas)
        let quotas = rule.effective_quotas();
        assert_eq!(quotas.len(), 2);
        assert!(quotas.iter().any(|q| q.metric == QuotaMetric::Requests && q.limit == 200));
        assert!(quotas.iter().any(|q| q.metric == QuotaMetric::TotalTokens && q.limit == 50000));
    }

    // ── RateLimitRule serde tests ──

    #[test]
    fn test_rule_serde_legacy_format() {
        let json = r#"{
            "name": "test",
            "key_headers": [],
            "window_secs": 60,
            "max_requests": 100,
            "max_tokens": 50000
        }"#;
        let rule: RateLimitRule = serde_json::from_str(json).unwrap();
        let quotas = rule.effective_quotas();
        assert_eq!(quotas.len(), 2);
    }

    #[test]
    fn test_rule_serde_new_format() {
        let json = r#"{
            "name": "test",
            "key_headers": ["x-model"],
            "window_secs": 60,
            "quotas": [
                {"metric": "requests", "limit": 100},
                {"metric": "total_tokens", "limit": 50000},
                {"metric": "input_tokens", "limit": 30000}
            ]
        }"#;
        let rule: RateLimitRule = serde_json::from_str(json).unwrap();
        assert_eq!(rule.quotas.len(), 3);
        let eff = rule.effective_quotas();
        assert_eq!(eff.len(), 3);
    }

    // ── build_key tests ──

    #[test]
    fn test_build_key_global() {
        let rule = make_rule("global", vec![], 60, 100);
        assert_eq!(RateLimiter::build_key(&rule, &headers(&[])), "global");
    }

    #[test]
    fn test_build_key_single_header() {
        let rule = make_rule("per-model", vec!["x-model"], 60, 100);
        assert_eq!(
            RateLimiter::build_key(&rule, &headers(&[("x-model", "gpt-4")])),
            "per-model:gpt-4"
        );
    }

    #[test]
    fn test_build_key_multi_header() {
        let rule = make_rule("model-app", vec!["x-model", "x-app-id"], 60, 100);
        assert_eq!(
            RateLimiter::build_key(&rule, &headers(&[("x-model", "gpt-4"), ("x-app-id", "app-123")])),
            "model-app:gpt-4:app-123"
        );
    }

    #[test]
    fn test_build_key_missing_header() {
        let rule = make_rule("per-model", vec!["x-model"], 60, 100);
        assert_eq!(RateLimiter::build_key(&rule, &headers(&[])), "per-model:");
    }

    // ── RateLimiter tests (legacy compat) ──

    #[test]
    fn test_limiter_empty_rules() {
        let limiter = RateLimiter::new(vec![]);
        assert!(limiter.check(&headers(&[])).is_ok());
    }

    #[test]
    fn test_limiter_single_rule_allows() {
        let limiter = RateLimiter::new(vec![make_rule("global", vec![], 60, 10)]);
        for _ in 0..10 {
            assert!(limiter.check(&headers(&[])).is_ok());
        }
    }

    #[test]
    fn test_limiter_single_rule_denies() {
        let limiter = RateLimiter::new(vec![make_rule("global", vec![], 60, 2)]);
        assert!(limiter.check(&headers(&[])).is_ok());
        assert!(limiter.check(&headers(&[])).is_ok());
        let denied = limiter.check(&headers(&[])).unwrap_err();
        assert_eq!(denied.rule_name, "global");
        assert_eq!(denied.status_code, 429);
        assert_eq!(denied.metric, QuotaMetric::Requests);
        assert_eq!(denied.current, 2);
        assert_eq!(denied.limit, 2);
    }

    #[test]
    fn test_limiter_disabled_rule_skipped() {
        let mut rule = make_rule("disabled", vec![], 60, 1);
        rule.enabled = false;
        let limiter = RateLimiter::new(vec![rule]);
        for _ in 0..100 {
            assert!(limiter.check(&headers(&[])).is_ok());
        }
    }

    #[test]
    fn test_limiter_header_based_isolation() {
        let limiter = RateLimiter::new(vec![make_rule("per-model", vec!["x-model"], 60, 2)]);
        let h_gpt4 = headers(&[("x-model", "gpt-4")]);
        let h_claude = headers(&[("x-model", "claude-3")]);
        assert!(limiter.check(&h_gpt4).is_ok());
        assert!(limiter.check(&h_gpt4).is_ok());
        assert!(limiter.check(&h_gpt4).is_err());
        assert!(limiter.check(&h_claude).is_ok());
    }

    #[test]
    fn test_limiter_token_recording_legacy() {
        let mut rule = make_rule("tok", vec![], 60, 100);
        rule.max_tokens = Some(500);
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[]);
        assert!(limiter.check(&h).is_ok());
        limiter.record_tokens("tok", &h, 499);
        assert!(limiter.check(&h).is_ok());
        limiter.record_tokens("tok", &h, 1);
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_limiter_update_rules() {
        let mut limiter = RateLimiter::new(vec![make_rule("old", vec![], 60, 1)]);
        assert!(limiter.check(&headers(&[])).is_ok());
        assert!(limiter.check(&headers(&[])).is_err());
        limiter.update_rules(vec![make_rule("new", vec![], 60, 10)]);
        assert_eq!(limiter.rules()[0].name, "new");
        assert!(limiter.check(&headers(&[])).is_ok());
    }

    // ── Multi-metric quota tests ──

    #[test]
    fn test_limiter_multi_metric() {
        let rule = make_quota_rule("multi", vec![], 60, vec![
            Quota { metric: QuotaMetric::Requests, limit: 100 },
            Quota { metric: QuotaMetric::InputTokens, limit: 500 },
            Quota { metric: QuotaMetric::OutputTokens, limit: 300 },
        ]);
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[]);

        assert!(limiter.check(&h).is_ok());
        limiter.record("multi", &h, &QuotaMetric::InputTokens, 200);
        limiter.record("multi", &h, &QuotaMetric::OutputTokens, 299);
        assert!(limiter.check(&h).is_ok());

        limiter.record("multi", &h, &QuotaMetric::OutputTokens, 1);
        let denied = limiter.check(&h).unwrap_err();
        assert_eq!(denied.metric, QuotaMetric::OutputTokens);
        assert_eq!(denied.current, 300);
        assert_eq!(denied.limit, 300);
    }

    #[test]
    fn test_limiter_cost_micros() {
        let rule = make_quota_rule("cost", vec!["x-app"], 60, vec![
            Quota { metric: QuotaMetric::Requests, limit: 1000 },
            Quota { metric: QuotaMetric::CostMicros, limit: 100000 },
        ]);
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-app", "app-1")]);

        assert!(limiter.check(&h).is_ok());
        limiter.record("cost", &h, &QuotaMetric::CostMicros, 99999);
        assert!(limiter.check(&h).is_ok());
        limiter.record("cost", &h, &QuotaMetric::CostMicros, 1);
        let denied = limiter.check(&h).unwrap_err();
        assert_eq!(denied.metric, QuotaMetric::CostMicros);
    }

    // ── Override tests (quota-aware) ──

    fn make_override_quotas(
        match_headers: &[(&str, &str)],
        quotas: Vec<Quota>,
    ) -> RateLimitOverride {
        RateLimitOverride {
            match_headers: match_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            quotas,
            max_requests: None,
            max_tokens: None,
        }
    }

    fn make_override_legacy(
        match_headers: &[(&str, &str)],
        max_requests: Option<u64>,
        max_tokens: Option<u64>,
    ) -> RateLimitOverride {
        RateLimitOverride {
            match_headers: match_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            quotas: vec![],
            max_requests,
            max_tokens,
        }
    }

    #[test]
    fn test_override_legacy_still_works() {
        let mut rule = make_rule("per-model", vec!["x-model"], 60, 100);
        rule.overrides = vec![make_override_legacy(&[("x-model", "gpt-4")], Some(200), None)];
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4")]);
        for _ in 0..200 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_quota_format() {
        let mut rule = make_quota_rule("per-model", vec!["x-model"], 60, vec![
            Quota { metric: QuotaMetric::Requests, limit: 100 },
            Quota { metric: QuotaMetric::TotalTokens, limit: 50000 },
        ]);
        rule.overrides = vec![make_override_quotas(
            &[("x-model", "gpt-4")],
            vec![
                Quota { metric: QuotaMetric::Requests, limit: 50 },
                // TotalTokens not overridden — keeps 50000
            ],
        )];

        let resolved = RateLimiter::resolve_quotas(&rule, &headers(&[("x-model", "gpt-4")]));
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().any(|q| q.metric == QuotaMetric::Requests && q.limit == 50));
        assert!(resolved.iter().any(|q| q.metric == QuotaMetric::TotalTokens && q.limit == 50000));
    }

    #[test]
    fn test_override_most_specific_wins() {
        let mut rule = make_rule("model-app", vec!["x-model", "x-app-id"], 60, 100);
        rule.overrides = vec![
            make_override_legacy(&[("x-model", "gpt-4")], Some(150), None),
            make_override_legacy(&[("x-model", "gpt-4"), ("x-app-id", "app-b")], Some(200), None),
        ];
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4"), ("x-app-id", "app-b")]);
        for _ in 0..200 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_no_match_uses_default() {
        let mut rule = make_rule("per-model", vec!["x-model"], 60, 5);
        rule.overrides = vec![make_override_legacy(&[("x-model", "gpt-4")], Some(200), None)];
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "claude-3")]);
        for _ in 0..5 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    // ── Backend injection tests ──

    #[test]
    fn test_with_backend_constructor() {
        let backend = Arc::new(LocalBackend::new());
        let limiter = RateLimiter::with_backend(
            vec![make_rule("test", vec![], 60, 2)],
            backend,
        );
        assert_eq!(limiter.backend_name(), "local");
        assert!(limiter.check(&headers(&[])).is_ok());
        assert!(limiter.check(&headers(&[])).is_ok());
        assert!(limiter.check(&headers(&[])).is_err());
    }

    #[test]
    fn test_new_uses_local_backend() {
        let limiter = RateLimiter::new(vec![]);
        assert_eq!(limiter.backend_name(), "local");
        assert!(limiter.is_backend_healthy());
    }

    #[test]
    fn test_cleanup_no_panic() {
        let limiter = RateLimiter::new(vec![make_rule("test", vec![], 60, 10)]);
        assert!(limiter.check(&headers(&[])).is_ok());
        limiter.cleanup();
    }
}
