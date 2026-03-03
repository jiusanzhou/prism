//! API Key authentication plugin — validates requests via header-based API keys.
//!
//! Runs in the global plugin chain at `on_request_headers` phase.
//! Rejects requests that don't carry a valid API key.

use std::collections::HashSet;

use super::{Action, ProxyContext, ProxyPlugin};

/// Auth plugin configuration.
#[derive(Debug, Clone)]
struct AuthConfig {
    header: String,
    keys: HashSet<String>,
    strip_prefix: Option<String>,
    skip_paths: HashSet<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            header: "authorization".to_string(),
            keys: HashSet::new(),
            strip_prefix: Some("Bearer ".to_string()),
            skip_paths: ["/health", "/readyz", "/version"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// API Key authentication plugin.
pub struct ApiKeyAuthPlugin {
    config: AuthConfig,
}

impl Default for ApiKeyAuthPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiKeyAuthPlugin {
    pub fn new() -> Self {
        Self {
            config: AuthConfig::default(),
        }
    }

    /// Create with explicit keys (for programmatic setup).
    pub fn with_keys(keys: Vec<String>) -> Self {
        Self {
            config: AuthConfig {
                keys: keys.into_iter().collect(),
                ..AuthConfig::default()
            },
        }
    }

    fn extract_key<'a>(&self, value: &'a str) -> &'a str {
        if let Some(ref prefix) = self.config.strip_prefix {
            value.strip_prefix(prefix.as_str()).unwrap_or(value)
        } else {
            value
        }
    }
}

impl ProxyPlugin for ApiKeyAuthPlugin {
    fn name(&self) -> &str {
        "api_key_auth"
    }

    fn on_config(&mut self, config: &serde_json::Value) -> Result<(), String> {
        if let Some(header) = config.get("header").and_then(|v| v.as_str()) {
            self.config.header = header.to_string();
        }
        if let Some(keys) = config.get("keys").and_then(|v| v.as_array()) {
            self.config.keys = keys
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
        if let Some(prefix) = config.get("strip_prefix") {
            if prefix.is_null() {
                self.config.strip_prefix = None;
            } else if let Some(s) = prefix.as_str() {
                self.config.strip_prefix = Some(s.to_string());
            }
        }
        if let Some(paths) = config.get("skip_paths").and_then(|v| v.as_array()) {
            self.config.skip_paths = paths
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
        if self.config.keys.is_empty() {
            return Err("api_key_auth: no keys configured".to_string());
        }
        Ok(())
    }

    fn on_request_headers(&self, ctx: &mut ProxyContext) -> Action {
        // Skip auth for allowlisted paths
        if self.config.skip_paths.contains(&ctx.request_path) {
            return Action::Continue;
        }

        // If no keys configured, pass through
        if self.config.keys.is_empty() {
            return Action::Continue;
        }

        let header_value = ctx.request_headers.get(&self.config.header).cloned();

        match header_value {
            Some(value) => {
                let key = self.extract_key(&value);
                if self.config.keys.contains(key) {
                    ctx.set("prism.auth.key", key.to_string());
                    Action::Continue
                } else {
                    ctx.response_status = Some(401);
                    ctx.set("prism.auth.error", "invalid API key".to_string());
                    Action::Stop
                }
            }
            None => {
                ctx.response_status = Some(401);
                ctx.set("prism.auth.error", "missing API key".to_string());
                Action::Stop
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_key_bearer() {
        let plugin = ApiKeyAuthPlugin::with_keys(vec!["sk-test123".to_string()]);
        let mut ctx = ProxyContext::new();
        ctx.request_path = "/v1/chat/completions".to_string();
        ctx.request_headers
            .insert("authorization".to_string(), "Bearer sk-test123".to_string());
        assert_eq!(plugin.on_request_headers(&mut ctx), Action::Continue);
        assert_eq!(ctx.get::<String>("prism.auth.key"), Some(&"sk-test123".to_string()));
    }

    #[test]
    fn test_invalid_key() {
        let plugin = ApiKeyAuthPlugin::with_keys(vec!["sk-valid".to_string()]);
        let mut ctx = ProxyContext::new();
        ctx.request_path = "/v1/chat/completions".to_string();
        ctx.request_headers
            .insert("authorization".to_string(), "Bearer sk-wrong".to_string());
        assert_eq!(plugin.on_request_headers(&mut ctx), Action::Stop);
        assert_eq!(ctx.response_status, Some(401));
    }

    #[test]
    fn test_missing_header() {
        let plugin = ApiKeyAuthPlugin::with_keys(vec!["sk-test".to_string()]);
        let mut ctx = ProxyContext::new();
        ctx.request_path = "/v1/chat/completions".to_string();
        assert_eq!(plugin.on_request_headers(&mut ctx), Action::Stop);
    }

    #[test]
    fn test_skip_health_path() {
        let plugin = ApiKeyAuthPlugin::with_keys(vec!["sk-test".to_string()]);
        let mut ctx = ProxyContext::new();
        ctx.request_path = "/health".to_string();
        assert_eq!(plugin.on_request_headers(&mut ctx), Action::Continue);
    }

    #[test]
    fn test_skip_readyz_path() {
        let plugin = ApiKeyAuthPlugin::with_keys(vec!["sk-test".to_string()]);
        let mut ctx = ProxyContext::new();
        ctx.request_path = "/readyz".to_string();
        assert_eq!(plugin.on_request_headers(&mut ctx), Action::Continue);
    }

    #[test]
    fn test_no_keys_passthrough() {
        let plugin = ApiKeyAuthPlugin::new();
        let mut ctx = ProxyContext::new();
        ctx.request_path = "/v1/chat/completions".to_string();
        assert_eq!(plugin.on_request_headers(&mut ctx), Action::Continue);
    }

    #[test]
    fn test_custom_header() {
        let mut plugin = ApiKeyAuthPlugin::new();
        plugin.on_config(&serde_json::json!({
            "header": "x-api-key",
            "keys": ["mykey123"],
            "strip_prefix": null
        })).unwrap();

        let mut ctx = ProxyContext::new();
        ctx.request_path = "/v1/chat/completions".to_string();
        ctx.request_headers.insert("x-api-key".to_string(), "mykey123".to_string());
        assert_eq!(plugin.on_request_headers(&mut ctx), Action::Continue);
    }

    #[test]
    fn test_on_config_no_keys_error() {
        let mut plugin = ApiKeyAuthPlugin::new();
        assert!(plugin.on_config(&serde_json::json!({"keys": []})).is_err());
    }

    #[test]
    fn test_multiple_keys() {
        let plugin = ApiKeyAuthPlugin::with_keys(vec![
            "sk-key1".to_string(), "sk-key2".to_string(), "sk-key3".to_string(),
        ]);
        for key in &["sk-key1", "sk-key2", "sk-key3"] {
            let mut ctx = ProxyContext::new();
            ctx.request_path = "/v1/chat/completions".to_string();
            ctx.request_headers.insert("authorization".to_string(), format!("Bearer {}", key));
            assert_eq!(plugin.on_request_headers(&mut ctx), Action::Continue);
        }
    }
}
