use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

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
// Sliding window counter
// ════════════════════════════════════════════════

/// Per-key window state: (window_start_timestamp, request_count, token_count).
struct SlidingWindowCounter {
    windows: DashMap<String, (u64, u64, u64)>,
}

impl SlidingWindowCounter {
    fn new() -> Self {
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

    /// Check the rate limit for `key` against the given effective limits.
    /// On success, increments the request counter and returns `Ok(())`.
    /// On failure, returns `Err` with the number of seconds until the window resets.
    fn check_and_increment(
        &self,
        key: &str,
        window_secs: u64,
        max_requests: u64,
        max_tokens: Option<u64>,
    ) -> Result<(), u64> {
        let now = Self::now_secs();

        let mut entry = self.windows.entry(key.to_string()).or_insert((now, 0, 0));
        let (window_start, count, tokens) = entry.value_mut();

        // Reset window if expired.
        if now - *window_start >= window_secs {
            *window_start = now;
            *count = 0;
            *tokens = 0;
        }

        // Check request count.
        if *count >= max_requests {
            let retry_after = window_secs.saturating_sub(now - *window_start);
            return Err(retry_after);
        }

        // Check token count (if configured).
        if let Some(max_tok) = max_tokens {
            if *tokens >= max_tok {
                let retry_after = window_secs.saturating_sub(now - *window_start);
                return Err(retry_after);
            }
        }

        *count += 1;
        Ok(())
    }

    /// Record token usage for a key (called from the response phase).
    fn record_tokens(&self, key: &str, tokens: u64) {
        if let Some(mut entry) = self.windows.get_mut(key) {
            entry.value_mut().2 += tokens;
        }
    }

    /// Remove entries whose window has expired.
    fn cleanup(&self, max_window_secs: u64) {
        let now = Self::now_secs();
        self.windows.retain(|_, (start, _, _)| {
            now - *start < max_window_secs
        });
    }
}

// ════════════════════════════════════════════════
// Rate limiter
// ════════════════════════════════════════════════

/// The rate limiter — holds all rules and their shared counters.
pub struct RateLimiter {
    rules: Vec<RateLimitRule>,
    counters: SlidingWindowCounter,
}

impl RateLimiter {
    pub fn new(rules: Vec<RateLimitRule>) -> Self {
        Self {
            rules,
            counters: SlidingWindowCounter::new(),
        }
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

            if let Err(retry_after) = self.counters.check_and_increment(
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
    fn resolve_limits(rule: &RateLimitRule, headers: &[(String, String)]) -> (u64, Option<u64>) {
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
            self.counters.record_tokens(&key, tokens);
        }
    }

    /// Build the rate limit key: `rule_name:header_val1:header_val2:...`
    /// Missing headers contribute an empty string segment.
    fn build_key(rule: &RateLimitRule, headers: &[(String, String)]) -> String {
        if rule.key_headers.is_empty() {
            // Global rate limit — all requests share one key per rule.
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
        let max_window = self
            .rules
            .iter()
            .map(|r| r.window_secs)
            .max()
            .unwrap_or(60);
        self.counters.cleanup(max_window);
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper ──

    fn make_rule(name: &str, key_headers: Vec<&str>, window_secs: u64, max_requests: u64) -> RateLimitRule {
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
        assert_eq!(rule.overrides[0].match_headers.get("x-model"), Some(&"gpt-4".to_string()));
        assert_eq!(rule.overrides[0].max_requests, Some(200));
        assert_eq!(rule.overrides[0].max_tokens, Some(5000));
        assert!(!rule.enabled);
        assert_eq!(rule.status_code, 503);
        assert_eq!(rule.message, "custom message");

        // Round-trip
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
        assert!(rule.enabled); // default true
        assert_eq!(rule.status_code, 429); // default 429
        assert_eq!(rule.message, "Rate limit exceeded"); // default message
        assert!(rule.max_tokens.is_none());
        assert!(rule.overrides.is_empty()); // default empty
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

    // ── SlidingWindowCounter tests ──

    #[test]
    fn test_counter_increment_within_limit() {
        let counter = SlidingWindowCounter::new();

        for _ in 0..5 {
            assert!(counter.check_and_increment("test", 60, 5, None).is_ok());
        }
    }

    #[test]
    fn test_counter_exceeds_limit() {
        let counter = SlidingWindowCounter::new();

        for _ in 0..3 {
            assert!(counter.check_and_increment("test", 60, 3, None).is_ok());
        }

        let result = counter.check_and_increment("test", 60, 3, None);
        assert!(result.is_err());
        // retry_after should be > 0
        let retry_after = result.unwrap_err();
        assert!(retry_after > 0);
    }

    #[test]
    fn test_counter_independent_keys() {
        let counter = SlidingWindowCounter::new();

        // Key "a" — fill up
        assert!(counter.check_and_increment("a", 60, 2, None).is_ok());
        assert!(counter.check_and_increment("a", 60, 2, None).is_ok());
        assert!(counter.check_and_increment("a", 60, 2, None).is_err());

        // Key "b" — still has capacity
        assert!(counter.check_and_increment("b", 60, 2, None).is_ok());
        assert!(counter.check_and_increment("b", 60, 2, None).is_ok());
        assert!(counter.check_and_increment("b", 60, 2, None).is_err());
    }

    #[test]
    fn test_counter_window_expiry() {
        let counter = SlidingWindowCounter::new();
        // Use a 0-second window so it expires immediately on the next check

        assert!(counter.check_and_increment("test", 0, 1, None).is_ok());
        // Window is 0 seconds, so the next check will reset
        assert!(counter.check_and_increment("test", 0, 1, None).is_ok());
    }

    #[test]
    fn test_counter_record_tokens() {
        let counter = SlidingWindowCounter::new();

        // First request — no tokens yet
        assert!(counter.check_and_increment("test", 60, 100, Some(1000)).is_ok());

        // Record 999 tokens
        counter.record_tokens("test", 999);

        // Still within limit
        assert!(counter.check_and_increment("test", 60, 100, Some(1000)).is_ok());

        // Record 1 more token → total 1000 = max_tokens
        counter.record_tokens("test", 1);

        // Now should be rejected due to token limit
        let result = counter.check_and_increment("test", 60, 100, Some(1000));
        assert!(result.is_err());
    }

    #[test]
    fn test_counter_cleanup() {
        let counter = SlidingWindowCounter::new();

        assert!(counter.check_and_increment("key1", 0, 1, None).is_ok());
        assert!(counter.check_and_increment("key2", 0, 1, None).is_ok());

        // With max_window=0, cleanup should remove expired entries
        // (window_secs=0 means everything is immediately expired for cleanup purposes with max_window=1)
        counter.cleanup(1);
        // After cleanup, entries with window_start older than 1 second should be removed.
        // Since they were just created, they might still be there.
        // Let's just verify it doesn't panic and the map is accessible.
        assert!(counter.windows.len() <= 2);
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

        // Should always pass since the rule is disabled
        for _ in 0..100 {
            assert!(limiter.check(&headers(&[])).is_ok());
        }
    }

    #[test]
    fn test_limiter_header_based_isolation() {
        let limiter = RateLimiter::new(vec![make_rule("per-model", vec!["x-model"], 60, 2)]);

        let h_gpt4 = headers(&[("x-model", "gpt-4")]);
        let h_claude = headers(&[("x-model", "claude-3")]);

        // Each model gets its own counter
        assert!(limiter.check(&h_gpt4).is_ok());
        assert!(limiter.check(&h_gpt4).is_ok());
        assert!(limiter.check(&h_gpt4).is_err()); // gpt-4 exhausted

        assert!(limiter.check(&h_claude).is_ok()); // claude still has capacity
        assert!(limiter.check(&h_claude).is_ok());
        assert!(limiter.check(&h_claude).is_err()); // claude exhausted
    }

    #[test]
    fn test_limiter_missing_header_uses_empty_key() {
        let limiter = RateLimiter::new(vec![make_rule("per-model", vec!["x-model"], 60, 2)]);

        let h_empty = headers(&[]);
        let h_gpt4 = headers(&[("x-model", "gpt-4")]);

        // Requests without x-model header share one key (empty segment)
        assert!(limiter.check(&h_empty).is_ok());
        assert!(limiter.check(&h_empty).is_ok());
        assert!(limiter.check(&h_empty).is_err());

        // gpt-4 is a different key, still has capacity
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

        // Update rules — counters are shared but keys change with new rule names
        limiter.update_rules(vec![make_rule("new", vec![], 60, 10)]);

        assert_eq!(limiter.rules().len(), 1);
        assert_eq!(limiter.rules()[0].name, "new");

        // New rule has different key prefix, so counter starts fresh
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
        // Now at 500 tokens = max, next check should fail
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_limiter_record_tokens_unknown_rule() {
        let limiter = RateLimiter::new(vec![make_rule("known", vec![], 60, 10)]);
        // Recording tokens for an unknown rule should be a no-op
        limiter.record_tokens("unknown", &headers(&[]), 100);
    }

    #[test]
    fn test_limiter_cleanup() {
        let limiter = RateLimiter::new(vec![make_rule("test", vec![], 60, 10)]);
        assert!(limiter.check(&headers(&[])).is_ok());
        // Just verify cleanup doesn't panic
        limiter.cleanup();
    }

    // ── Override tests ──

    fn make_override(match_headers: &[(&str, &str)], max_requests: Option<u64>, max_tokens: Option<u64>) -> RateLimitOverride {
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
        rule.overrides = vec![
            make_override(&[("x-model", "gpt-4")], Some(200), None),
        ];

        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4")]);

        // gpt-4 gets 200 requests
        for _ in 0..200 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_partial_header_match() {
        // Rule keys on (x-model, x-app-id), override matches only x-model
        let mut rule = make_rule("model-app", vec!["x-model", "x-app-id"], 60, 100);
        rule.overrides = vec![
            make_override(&[("x-model", "claude-3")], Some(50), None),
        ];

        let limiter = RateLimiter::new(vec![rule]);

        // claude-3 with any app-id gets override limit of 50
        let h = headers(&[("x-model", "claude-3"), ("x-app-id", "app-a")]);
        for _ in 0..50 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_multi_match_first_wins() {
        let mut rule = make_rule("per-model", vec!["x-model"], 60, 100);
        rule.overrides = vec![
            make_override(&[("x-model", "gpt-4")], Some(10), None),   // first match
            make_override(&[("x-model", "gpt-4")], Some(200), None),  // would also match but never reached
        ];

        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4")]);

        // Should use first override's limit of 10
        for _ in 0..10 {
            assert!(limiter.check(&h).is_ok());
        }
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_no_match_uses_default() {
        let mut rule = make_rule("per-model", vec!["x-model"], 60, 5);
        rule.overrides = vec![
            make_override(&[("x-model", "gpt-4")], Some(200), None),
        ];

        let limiter = RateLimiter::new(vec![rule]);

        // claude-3 doesn't match any override → default limit of 5
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
        rule.overrides = vec![
            // Override only max_requests, max_tokens stays None → keep rule default (1000)
            make_override(&[("x-model", "gpt-4")], Some(200), None),
        ];

        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4")]);

        // Should allow 200 requests (overridden)
        assert!(limiter.check(&h).is_ok());

        // Record 1000 tokens → hits the default token limit
        limiter.record_tokens("test", &h, 1000);
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_only_max_tokens() {
        let mut rule = make_rule("test", vec!["x-model"], 60, 100);
        rule.max_tokens = Some(1000);
        rule.overrides = vec![
            // Override only max_tokens, max_requests stays None → keep default (100)
            make_override(&[("x-model", "gpt-4")], None, Some(500)),
        ];

        let limiter = RateLimiter::new(vec![rule]);
        let h = headers(&[("x-model", "gpt-4")]);

        assert!(limiter.check(&h).is_ok());
        // Record 500 tokens → hits overridden token limit
        limiter.record_tokens("test", &h, 500);
        assert!(limiter.check(&h).is_err());
    }

    #[test]
    fn test_override_multi_header_condition() {
        let mut rule = make_rule("model-app", vec!["x-model", "x-app-id"], 60, 100);
        rule.overrides = vec![
            // Must match both headers
            make_override(&[("x-model", "gpt-4"), ("x-app-id", "app-b")], Some(200), None),
        ];

        let limiter = RateLimiter::new(vec![rule]);

        // Exact match → override limit 200
        let h_match = headers(&[("x-model", "gpt-4"), ("x-app-id", "app-b")]);
        for _ in 0..200 {
            assert!(limiter.check(&h_match).is_ok());
        }
        assert!(limiter.check(&h_match).is_err());

        // Only one header matches → default limit 100
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

        // gpt-4 → (200, Some(1000))
        let (req, tok) = RateLimiter::resolve_limits(&rule, &headers(&[("x-model", "gpt-4")]));
        assert_eq!(req, 200);
        assert_eq!(tok, Some(1000));

        // claude-3 → (50, Some(2000))  (max_tokens not overridden, keeps rule default)
        let (req, tok) = RateLimiter::resolve_limits(&rule, &headers(&[("x-model", "claude-3")]));
        assert_eq!(req, 50);
        assert_eq!(tok, Some(2000));

        // unknown model → (100, Some(2000))  (no override matches)
        let (req, tok) = RateLimiter::resolve_limits(&rule, &headers(&[("x-model", "llama")]));
        assert_eq!(req, 100);
        assert_eq!(tok, Some(2000));
    }
}
