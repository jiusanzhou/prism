mod env_source;
mod file_source;
pub mod registry;
mod types;
#[cfg(feature = "wasm")]
mod wasm_source;

use std::fmt;

pub use env_source::EnvSource;
pub use file_source::{ConfigFormat, FileSource};
pub use registry::{build_registry, build_registry_with_discoveries};
pub use types::*;
#[cfg(feature = "wasm")]
pub use wasm_source::WasmConfigSource;

// ── ConfigError ──

/// Configuration loading error.
#[derive(Debug, Clone)]
pub enum ConfigError {
    /// File / network unreachable.
    NotFound(String),
    /// Parse failure.
    ParseError(String),
    /// Unsupported operation.
    NotSupported(String),
    /// Other error.
    Other(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NotFound(msg) => write!(f, "not found: {}", msg),
            ConfigError::ParseError(msg) => write!(f, "parse error: {}", msg),
            ConfigError::NotSupported(msg) => write!(f, "not supported: {}", msg),
            ConfigError::Other(msg) => write!(f, "config error: {}", msg),
        }
    }
}

impl std::error::Error for ConfigError {}

// ── ConfigEvent ──

/// Configuration change event (foundation for hot-reload).
#[derive(Debug, Clone)]
pub enum ConfigEvent {
    /// Full configuration reload.
    FullReload(PrismConfig),
    /// Provider / Model / Variant changed.
    DiscoveryChanged,
    /// Routing strategy changed.
    RoutingChanged(RoutingConfig),
    /// Rate-limit rules changed.
    RateLimitChanged,
    /// Plugin configuration changed.
    PluginChanged,
}

// ── ConfigSource trait ──

/// Pluggable configuration source trait.
pub trait ConfigSource: Send + Sync {
    /// Source name (for logging).
    fn name(&self) -> &str;

    /// Load the full configuration.
    fn load(&self) -> Result<PrismConfig, ConfigError>;

    /// Whether this source supports change watching.
    fn supports_watch(&self) -> bool {
        false
    }

    /// Watch for configuration changes (hot-reload foundation).
    fn watch(
        &self,
        _callback: Box<dyn Fn(ConfigEvent) + Send + Sync>,
    ) -> Result<(), ConfigError> {
        Err(ConfigError::NotSupported("watch not supported".into()))
    }
}

// ── ConfigPriority ──

/// Priority for merging multiple configuration sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConfigPriority {
    /// Base configuration (overridden by everything).
    Base = 0,
    /// File configuration.
    File = 10,
    /// Remote configuration center.
    Remote = 20,
    /// Environment variables.
    Environment = 30,
    /// Explicit override.
    Override = 40,
}

// ── CompositeSource ──

/// Composite configuration source — merges multiple sources by priority.
pub struct CompositeSource {
    sources: Vec<(ConfigPriority, Box<dyn ConfigSource>)>,
}

impl CompositeSource {
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    pub fn add_source(&mut self, priority: ConfigPriority, source: Box<dyn ConfigSource>) {
        self.sources.push((priority, source));
        self.sources.sort_by_key(|(p, _)| *p);
    }

    /// Get number of registered sources.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }
}

impl Default for CompositeSource {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigSource for CompositeSource {
    fn name(&self) -> &str {
        "composite"
    }

    fn load(&self) -> Result<PrismConfig, ConfigError> {
        if self.sources.is_empty() {
            return Err(ConfigError::NotFound(
                "no config sources registered".into(),
            ));
        }

        // Load from lowest to highest priority; later overrides earlier.
        let mut base_config: Option<PrismConfig> = None;

        for (_, source) in &self.sources {
            match source.load() {
                Ok(config) => {
                    base_config = Some(match base_config {
                        None => config,
                        Some(existing) => merge_configs(existing, config),
                    });
                }
                Err(e) => {
                    // Log but continue — partial failure is ok for composite.
                    tracing::warn!("Config source '{}' failed: {}", source.name(), e);
                }
            }
        }

        base_config.ok_or_else(|| ConfigError::NotFound("all config sources failed".into()))
    }

    fn supports_watch(&self) -> bool {
        self.sources.iter().any(|(_, s)| s.supports_watch())
    }
}

// ── merge_configs ──

/// Merge two configs: `overlay` values take precedence over `base`.
/// - Maps (providers): overlay entries win
/// - Vecs (models, variants, pools): overlay replaces entirely if non-empty
/// - Scalars: overlay wins if non-default
fn merge_configs(base: PrismConfig, overlay: PrismConfig) -> PrismConfig {
    PrismConfig {
        server: if overlay.server.http_port != 8080
            || overlay.server.grpc_port != 9090
            || overlay.server.proxy_port != 8000
        {
            overlay.server
        } else {
            base.server
        },
        providers: {
            let mut merged = base.providers;
            merged.extend(overlay.providers);
            merged
        },
        models: if overlay.models.is_empty() {
            base.models
        } else {
            overlay.models
        },
        variants: if overlay.variants.is_empty() {
            base.variants
        } else {
            overlay.variants
        },
        pools: if overlay.pools.is_empty() {
            base.pools
        } else {
            overlay.pools
        },
        routing: if overlay.routing.default_strategy != "balanced"
            || overlay.routing.balanced.is_some()
        {
            overlay.routing
        } else {
            base.routing
        },
        rate_limits: if overlay.rate_limits.is_empty() {
            base.rate_limits
        } else {
            overlay.rate_limits
        },
        plugins: if overlay.plugins.is_empty() {
            base.plugins
        } else {
            overlay.plugins
        },
        discoveries: if overlay.discoveries.is_empty() {
            base.discoveries
        } else {
            overlay.discoveries
        },
        codec: if overlay.codec.provider_chains.is_empty()
            && overlay.codec.protocol_chains.is_empty()
            && overlay.codec.default_inbound_protocol == "openai"
        {
            base.codec
        } else {
            overlay.codec
        },
    }
}

// ── Backward-compatible load_config ──

/// Load a YAML config file (backward-compatible wrapper around `FileSource`).
pub fn load_config(path: &std::path::Path) -> Result<PrismConfig, String> {
    FileSource::new(path).load().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_config_error_display() {
        assert_eq!(
            ConfigError::NotFound("foo".into()).to_string(),
            "not found: foo"
        );
        assert_eq!(
            ConfigError::ParseError("bar".into()).to_string(),
            "parse error: bar"
        );
        assert_eq!(
            ConfigError::NotSupported("baz".into()).to_string(),
            "not supported: baz"
        );
        assert_eq!(
            ConfigError::Other("qux".into()).to_string(),
            "config error: qux"
        );
    }

    #[test]
    fn test_config_event_variants() {
        // Just ensure all variants can be constructed.
        let _ = ConfigEvent::FullReload(PrismConfig::default());
        let _ = ConfigEvent::DiscoveryChanged;
        let _ = ConfigEvent::RoutingChanged(RoutingConfig::default());
        let _ = ConfigEvent::RateLimitChanged;
        let _ = ConfigEvent::PluginChanged;
    }

    #[test]
    fn test_composite_source_empty() {
        let source = CompositeSource::new();
        assert_eq!(source.source_count(), 0);
        match source.load() {
            Err(ConfigError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {:?}", other),
        }
    }

    /// A simple in-memory config source for testing.
    struct MockSource {
        name: &'static str,
        config: PrismConfig,
    }

    impl ConfigSource for MockSource {
        fn name(&self) -> &str {
            self.name
        }
        fn load(&self) -> Result<PrismConfig, ConfigError> {
            Ok(self.config.clone())
        }
    }

    /// A failing source for testing.
    struct FailSource;

    impl ConfigSource for FailSource {
        fn name(&self) -> &str {
            "fail"
        }
        fn load(&self) -> Result<PrismConfig, ConfigError> {
            Err(ConfigError::Other("boom".into()))
        }
    }

    #[test]
    fn test_composite_source_single() {
        let mut composite = CompositeSource::new();
        let mut config = PrismConfig::default();
        config.server.http_port = 1234;
        composite.add_source(
            ConfigPriority::File,
            Box::new(MockSource {
                name: "mock",
                config,
            }),
        );
        assert_eq!(composite.source_count(), 1);
        let loaded = composite.load().unwrap();
        assert_eq!(loaded.server.http_port, 1234);
    }

    #[test]
    fn test_composite_source_priority_merge() {
        let mut composite = CompositeSource::new();

        // Low priority: set http_port=1111
        let mut low = PrismConfig::default();
        low.server.http_port = 1111;
        composite.add_source(
            ConfigPriority::File,
            Box::new(MockSource {
                name: "low",
                config: low,
            }),
        );

        // High priority: set http_port=2222
        let mut high = PrismConfig::default();
        high.server.http_port = 2222;
        composite.add_source(
            ConfigPriority::Environment,
            Box::new(MockSource {
                name: "high",
                config: high,
            }),
        );

        let loaded = composite.load().unwrap();
        assert_eq!(loaded.server.http_port, 2222);
    }

    #[test]
    fn test_composite_source_partial_failure() {
        let mut composite = CompositeSource::new();

        // Failing source
        composite.add_source(ConfigPriority::Base, Box::new(FailSource));

        // Working source
        let mut config = PrismConfig::default();
        config.server.http_port = 3333;
        composite.add_source(
            ConfigPriority::File,
            Box::new(MockSource {
                name: "ok",
                config,
            }),
        );

        let loaded = composite.load().unwrap();
        assert_eq!(loaded.server.http_port, 3333);
    }

    #[test]
    fn test_merge_configs_providers() {
        let mut base = PrismConfig::default();
        base.providers.insert(
            "openai".into(),
            ProviderConfig {
                provider_type: None,
                api_key: Some("sk-base".into()),
                base_url: "https://base.com".into(),
                timeout_ms: None,
                protocols: vec![],
            },
        );

        let mut overlay = PrismConfig::default();
        overlay.providers.insert(
            "anthropic".into(),
            ProviderConfig {
                provider_type: None,
                api_key: Some("sk-ant".into()),
                base_url: "https://anthropic.com".into(),
                timeout_ms: None,
                protocols: vec![],
            },
        );

        let merged = merge_configs(base, overlay);
        assert_eq!(merged.providers.len(), 2);
        assert!(merged.providers.contains_key("openai"));
        assert!(merged.providers.contains_key("anthropic"));
    }

    #[test]
    fn test_merge_configs_server_override() {
        let mut base = PrismConfig::default();
        base.server.http_port = 3000;

        let mut overlay = PrismConfig::default();
        overlay.server.http_port = 4000;

        let merged = merge_configs(base, overlay);
        assert_eq!(merged.server.http_port, 4000);
    }

    #[test]
    fn test_merge_configs_models_override() {
        let base_yaml = r#"
models:
  - id: m1
    display_name: M1
    quality_tier: A
"#;
        let overlay_yaml = r#"
models:
  - id: m2
    display_name: M2
    quality_tier: S
"#;
        let base: PrismConfig = serde_yaml::from_str(base_yaml).unwrap();
        let overlay: PrismConfig = serde_yaml::from_str(overlay_yaml).unwrap();

        let merged = merge_configs(base, overlay);
        assert_eq!(merged.models.len(), 1);
        assert_eq!(merged.models[0].id, "m2");
    }

    #[test]
    fn test_merge_configs_empty_overlay_keeps_base() {
        let base_yaml = r#"
models:
  - id: m1
    display_name: M1
    quality_tier: A
"#;
        let base: PrismConfig = serde_yaml::from_str(base_yaml).unwrap();
        let overlay = PrismConfig::default();

        let merged = merge_configs(base, overlay);
        assert_eq!(merged.models.len(), 1);
        assert_eq!(merged.models[0].id, "m1");
    }

    #[test]
    fn test_composite_source_supports_watch() {
        let composite = CompositeSource::new();
        assert!(!composite.supports_watch());
    }

    #[test]
    fn test_config_priority_ordering() {
        assert!(ConfigPriority::Base < ConfigPriority::File);
        assert!(ConfigPriority::File < ConfigPriority::Remote);
        assert!(ConfigPriority::Remote < ConfigPriority::Environment);
        assert!(ConfigPriority::Environment < ConfigPriority::Override);
    }

    #[test]
    fn test_merge_configs_new_fields() {
        let base_yaml = r#"
rate_limits:
  - name: global
    window_secs: 60
    max_requests: 1000
plugins:
  - name: p1
    source:
      builtin: p1
    phases: [proxy]
discoveries:
  - name: d1
    type: static
codec:
  default_inbound_protocol: openai
  provider_chains:
    anthropic: [openai-to-anthropic]
"#;
        let base: PrismConfig = serde_yaml::from_str(base_yaml).unwrap();
        let overlay = PrismConfig::default();

        // Empty overlay keeps base for new fields
        let merged = merge_configs(base.clone(), overlay);
        assert_eq!(merged.rate_limits.len(), 1);
        assert_eq!(merged.plugins.len(), 1);
        assert_eq!(merged.discoveries.len(), 1);
        assert_eq!(merged.codec.provider_chains.len(), 1);

        // Non-empty overlay replaces
        let overlay_yaml = r#"
rate_limits:
  - name: overlay-rule
    window_secs: 30
    max_requests: 500
plugins:
  - name: p2
    source:
      wasm: ./p2.wasm
    phases: [route]
"#;
        let overlay: PrismConfig = serde_yaml::from_str(overlay_yaml).unwrap();
        let merged = merge_configs(base, overlay);
        assert_eq!(merged.rate_limits.len(), 1);
        assert_eq!(merged.rate_limits[0].name, "overlay-rule");
        assert_eq!(merged.plugins.len(), 1);
        assert_eq!(merged.plugins[0].name, "p2");
    }
}
