//! Rate limiting module — pluggable backend architecture.
//!
//! Provides [`RateLimiter`] with configurable counting backends via
//! [`RateLimitBackend`](backend::RateLimitBackend).
//!
//! Built-in backends:
//! - [`LocalBackend`](local::LocalBackend) — in-memory DashMap (default)

pub mod backend;
pub mod local;
#[cfg(feature = "redis")]
pub mod redis_backend;

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use backend::RateLimitBackend;
pub use local::LocalBackend;
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
/// `max_requests` and/or `max_tokens` replace the rule's defaults.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitOverride {
    /// Match condition: header_name → expected_value.
    /// All entries must match for this override to apply.
    pub match_headers: HashMap<String, String>,
    /// Override max requests (None = keep rule default).
    pub max_requests: Option<u64>,
    /// Override max tokens (None = keep rule default).
    pub max_tokens: Option<u64>,
}

/// A single rate limit rule.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitRule {
    /// Rule name (used in logs and metrics).
    pub name: String,
    /// Header names to extract and combine into the rate limit key.
    /// Empty = global rate limit (all requests share one key).
    /// `["x-model"]` = per-model rate limit.
    /// `["x-model", "x-app-id"]` = per model+app rate limit.
    pub key_headers: Vec<String>,
    /// Time window in seconds.
    pub window_secs: u64,
    /// Maximum requests allowed within the window.
    pub max_requests: u64,
    /// Maximum tokens allowed within the window (optional, requires response-phase write-back).
    pub max_tokens: Option<u64>,
    /// Differential overrides — most-specific match wins (by match_headers count).
    /// When multiple overrides match, the one with the most conditions takes priority.
    /// Ties are broken by declaration order (first wins).
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
    /// Backward-compatible — same behavior as the original constructor.
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
            let (eff_max_requests, eff_max_tokens) = Self::resolve_limits(rule, headers);

            if let Err(retry_after) = self.backend.check_and_increment(
                &key,
                rule.window_secs,
                eff_max_requests,
                eff_max_tokens,
            ) {
                return Err(RateLimitDenied {
                    rule_name: rule.name.clone(),
                    status_code: rule.status_code,
                    message: rule.message.clone(),
                    retry_after_secs: Some(retry_after),
                });
            }
        }

        Ok(())
    }

    /// Resolve effective limits by checking overrides.
    /// Most-specific match wins (by match_headers count). Ties broken by declaration order.
    /// Returns (max_requests, max_tokens) with override values merged over defaults.
    pub(crate) fn resolve_limits(
        rule: &RateLimitRule,
        headers: &[(String, String)],
    ) -> (u64, Option<u64>) {
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
            let max_requests = ovr.max_requests.unwrap_or(rule.max_requests);
            let max_tokens = ovr.max_tokens.or(rule.max_tokens);
            (max_requests, max_tokens)
        } else {
            (rule.max_requests, rule.max_tokens)
        }
    }

    /// Record token usage for a specific rule + headers combination.
    pub fn record_tokens(&self, rule_name: &str, headers: &[(String, String)], tokens: u64) {
        if let Some(rule) = self.rules.iter().find(|r| r.name == rule_name) {
            let key = Self::build_key(rule, headers);
            self.backend.record_tokens(&key, tokens);
        }
    }

    /// Build the rate limit key: `rule_name:header_val1:header_val2:...`
    /// Missing headers contribute an empty string segment.
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

    // ── Helper ──

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
            max_requests,
            max_tokens: None,
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

    // ── RateLimitRule serde tests ──

    #[test]
    fn test_rule_serde_full() {
        let json = r#"{
            "name": "test-rule",
            "key_headers": ["x-model", "x-app-id"],
            "window_secs": 60,
            "max_requests": 100,
            "max_tokens": 10000,
            "overrides": [
                {
                    "match_headers": {"x-model": "gpt-4"},
                    "max_requests": 200,
                    "max_tokens": 5000
                }
            ],
            "enabled": false,
            "status_code": 503,
            "message": "custom message"
        }"#;

        let rule: RateLimitRule = serde_json::from_str(json).unwrap();
        assert_eq!(rule.name, "test-rule");
        assert_eq!(rule.key_headers, vec!["x-model", "x-app-id"]);
        assert_eq!(rule.window_secs, 60);
        assert_eq!(rule.max_requests, 100);
        assert_eq!(rule.max_tokens, Some(10000));
        assert_eq!(rule.overrides.len(), 1);
        assert_eq!(
            rule.overrides[0].match_headers.get("x-model"),
            Some(&"gpt-4".to_string())
        );
        assert_eq!(rule.overrides[0].max_requests, Some(200));
        assert_eq!(rule.overrides[0].max_tokens, Some(5000));
        assert!(!rule.enabled);
        assert_eq!(rule.status_code, 503);
        assert_eq!(rule.message, "custom message");

        let serialized = serde_json::to_string(&rule).unwrap();
        let deserialized: RateLimitRule = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.name, rule.name);
        assert_eq!(deserialized.overrides.len(), 1);
    }

    #[test]
    fn test_rule_serde_defaults() {
        let json = r#"{
            "name": "minimal",
            "key_headers": [],
            "window_secs": 10,
            "max_requests": 5
        }"#;

        let rule: RateLimitRule = serde_json::from_str(json).unwrap();
        assert!(rule.enabled);
        assert_eq!(rule.status_code, 429);
        assert_eq!(rule.message, "Rate limit exceeded");
        assert!(rule.max_tokens.is_none());
        assert!(rule.overrides.is_empty());
    }

    // ── build_key tests ──

    #[test]
    fn test_build_key_global() {
        let rule = make_rule("global", vec![], 60, 100);
        let key = RateLimiter::build_key(&rule, &headers(&[]));
        assert_eq!(key, "global");
    }

    #[test]
    fn test_build_key_single_header() {
        let rule = make_rule("per-model", vec!["x-model"], 60, 100);
        let key = RateLimiter::build_key(&rule, &headers(&[("x-model", "gpt-4")]));
        assert_eq!(key, "per-model:gpt-4");
    }

    #[test]
    fn test_build_key_multi_header() {
        let rule = make_rule("model-app", vec!["x-model", "x-app-id"], 60, 100);
        let key = RateLimiter::build_key(
            &rule,
            &headers(&[("x-model", "gpt-4"), ("x-app-id", "app-123")]),
        );
        assert_eq!(key, "model-app:gpt-4:app-123");
    }

    #[test]
    fn test_build_key_missing_header() {
        let rule = make_rule("per-model", vec!["x-model"], 60, 100);
        let key = RateLimiter::build_key(&rule, &headers(&[]));
        assert_eq!(key, "per-model:");
    }

    #[test]
    fn test_build_key_partial_headers() {
        let rule = make_rule("multi", vec!["x-model", "x-app-id"], 60, 100);
        let key = RateLimiter::build_key(&rule, &headers(&[("x-model", "gpt-4")]));
        assert_eq!(key, "multi:gpt-4:");
    }

    // ── RateLimiter tests ──

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
        let result = limiter.check(&headers(&[]));
        assert!(result.is_err());
        let denied = result.unwrap_err();
        assert_eq!(denied.rule_name, "global");
        assert_eq!(denied.status_code, 429);
        assert_eq!(denied.message, "Rate limit exceeded");
        assert!(denied.retry_after_secs.is_some());
    }

    #[test]
    fn test_limiter_multi_rules_first_triggers() {
        let limiter = RateLimiter::new(vec![
            make_rule("strict", vec![], 60, 1),
            make_rule("relaxed", vec![], 60, 100),
        ]);
        assert!(limiter.check(&headers(&[])).is_ok());
        let result = limiter.check(&headers(&[]));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().rule_name, "strict");
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
        assert!(limiter.check(&h_claude).is_ok());
        assert!(limiter.check(&h_claude).is_err());
    }

    #[test]
    fn test_limiter_missing_header_uses_empty_key() {
        let limiter = RateLimiter::new(vec![make_rule("per-model", vec!["x-model"], 60, 2)]);
        let h_empty = headers(&[]);
        let h_gpt4 = headers(&[("x-model", "gpt-4")]);
        assert!(limiter.check(&h_empty).is_ok());
        assert!(limiter.check(&h_empty).is_ok());
        assert!(limiter.check(&h_empty).is_err());
        assert!(limiter.check(&h_gpt4).is_ok());
    }

    #[test]
    fn test_limiter_custom_status_code() {
        let mut rule = make_rule("custom", vec![], 60, 1);
        rule.status_code = 503;
        rule.message = "Service overloaded".to_string();
        let limiter = RateLimiter::new(vec![rule]);
        assert!(limiter.check(&headers(&[])).is_ok());
        let denied = limiter.check(&headers(&[])).unwrap_err();
        assert_eq!(denied.status_code, 503);
        assert_eq!(denied.message, "Service overloaded");
    }

    #[test]
    fn test_limiter_update_rules() {
        let mut limiter = RateLimiter::new(vec![make_rule("old", vec![], 60, 1)]);
        assert!(limiter.check(&headers(&[])).is_ok());
        assert!(limiter.check(&headers(&[])).is_err());
        limiter.update_rules(vec![make_rule("new", vec![], 60, 10)]);
        assert_eq!(limiter.rules().len(), 1);
        assert_eq!(limiter.rules()[0].name, "new");
        assert!(limiter.check(&headers(&[])).is_ok());
    }

    #[test]
    fn test_limiter_rules_accessor() {
        let rules = vec![
            make_rule("a", vec![], 60, 10),
            make_rule("b", vec!["x-model"], 30, 5),
        ];
        let limiter = RateLimiter::new(rules);
        assert_eq!(limiter.rules().len(), 2);
        assert_eq!(limiter.rules()[0].name, "a");
        assert_eq!(limiter.rules()[1].name, "b");
    }

    #[test]
    fn test_limiter_token_recording() {
        let mut rule = make_rule("token-rule", vec!["x-app-id"], 60, 100);
        rule.max_tokens = Some(500);
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-app-id", "app-1")]);
        assert!(limiter.check(&h).is_ok());
        limiter.record_tokens("token-rule", &h, 499);
        assert!(limiter.check(&h).is_ok());
        limiter.record_tokens("token-rule", &h, 1);
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_limiter_record_tokens_unknown_rule() {
        let limiter = RateLimiter::new(vec![make_rule("known", vec![], 60, 10)]);
        limiter.record_tokens("unknown", &headers(&[]), 100);
    }

    #[test]
    fn test_limiter_cleanup() {
        let limiter = RateLimiter::new(vec![make_rule("test", vec![], 60, 10)]);
        assert!(limiter.check(&headers(&[])).is_ok());
        limiter.cleanup();
    }

    // ── Override tests ──

    fn make_override(
        match_headers: &[(&str, &str)],
        max_requests: Option<u64>,
        max_tokens: Option<u64>,
    ) -> RateLimitOverride {
        RateLimitOverride {
            match_headers: match_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            max_requests,
            max_tokens,
        }
    }

    #[test]
    fn test_override_exact_match() {
        let mut rule = make_rule("per-model", vec!["x-model"], 60, 100);
        rule.overrides = vec![make_override(&[("x-model", "gpt-4")], Some(200), None)];
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4")]);
        for _ in 0..200 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_partial_header_match() {
        let mut rule = make_rule("model-app", vec!["x-model", "x-app-id"], 60, 100);
        rule.overrides = vec![make_override(&[("x-model", "claude-3")], Some(50), None)];
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "claude-3"), ("x-app-id", "app-a")]);
        for _ in 0..50 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_same_specificity_first_wins() {
        let mut rule = make_rule("per-model", vec!["x-model"], 60, 100);
        rule.overrides = vec![
            make_override(&[("x-model", "gpt-4")], Some(10), None),
            make_override(&[("x-model", "gpt-4")], Some(200), None),
        ];
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4")]);
        for _ in 0..10 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_no_match_uses_default() {
        let mut rule = make_rule("per-model", vec!["x-model"], 60, 5);
        rule.overrides = vec![make_override(&[("x-model", "gpt-4")], Some(200), None)];
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "claude-3")]);
        for _ in 0..5 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_only_max_requests_keeps_default_tokens() {
        let mut rule = make_rule("test", vec!["x-model"], 60, 100);
        rule.max_tokens = Some(1000);
        rule.overrides = vec![make_override(&[("x-model", "gpt-4")], Some(200), None)];
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4")]);
        assert!(limiter.check(&h).is_ok());
        limiter.record_tokens("test", &h, 1000);
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_only_max_tokens() {
        let mut rule = make_rule("test", vec!["x-model"], 60, 100);
        rule.max_tokens = Some(1000);
        rule.overrides = vec![make_override(&[("x-model", "gpt-4")], None, Some(500))];
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4")]);
        assert!(limiter.check(&h).is_ok());
        limiter.record_tokens("test", &h, 500);
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_multi_header_condition() {
        let mut rule = make_rule("model-app", vec!["x-model", "x-app-id"], 60, 100);
        rule.overrides = vec![make_override(
            &[("x-model", "gpt-4"), ("x-app-id", "app-b")],
            Some(200),
            None,
        )];
        let limiter = RateLimiter::new(vec![rule]);
        let h_match = headers(&[("x-model", "gpt-4"), ("x-app-id", "app-b")]);
        for _ in 0..200 {
            assert!(limiter.check(&h_match).is_ok());
        }
        assert!(limiter.check(&h_match).is_err());
        let h_partial = headers(&[("x-model", "gpt-4"), ("x-app-id", "app-a")]);
        for _ in 0..100 {
            assert!(limiter.check(&h_partial).is_ok());
        }
        assert!(limiter.check(&h_partial).is_err());
    }

    #[test]
    fn test_resolve_limits_directly() {
        let mut rule = make_rule("test", vec!["x-model"], 60, 100);
        rule.max_tokens = Some(2000);
        rule.overrides = vec![
            make_override(&[("x-model", "gpt-4")], Some(200), Some(1000)),
            make_override(&[("x-model", "claude-3")], Some(50), None),
        ];
        let (req, tok) = RateLimiter::resolve_limits(&rule, &headers(&[("x-model", "gpt-4")]));
        assert_eq!(req, 200);
        assert_eq!(tok, Some(1000));
        let (req, tok) = RateLimiter::resolve_limits(&rule, &headers(&[("x-model", "claude-3")]));
        assert_eq!(req, 50);
        assert_eq!(tok, Some(2000));
        let (req, tok) = RateLimiter::resolve_limits(&rule, &headers(&[("x-model", "llama")]));
        assert_eq!(req, 100);
        assert_eq!(tok, Some(2000));
    }

    #[test]
    fn test_override_most_specific_wins_over_order() {
        let mut rule = make_rule("model-app", vec!["x-model", "x-app-id"], 60, 100);
        rule.overrides = vec![
            make_override(&[("x-model", "gpt-4")], Some(150), None),
            make_override(
                &[("x-model", "gpt-4"), ("x-app-id", "app-b")],
                Some(200),
                None,
            ),
        ];
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4"), ("x-app-id", "app-b")]);
        for _ in 0..200 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_most_specific_reverse_order() {
        let mut rule = make_rule("model-app", vec!["x-model", "x-app-id"], 60, 100);
        rule.overrides = vec![
            make_override(
                &[("x-model", "gpt-4"), ("x-app-id", "app-b")],
                Some(200),
                None,
            ),
            make_override(&[("x-model", "gpt-4")], Some(150), None),
        ];
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4"), ("x-app-id", "app-b")]);
        for _ in 0..200 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_less_specific_fallback() {
        let mut rule = make_rule("model-app", vec!["x-model", "x-app-id"], 60, 100);
        rule.overrides = vec![
            make_override(&[("x-model", "gpt-4")], Some(150), None),
            make_override(
                &[("x-model", "gpt-4"), ("x-app-id", "app-b")],
                Some(200),
                None,
            ),
        ];
        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4"), ("x-app-id", "app-a")]);
        for _ in 0..150 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_resolve_limits_most_specific() {
        let mut rule = make_rule("test", vec!["x-model", "x-app-id"], 60, 100);
        rule.max_tokens = Some(2000);
        rule.overrides = vec![
            make_override(&[("x-model", "gpt-4")], Some(150), Some(1500)),
            make_override(
                &[("x-model", "gpt-4"), ("x-app-id", "app-b")],
                Some(200),
                Some(3000),
            ),
            make_override(&[("x-model", "claude-3")], Some(50), None),
        ];
        let (req, tok) = RateLimiter::resolve_limits(
            &rule,
            &headers(&[("x-model", "gpt-4"), ("x-app-id", "app-b")]),
        );
        assert_eq!(req, 200);
        assert_eq!(tok, Some(3000));
        let (req, tok) = RateLimiter::resolve_limits(
            &rule,
            &headers(&[("x-model", "gpt-4"), ("x-app-id", "app-a")]),
        );
        assert_eq!(req, 150);
        assert_eq!(tok, Some(1500));
        let (req, tok) = RateLimiter::resolve_limits(
            &rule,
            &headers(&[("x-model", "claude-3"), ("x-app-id", "app-x")]),
        );
        assert_eq!(req, 50);
        assert_eq!(tok, Some(2000));
        let (req, tok) = RateLimiter::resolve_limits(
            &rule,
            &headers(&[("x-model", "llama"), ("x-app-id", "app-x")]),
        );
        assert_eq!(req, 100);
        assert_eq!(tok, Some(2000));
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
        assert!(limiter.is_backend_healthy());
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
}
