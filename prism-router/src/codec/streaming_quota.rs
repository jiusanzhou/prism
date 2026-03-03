//! Streaming quota enforcement — checks accumulated token usage during streaming
//! and signals stream termination when a quota is exceeded.
//!
//! Works with `UsageExtractorPlugin` (which accumulates usage in ctx.data)
//! and `QuotaRecorderPlugin` (which records final usage at on_log).
//!
//! This plugin runs `on_response_stream_chunk` and checks accumulated tokens
//! against the rate limiter's quotas in real-time.

use std::sync::Arc;

use super::{ProxyContext, ProxyPlugin};
use crate::codec::quota_recorder::{HEADERS_KEY, LIMITER_KEY};
use crate::codec::usage_extractor::{ExtractedUsage, USAGE_KEY};
use crate::ratelimit::{QuotaMetric, RateLimiter};

/// Key to signal that streaming was terminated due to quota.
pub const STREAMING_QUOTA_EXCEEDED_KEY: &str = "prism.streaming_quota.exceeded";

/// Streaming quota enforcement plugin.
pub struct StreamingQuotaPlugin {
    /// How often to check (every N chunks). Default: 1 (every chunk).
    check_interval: u32,
}

impl Default for StreamingQuotaPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingQuotaPlugin {
    pub fn new() -> Self {
        Self { check_interval: 1 }
    }

    pub fn with_interval(interval: u32) -> Self {
        Self {
            check_interval: interval.max(1),
        }
    }

    fn check_quotas(
        &self,
        limiter: &RateLimiter,
        headers: &[(String, String)],
        usage: &ExtractedUsage,
    ) -> Option<(String, QuotaMetric, u64, u64)> {
        // Check each rule's quotas against accumulated usage
        for rule in limiter.rules() {
            if !rule.enabled {
                continue;
            }

            let quotas = rule.effective_quotas();
            for quota in &quotas {
                let current_value = match &quota.metric {
                    QuotaMetric::InputTokens => usage.input_tokens.unwrap_or(0),
                    QuotaMetric::OutputTokens => usage.output_tokens.unwrap_or(0),
                    QuotaMetric::TotalTokens => usage.total_tokens.unwrap_or(0),
                    _ => continue, // Only check token metrics during streaming
                };

                if current_value > 0 {
                    // Peek at the current window value without incrementing
                    let key = RateLimiter::build_key_for_rule(rule, headers);
                    let window_current = limiter.peek(&key, &quota.metric);

                    if window_current + current_value > quota.limit {
                        return Some((
                            rule.name.clone(),
                            quota.metric.clone(),
                            window_current + current_value,
                            quota.limit,
                        ));
                    }
                }
            }
        }
        None
    }
}

impl ProxyPlugin for StreamingQuotaPlugin {
    fn name(&self) -> &str {
        "streaming_quota"
    }

    fn on_response_stream_chunk(
        &self,
        ctx: &mut ProxyContext,
        chunk: &str,
    ) -> Result<Option<String>, String> {
        // Only check periodically
        let chunk_count = ctx
            .get::<u32>("prism.streaming_quota.chunk_count")
            .copied()
            .unwrap_or(0);
        ctx.set("prism.streaming_quota.chunk_count", chunk_count + 1);

        if chunk_count % self.check_interval != 0 {
            return Ok(Some(chunk.to_string()));
        }

        // Get limiter and usage
        let limiter = ctx.get::<Arc<RateLimiter>>(LIMITER_KEY).cloned();
        let usage = ctx.get::<ExtractedUsage>(USAGE_KEY).cloned();
        let headers = ctx
            .get::<Vec<(String, String)>>(HEADERS_KEY)
            .cloned()
            .unwrap_or_default();

        if let (Some(limiter), Some(usage)) = (limiter, usage) {
            if let Some((rule_name, metric, current, limit)) =
                self.check_quotas(&limiter, &headers, &usage)
            {
                tracing::warn!(
                    rule = %rule_name,
                    metric = %metric,
                    current = %current,
                    limit = %limit,
                    "Streaming quota exceeded — terminating stream"
                );

                ctx.set(STREAMING_QUOTA_EXCEEDED_KEY, true);
                ctx.set(
                    "prism.streaming_quota.reason",
                    format!(
                        "quota exceeded: {} {} > {} (rule: {})",
                        metric, current, limit, rule_name
                    ),
                );

                // Send a final SSE event informing the client, then signal end
                let error_event = format!(
                    "data: {{\"error\":{{\"type\":\"quota_exceeded\",\"message\":\"Streaming token quota exceeded for rule '{}'\"}}}}\n\ndata: [DONE]\n\n",
                    rule_name
                );
                return Ok(Some(error_event));
            }
        }

        Ok(Some(chunk.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ratelimit::{Quota, RateLimitRule, RateLimiter};

    fn make_limiter(max_output_tokens: u64) -> Arc<RateLimiter> {
        Arc::new(RateLimiter::new(vec![RateLimitRule {
            name: "test-rule".to_string(),
            key_headers: vec![],
            window_secs: 60,
            max_requests: None,
            max_tokens: None,
            quotas: vec![Quota {
                metric: QuotaMetric::OutputTokens,
                limit: max_output_tokens,
            }],
            overrides: vec![],
            enabled: true,
            status_code: 429,
            message: "Rate limit exceeded".to_string(),
            additional_windows: vec![],
        }]))
    }

    #[test]
    fn test_no_quota_exceeded() {
        let plugin = StreamingQuotaPlugin::new();
        let mut ctx = ProxyContext::new();

        let limiter = make_limiter(1000);
        ctx.set(LIMITER_KEY, limiter);
        ctx.set(HEADERS_KEY, Vec::<(String, String)>::new());
        ctx.set(
            USAGE_KEY,
            ExtractedUsage {
                output_tokens: Some(500),
                ..Default::default()
            },
        );

        let result = plugin
            .on_response_stream_chunk(&mut ctx, "data: {}\n")
            .unwrap();
        assert!(result.is_some());
        assert!(!result.unwrap().contains("quota_exceeded"));
    }

    #[test]
    fn test_quota_exceeded_terminates() {
        let plugin = StreamingQuotaPlugin::new();
        let mut ctx = ProxyContext::new();

        let limiter = make_limiter(100);
        ctx.set(LIMITER_KEY, limiter);
        ctx.set(HEADERS_KEY, Vec::<(String, String)>::new());
        ctx.set(
            USAGE_KEY,
            ExtractedUsage {
                output_tokens: Some(150),
                ..Default::default()
            },
        );

        let result = plugin
            .on_response_stream_chunk(&mut ctx, "data: {}\n")
            .unwrap();
        assert!(result.unwrap().contains("quota_exceeded"));
        assert_eq!(
            ctx.get::<bool>(STREAMING_QUOTA_EXCEEDED_KEY),
            Some(&true)
        );
    }

    #[test]
    fn test_no_limiter_passthrough() {
        let plugin = StreamingQuotaPlugin::new();
        let mut ctx = ProxyContext::new();

        let result = plugin
            .on_response_stream_chunk(&mut ctx, "data: hello\n")
            .unwrap();
        assert_eq!(result, Some("data: hello\n".to_string()));
    }

    #[test]
    fn test_interval_skips() {
        let plugin = StreamingQuotaPlugin::with_interval(3);
        let mut ctx = ProxyContext::new();

        let limiter = make_limiter(10);
        ctx.set(LIMITER_KEY, limiter);
        ctx.set(HEADERS_KEY, Vec::<(String, String)>::new());
        ctx.set(
            USAGE_KEY,
            ExtractedUsage {
                output_tokens: Some(999),
                ..Default::default()
            },
        );

        // Chunk 0: check (exceeded)
        let r0 = plugin
            .on_response_stream_chunk(&mut ctx, "data: {}\n")
            .unwrap();
        assert!(r0.unwrap().contains("quota_exceeded"));
    }
}
