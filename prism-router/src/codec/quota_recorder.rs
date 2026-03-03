//! Quota recorder plugin — records extracted usage to the rate limiter.
//!
//! Runs in the `on_log` phase (after `UsageExtractorPlugin` has stored usage
//! in `ProxyContext.data`). Reads [`ExtractedUsage`] and records each metric
//! to the rate limiter.
//!
//! This plugin needs a reference to the `RateLimiter` and the request headers
//! that were used for rate limit key construction. These are passed via the
//! `ProxyContext.data` map.

use std::sync::Arc;

use super::{ProxyContext, ProxyPlugin};
use super::usage_extractor::{ExtractedUsage, USAGE_KEY};
use crate::ratelimit::{QuotaMetric, RateLimiter};

/// Key for the request headers snapshot in ProxyContext data.
pub const HEADERS_KEY: &str = "prism.request_headers";

/// Key for the rate limiter reference in ProxyContext data.
pub const LIMITER_KEY: &str = "prism.rate_limiter";

/// Quota recorder plugin.
pub struct QuotaRecorderPlugin;

impl Default for QuotaRecorderPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl QuotaRecorderPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl ProxyPlugin for QuotaRecorderPlugin {
    fn name(&self) -> &str {
        "quota_recorder"
    }

    fn on_log(&self, ctx: &mut ProxyContext) {
        // Read extracted usage
        let usage = match ctx.get::<ExtractedUsage>(USAGE_KEY) {
            Some(u) if u.has_data() => u.clone(),
            _ => return,
        };

        // Read rate limiter
        let limiter = match ctx.get::<Arc<RateLimiter>>(LIMITER_KEY) {
            Some(l) => Arc::clone(l),
            None => return,
        };

        // Read request headers for key construction
        let headers = ctx
            .get::<Vec<(String, String)>>(HEADERS_KEY)
            .cloned()
            .unwrap_or_default();

        // Record each metric to all enabled rules
        for rule in limiter.rules() {
            if !rule.enabled {
                continue;
            }
            if let Some(input) = usage.input_tokens {
                limiter.record(&rule.name, &headers, &QuotaMetric::InputTokens, input);
            }
            if let Some(output) = usage.output_tokens {
                limiter.record(&rule.name, &headers, &QuotaMetric::OutputTokens, output);
            }
            if let Some(total) = usage.total_tokens {
                limiter.record(&rule.name, &headers, &QuotaMetric::TotalTokens, total);
            }
            // Record custom metrics
            for (name, value) in &usage.metrics {
                limiter.record(
                    &rule.name,
                    &headers,
                    &QuotaMetric::Custom(name.clone()),
                    *value,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ProxyContext;
    use crate::ratelimit::{Quota, QuotaMetric, RateLimitRule, RateLimiter};

    fn make_rule(name: &str, quotas: Vec<Quota>) -> RateLimitRule {
        RateLimitRule {
            name: name.to_string(),
            key_headers: vec![],
            window_secs: 60,
            max_requests: None,
            max_tokens: None,
            quotas,
            overrides: vec![],
            enabled: true,
            status_code: 429,
            message: "Rate limit exceeded".to_string(),
            additional_windows: vec![],
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn test_quota_recorder_records_usage() {
        let limiter = Arc::new(RateLimiter::new(vec![make_rule(
            "test",
            vec![
                Quota { metric: QuotaMetric::Requests, limit: 1000 },
                Quota { metric: QuotaMetric::InputTokens, limit: 100000 },
                Quota { metric: QuotaMetric::OutputTokens, limit: 50000 },
            ],
        )]));

        let plugin = QuotaRecorderPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.set(USAGE_KEY, ExtractedUsage {
            input_tokens: Some(42),
            output_tokens: Some(128),
            total_tokens: Some(170),
            metrics: Default::default(),
        });
        ctx.set(LIMITER_KEY, Arc::clone(&limiter));
        ctx.set(HEADERS_KEY, headers(&[]));

        plugin.on_log(&mut ctx);

        // Verify: consuming 42 input tokens + checking quota should pass
        // (we've already recorded, so the check sees the updated count)
        let h = headers(&[]);
        // Request should still pass (well under limits)
        assert!(limiter.check(&h).is_ok());
    }

    #[test]
    fn test_quota_recorder_no_usage_noop() {
        let limiter = Arc::new(RateLimiter::new(vec![make_rule(
            "test",
            vec![Quota { metric: QuotaMetric::InputTokens, limit: 100 }],
        )]));

        let plugin = QuotaRecorderPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.set(LIMITER_KEY, Arc::clone(&limiter));
        // No usage set — should be a no-op
        plugin.on_log(&mut ctx);
    }

    #[test]
    fn test_quota_recorder_no_limiter_noop() {
        let plugin = QuotaRecorderPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.set(USAGE_KEY, ExtractedUsage {
            input_tokens: Some(42),
            output_tokens: None,
            total_tokens: None,
            metrics: Default::default(),
        });
        // No limiter set — should be a no-op
        plugin.on_log(&mut ctx);
    }

    #[test]
    fn test_quota_recorder_custom_metrics() {
        let limiter = Arc::new(RateLimiter::new(vec![make_rule(
            "aigc",
            vec![
                Quota { metric: QuotaMetric::Custom("steps".to_string()), limit: 1000 },
            ],
        )]));

        let plugin = QuotaRecorderPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.set(USAGE_KEY, ExtractedUsage {
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            metrics: [("steps".to_string(), 50)].into(),
        });
        ctx.set(LIMITER_KEY, Arc::clone(&limiter));
        ctx.set(HEADERS_KEY, headers(&[]));

        plugin.on_log(&mut ctx);

        // Should have recorded 50 steps — still under 1000 limit
        assert!(limiter.check(&headers(&[])).is_ok());
    }

    #[test]
    fn test_quota_recorder_triggers_limit() {
        let limiter = Arc::new(RateLimiter::new(vec![make_rule(
            "strict",
            vec![
                Quota { metric: QuotaMetric::Requests, limit: 1000 },
                Quota { metric: QuotaMetric::InputTokens, limit: 100 },
            ],
        )]));

        let plugin = QuotaRecorderPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.set(USAGE_KEY, ExtractedUsage {
            input_tokens: Some(101), // over limit
            output_tokens: None,
            total_tokens: None,
            metrics: Default::default(),
        });
        ctx.set(LIMITER_KEY, Arc::clone(&limiter));
        ctx.set(HEADERS_KEY, headers(&[]));

        plugin.on_log(&mut ctx);

        // Next check should fail — input_tokens at limit
        let denied = limiter.check(&headers(&[])).unwrap_err();
        assert_eq!(denied.metric, QuotaMetric::InputTokens);
    }
}
