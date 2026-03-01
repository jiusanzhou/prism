use std::collections::HashMap;
use super::{ConfigError, ConfigSource, PrismConfig, ProviderConfig};

/// Environment variable configuration source.
///
/// Prefix: `PRISM_` (default)
///
/// Mapping rules:
/// - `PRISM_HTTP_PORT=8080`             → `server.http_port`
/// - `PRISM_GRPC_PORT=9090`             → `server.grpc_port`
/// - `PRISM_PROXY_PORT=8000`            → `server.proxy_port`
/// - `PRISM_LISTEN=0.0.0.0:8080`        → `server.listen`
/// - `PRISM_DEFAULT_STRATEGY=balanced`  → `routing.default_strategy`
/// - `PRISM_PROVIDER_<ID>_BASE_URL=`    → `providers.<id>.base_url`
/// - `PRISM_PROVIDER_<ID>_API_KEY=`     → `providers.<id>.api_key`
#[allow(dead_code)]
pub struct EnvSource {
    prefix: String,
}

#[allow(dead_code)]
impl EnvSource {
    pub fn new() -> Self {
        Self {
            prefix: "PRISM_".to_string(),
        }
    }

    pub fn with_prefix(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }
}

impl Default for EnvSource {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigSource for EnvSource {
    fn name(&self) -> &str {
        "env"
    }

    fn load(&self) -> Result<PrismConfig, ConfigError> {
        let mut config = PrismConfig::default();

        // Server settings
        if let Ok(v) = std::env::var(format!("{}HTTP_PORT", self.prefix)) {
            if let Ok(port) = v.parse() {
                config.server.http_port = port;
            }
        }
        if let Ok(v) = std::env::var(format!("{}GRPC_PORT", self.prefix)) {
            if let Ok(port) = v.parse() {
                config.server.grpc_port = port;
            }
        }
        if let Ok(v) = std::env::var(format!("{}PROXY_PORT", self.prefix)) {
            if let Ok(port) = v.parse() {
                config.server.proxy_port = port;
            }
        }
        if let Ok(v) = std::env::var(format!("{}LISTEN", self.prefix)) {
            config.server.listen = v;
        }

        // Routing
        if let Ok(v) = std::env::var(format!("{}DEFAULT_STRATEGY", self.prefix)) {
            config.routing.default_strategy = v;
        }

        // Dynamic provider discovery from env vars
        // PRISM_PROVIDER_OPENAI_BASE_URL, PRISM_PROVIDER_OPENAI_API_KEY etc.
        let provider_prefix = format!("{}PROVIDER_", self.prefix);
        for (key, value) in std::env::vars() {
            if key.starts_with(&provider_prefix) {
                let remainder = &key[provider_prefix.len()..];
                // Parse: <PROVIDER_ID>_<FIELD>
                if let Some(pos) = remainder.rfind('_') {
                    let provider_id = remainder[..pos].to_lowercase();
                    let field = &remainder[pos + 1..];

                    let entry = config.providers.entry(provider_id).or_insert_with(|| {
                        ProviderConfig {
                            provider_type: None,
                            api_key: None,
                            base_url: String::new(),
                            timeout_ms: None,
                            protocols: vec![],
                            headers: HashMap::new(),
                        }
                    });

                    match field {
                        "URL" | "BASEURL" => {
                            entry.base_url = value;
                        }
                        "KEY" | "APIKEY" => {
                            entry.api_key = Some(value);
                        }
                        "TYPE" => {
                            entry.provider_type = Some(value);
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_source_http_port() {
        std::env::set_var("PRISM_TEST_SRC_HTTP_PORT", "4000");
        let source = EnvSource::with_prefix("PRISM_TEST_SRC_");
        let config = source.load().unwrap();
        assert_eq!(config.server.http_port, 4000);
        std::env::remove_var("PRISM_TEST_SRC_HTTP_PORT");
    }

    #[test]
    fn test_env_source_provider() {
        std::env::set_var("PRISM_ENV2_PROVIDER_OPENAI_BASEURL", "https://api.openai.com/v1");
        std::env::set_var("PRISM_ENV2_PROVIDER_OPENAI_APIKEY", "sk-test");
        let source = EnvSource::with_prefix("PRISM_ENV2_");
        let config = source.load().unwrap();
        let provider = config.providers.get("openai").expect("openai provider");
        assert_eq!(provider.base_url, "https://api.openai.com/v1");
        assert_eq!(provider.api_key.as_deref(), Some("sk-test"));
        std::env::remove_var("PRISM_ENV2_PROVIDER_OPENAI_BASEURL");
        std::env::remove_var("PRISM_ENV2_PROVIDER_OPENAI_APIKEY");
    }

    #[test]
    fn test_env_source_custom_prefix() {
        std::env::set_var("MYAPP_HTTP_PORT", "5000");
        let source = EnvSource::with_prefix("MYAPP_");
        assert_eq!(source.prefix(), "MYAPP_");
        let config = source.load().unwrap();
        assert_eq!(config.server.http_port, 5000);
        std::env::remove_var("MYAPP_HTTP_PORT");
    }

    #[test]
    fn test_env_source_no_env_vars() {
        // Use a unique prefix that won't match anything
        let source = EnvSource::with_prefix("PRISM_NOEXIST_XYZ_");
        let config = source.load().unwrap();
        // Should return default config
        assert_eq!(config.server.http_port, 8080);
        assert!(config.providers.is_empty());
    }

    #[test]
    fn test_env_source_default_name() {
        let source = EnvSource::new();
        assert_eq!(source.name(), "env");
        assert_eq!(source.prefix(), "PRISM_");
    }
}
