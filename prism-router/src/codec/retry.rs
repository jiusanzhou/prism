//! Retry / Failover plugin — on upstream failure, automatically retries
//! the next variant from the fallback chain.
//!
//! Works with prism-core's `RoutingDecision.fallback_chain` and
//! `ResilienceHandler.on_failure()`.
//!
//! This plugin operates at the `on_response_headers` phase:
//! - Detects upstream error status codes (5xx, 429)
//! - Marks the response for retry with the next fallback variant
//! - The proxy layer checks for retry signals and re-dispatches
//!
//! Configuration (via on_config):
//! ```json
//! {
//!   "max_retries": 2,
//!   "retry_status_codes": [502, 503, 504, 429],
//!   "retry_on_timeout": true
//! }
//! ```

use std::collections::HashSet;

use super::{Action, ProxyContext, ProxyPlugin};

/// Retry config.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries (default: 2).
    pub max_retries: u32,
    /// HTTP status codes that trigger a retry.
    pub retry_status_codes: HashSet<u16>,
    /// Whether to retry on timeout errors.
    pub retry_on_timeout: bool,
    /// Whether the plugin is enabled.
    pub enabled: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            retry_status_codes: [502, 503, 504, 429].into_iter().collect(),
            retry_on_timeout: true,
            enabled: true,
        }
    }
}

/// Keys stored in ProxyContext.data for retry coordination.
pub const RETRY_ATTEMPT_KEY: &str = "prism.retry.attempt";
pub const RETRY_NEXT_VARIANT_KEY: &str = "prism.retry.next_variant";
pub const RETRY_SHOULD_RETRY_KEY: &str = "prism.retry.should_retry";
pub const FALLBACK_CHAIN_KEY: &str = "prism.retry.fallback_chain";

/// Retry / Failover plugin.
pub struct RetryPlugin {
    config: RetryConfig,
}

impl Default for RetryPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl RetryPlugin {
    pub fn new() -> Self {
        Self {
            config: RetryConfig::default(),
        }
    }

    pub fn with_config(config: RetryConfig) -> Self {
        Self { config }
    }

    /// Check if a status code should trigger a retry.
    fn should_retry_status(&self, status: u16) -> bool {
        self.config.retry_status_codes.contains(&status)
    }
}

impl ProxyPlugin for RetryPlugin {
    fn name(&self) -> &str {
        "retry"
    }

    fn on_config(&mut self, config: &serde_json::Value) -> Result<(), String> {
        if let Some(max) = config.get("max_retries").and_then(|v| v.as_u64()) {
            self.config.max_retries = max as u32;
        }
        if let Some(codes) = config.get("retry_status_codes").and_then(|v| v.as_array()) {
            self.config.retry_status_codes = codes
                .iter()
                .filter_map(|v| v.as_u64().map(|n| n as u16))
                .collect();
        }
        if let Some(v) = config.get("retry_on_timeout").and_then(|v| v.as_bool()) {
            self.config.retry_on_timeout = v;
        }
        if let Some(v) = config.get("enabled").and_then(|v| v.as_bool()) {
            self.config.enabled = v;
        }
        Ok(())
    }

    fn on_request_headers(&self, ctx: &mut ProxyContext) -> Action {
        if !self.config.enabled {
            return Action::Continue;
        }

        // Initialize retry attempt counter if not set
        if ctx.get::<u32>(RETRY_ATTEMPT_KEY).is_none() {
            ctx.set(RETRY_ATTEMPT_KEY, 0u32);
        }

        Action::Continue
    }

    fn on_response_headers(&self, ctx: &mut ProxyContext) -> Action {
        if !self.config.enabled {
            return Action::Continue;
        }

        let status = ctx.response_status.unwrap_or(200);
        if !self.should_retry_status(status) {
            return Action::Continue;
        }

        let attempt = ctx.get::<u32>(RETRY_ATTEMPT_KEY).copied().unwrap_or(0);
        if attempt >= self.config.max_retries {
            // Exhausted retries, let the error response through
            tracing::warn!(
                status = %status,
                attempt = %attempt,
                max_retries = %self.config.max_retries,
                "Retry exhausted, returning upstream error"
            );
            return Action::Continue;
        }

        // Get fallback chain
        let fallback_chain = ctx
            .get::<Vec<String>>(FALLBACK_CHAIN_KEY)
            .cloned()
            .unwrap_or_default();

        if fallback_chain.is_empty() {
            tracing::warn!(
                status = %status,
                "No fallback variants available for retry"
            );
            return Action::Continue;
        }

        // Pick next variant from fallback chain
        let next_idx = attempt as usize;
        if next_idx >= fallback_chain.len() {
            tracing::warn!(
                status = %status,
                attempt = %attempt,
                available = %fallback_chain.len(),
                "Fallback chain exhausted"
            );
            return Action::Continue;
        }

        let next_variant = fallback_chain[next_idx].clone();
        tracing::info!(
            status = %status,
            attempt = %(attempt + 1),
            next_variant = %next_variant,
            "Scheduling retry with fallback variant"
        );

        // Signal to the proxy layer that a retry is needed
        ctx.set(RETRY_ATTEMPT_KEY, attempt + 1);
        ctx.set(RETRY_NEXT_VARIANT_KEY, next_variant);
        ctx.set(RETRY_SHOULD_RETRY_KEY, true);

        Action::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_on_502() {
        let plugin = RetryPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.response_status = Some(502);

        // Set up fallback chain
        ctx.set(
            FALLBACK_CHAIN_KEY,
            vec!["variant-b".to_string(), "variant-c".to_string()],
        );
        ctx.set(RETRY_ATTEMPT_KEY, 0u32);

        assert_eq!(plugin.on_response_headers(&mut ctx), Action::Continue);
        assert_eq!(ctx.get::<bool>(RETRY_SHOULD_RETRY_KEY), Some(&true));
        assert_eq!(
            ctx.get::<String>(RETRY_NEXT_VARIANT_KEY),
            Some(&"variant-b".to_string())
        );
        assert_eq!(ctx.get::<u32>(RETRY_ATTEMPT_KEY), Some(&1));
    }

    #[test]
    fn test_retry_on_429() {
        let plugin = RetryPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.response_status = Some(429);
        ctx.set(
            FALLBACK_CHAIN_KEY,
            vec!["variant-b".to_string()],
        );
        ctx.set(RETRY_ATTEMPT_KEY, 0u32);

        assert_eq!(plugin.on_response_headers(&mut ctx), Action::Continue);
        assert_eq!(ctx.get::<bool>(RETRY_SHOULD_RETRY_KEY), Some(&true));
    }

    #[test]
    fn test_no_retry_on_200() {
        let plugin = RetryPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.response_status = Some(200);
        ctx.set(
            FALLBACK_CHAIN_KEY,
            vec!["variant-b".to_string()],
        );
        ctx.set(RETRY_ATTEMPT_KEY, 0u32);

        assert_eq!(plugin.on_response_headers(&mut ctx), Action::Continue);
        assert!(ctx.get::<bool>(RETRY_SHOULD_RETRY_KEY).is_none());
    }

    #[test]
    fn test_no_retry_on_4xx() {
        let plugin = RetryPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.response_status = Some(400);
        ctx.set(
            FALLBACK_CHAIN_KEY,
            vec!["variant-b".to_string()],
        );
        ctx.set(RETRY_ATTEMPT_KEY, 0u32);

        assert_eq!(plugin.on_response_headers(&mut ctx), Action::Continue);
        assert!(ctx.get::<bool>(RETRY_SHOULD_RETRY_KEY).is_none());
    }

    #[test]
    fn test_retry_exhausted() {
        let plugin = RetryPlugin::new(); // max_retries = 2
        let mut ctx = ProxyContext::new();
        ctx.response_status = Some(502);
        ctx.set(
            FALLBACK_CHAIN_KEY,
            vec!["variant-b".to_string()],
        );
        ctx.set(RETRY_ATTEMPT_KEY, 2u32); // already at max

        assert_eq!(plugin.on_response_headers(&mut ctx), Action::Continue);
        assert!(ctx.get::<bool>(RETRY_SHOULD_RETRY_KEY).is_none());
    }

    #[test]
    fn test_fallback_chain_exhausted() {
        let plugin = RetryPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.response_status = Some(502);
        ctx.set(FALLBACK_CHAIN_KEY, vec!["variant-b".to_string()]);
        ctx.set(RETRY_ATTEMPT_KEY, 1u32); // attempt 1, but only 1 fallback

        assert_eq!(plugin.on_response_headers(&mut ctx), Action::Continue);
        assert!(ctx.get::<bool>(RETRY_SHOULD_RETRY_KEY).is_none());
    }

    #[test]
    fn test_no_fallback_chain() {
        let plugin = RetryPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.response_status = Some(502);
        ctx.set(RETRY_ATTEMPT_KEY, 0u32);

        assert_eq!(plugin.on_response_headers(&mut ctx), Action::Continue);
        assert!(ctx.get::<bool>(RETRY_SHOULD_RETRY_KEY).is_none());
    }

    #[test]
    fn test_sequential_retries() {
        let plugin = RetryPlugin::new();
        let fallbacks = vec![
            "variant-b".to_string(),
            "variant-c".to_string(),
        ];

        // First retry
        let mut ctx = ProxyContext::new();
        ctx.response_status = Some(503);
        ctx.set(FALLBACK_CHAIN_KEY, fallbacks.clone());
        ctx.set(RETRY_ATTEMPT_KEY, 0u32);
        plugin.on_response_headers(&mut ctx);
        assert_eq!(
            ctx.get::<String>(RETRY_NEXT_VARIANT_KEY),
            Some(&"variant-b".to_string())
        );

        // Second retry
        let mut ctx2 = ProxyContext::new();
        ctx2.response_status = Some(503);
        ctx2.set(FALLBACK_CHAIN_KEY, fallbacks);
        ctx2.set(RETRY_ATTEMPT_KEY, 1u32);
        plugin.on_response_headers(&mut ctx2);
        assert_eq!(
            ctx2.get::<String>(RETRY_NEXT_VARIANT_KEY),
            Some(&"variant-c".to_string())
        );
    }

    #[test]
    fn test_disabled_plugin() {
        let plugin = RetryPlugin::with_config(RetryConfig {
            enabled: false,
            ..RetryConfig::default()
        });
        let mut ctx = ProxyContext::new();
        ctx.response_status = Some(502);
        ctx.set(FALLBACK_CHAIN_KEY, vec!["variant-b".to_string()]);
        ctx.set(RETRY_ATTEMPT_KEY, 0u32);

        assert_eq!(plugin.on_response_headers(&mut ctx), Action::Continue);
        assert!(ctx.get::<bool>(RETRY_SHOULD_RETRY_KEY).is_none());
    }

    #[test]
    fn test_custom_status_codes() {
        let mut plugin = RetryPlugin::new();
        plugin
            .on_config(&serde_json::json!({
                "retry_status_codes": [500, 502],
                "max_retries": 1
            }))
            .unwrap();

        // 500 should retry
        let mut ctx = ProxyContext::new();
        ctx.response_status = Some(500);
        ctx.set(FALLBACK_CHAIN_KEY, vec!["v-b".to_string()]);
        ctx.set(RETRY_ATTEMPT_KEY, 0u32);
        plugin.on_response_headers(&mut ctx);
        assert_eq!(ctx.get::<bool>(RETRY_SHOULD_RETRY_KEY), Some(&true));

        // 429 should NOT retry (not in custom list)
        let mut ctx2 = ProxyContext::new();
        ctx2.response_status = Some(429);
        ctx2.set(FALLBACK_CHAIN_KEY, vec!["v-b".to_string()]);
        ctx2.set(RETRY_ATTEMPT_KEY, 0u32);
        plugin.on_response_headers(&mut ctx2);
        assert!(ctx2.get::<bool>(RETRY_SHOULD_RETRY_KEY).is_none());
    }
}
