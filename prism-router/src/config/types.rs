use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use prism_core::types::*;

/// Top-level configuration file structure.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrismConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    #[serde(default)]
    pub variants: Vec<VariantConfig>,
    #[serde(default)]
    pub pools: Vec<PoolConfig>,
    #[serde(default)]
    pub routing: RoutingConfig,

    /// Rate limiting rules.
    #[serde(default)]
    pub rate_limits: Vec<RateLimitConfig>,

    /// Plugin descriptors (WASM / builtin).
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,

    /// External discovery source configurations.
    #[serde(default)]
    pub discoveries: Vec<DiscoveryConfig>,

    /// Codec / protocol conversion configuration.
    #[serde(default)]
    pub codec: CodecConfig,

    /// Access log configuration.
    #[serde(default)]
    pub access_log: AccessLogConfig,
}

/// Server-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    #[serde(default = "default_grpc_port")]
    pub grpc_port: u16,
    #[serde(default = "default_proxy_port")]
    pub proxy_port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            http_port: default_http_port(),
            grpc_port: default_grpc_port(),
            proxy_port: default_proxy_port(),
        }
    }
}

fn default_listen() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_http_port() -> u16 {
    8080
}

fn default_grpc_port() -> u16 {
    9090
}

fn default_proxy_port() -> u16 {
    8000
}

/// Provider configuration — map key is the provider id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(rename = "type", default)]
    pub provider_type: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    pub base_url: String,
    #[serde(default)]
    pub timeout_ms: Option<u32>,
    #[serde(default)]
    pub protocols: Vec<String>,
    /// Custom headers to send with every request to this provider.
    /// When set, these take precedence over the default `Authorization: Bearer <api_key>`.
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

/// Model configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub id: String,
    pub display_name: String,
    pub quality_tier: QualityTier,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    #[serde(default)]
    pub pricing: ModelPricing,
    #[serde(default)]
    pub specialties: Vec<TaskType>,
}

/// Variant configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantConfig {
    pub id: String,
    pub model_id: String,
    pub provider_id: String,
    #[serde(default)]
    pub weight: Option<f32>,
    /// Account id for multi-account routing.
    #[serde(default)]
    pub account_id: Option<String>,
    /// Base URL override for self-hosted models.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Deployment name (for Azure/GCP etc).
    #[serde(default)]
    pub deployment_name: Option<String>,
    /// Upstream model name (vendor-specific, e.g. "deepseek-reasoner" for "deepseek-r1").
    #[serde(default)]
    pub upstream_model_name: Option<String>,
    /// Pricing override.
    #[serde(default)]
    pub pricing_override: Option<ModelPricing>,
}

/// Pool configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    pub id: String,
    pub name: String,
    pub variants: Vec<String>,
    #[serde(default)]
    pub selection_mode: Option<SelectionMode>,
}

/// Routing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    #[serde(default = "default_strategy")]
    pub default_strategy: String,
    #[serde(default)]
    pub balanced: Option<BalancedConfig>,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            default_strategy: default_strategy(),
            balanced: None,
        }
    }
}

fn default_strategy() -> String {
    "balanced".to_string()
}

/// Balanced strategy weights.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalancedConfig {
    #[serde(default = "default_cost_weight")]
    pub cost_weight: f32,
    #[serde(default = "default_quality_weight")]
    pub quality_weight: f32,
    #[serde(default = "default_latency_weight")]
    pub latency_weight: f32,
}

fn default_cost_weight() -> f32 {
    0.3
}
fn default_quality_weight() -> f32 {
    0.5
}
fn default_latency_weight() -> f32 {
    0.2
}

impl Default for BalancedConfig {
    fn default() -> Self {
        Self {
            cost_weight: 0.3,
            quality_weight: 0.5,
            latency_weight: 0.2,
        }
    }
}

// ════════════════════════════════════════════════
// Rate limiting configuration
// ════════════════════════════════════════════════

fn default_true() -> bool {
    true
}

/// Rate limit rule configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Rule name (used in logs and metrics).
    pub name: String,
    /// Header names to extract and combine into the rate limit key.
    /// Empty = global rate limit (all requests share one key).
    #[serde(default)]
    pub key_headers: Vec<String>,
    /// Time window in seconds.
    pub window_secs: u64,
    /// Maximum requests allowed within the window.
    pub max_requests: u64,
    /// Maximum tokens allowed within the window (optional).
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// Differential overrides — most-specific match wins.
    #[serde(default)]
    pub overrides: Vec<RateLimitOverrideConfig>,
    /// Whether this rule is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Differential override for a rate limit rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitOverrideConfig {
    /// Match condition: header_name → expected_value.
    pub match_headers: HashMap<String, String>,
    /// Override max requests (None = keep rule default).
    pub max_requests: Option<u64>,
    /// Override max tokens (None = keep rule default).
    pub max_tokens: Option<u64>,
}

// ════════════════════════════════════════════════
// Plugin configuration
// ════════════════════════════════════════════════

fn default_priority() -> u32 {
    100
}

/// Plugin configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Plugin name (unique identifier).
    pub name: String,
    /// Where the plugin code comes from.
    pub source: PluginSourceConfig,
    /// Which phases this plugin participates in.
    pub phases: Vec<String>,
    /// Plugin-specific configuration.
    #[serde(default)]
    pub config: serde_json::Value,
    /// Whether this plugin is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Execution priority (lower = earlier). Default 100.
    #[serde(default = "default_priority")]
    pub priority: u32,
}

/// Where the plugin code comes from.
///
/// YAML examples:
/// ```yaml
/// source:
///   wasm: ./plugins/openai-compat.wasm
/// source:
///   builtin: my-builtin-plugin
/// source:
///   url: https://cdn.example.com/plugins/scorer-v1.0.wasm
///   sha256: abc123...
/// source:
///   oci: ghcr.io/prism-plugins/k8s-router:v1.2.0
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginSourceConfig {
    /// Remote HTTP/HTTPS URL: `{ url: "https://cdn.example.com/plugin.wasm" }`
    Url {
        url: String,
        /// Optional SHA-256 checksum for integrity verification.
        #[serde(default)]
        sha256: Option<String>,
    },
    /// OCI registry image: `{ oci: "registry.example.com/plugins/name:tag" }`
    Oci {
        oci: String,
        /// Optional SHA-256 digest for pinning.
        #[serde(default)]
        digest: Option<String>,
    },
    /// WASM module file path: `{ wasm: "./path.wasm" }`
    Wasm { wasm: String },
    /// Built-in Rust plugin: `{ builtin: "plugin-name" }`
    Builtin { builtin: String },
}

// ════════════════════════════════════════════════
// Discovery configuration
// ════════════════════════════════════════════════

/// External discovery source configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    /// Discovery source name.
    pub name: String,
    /// Source type.
    #[serde(rename = "type")]
    pub source_type: DiscoverySourceType,
    /// Source-specific configuration.
    #[serde(default)]
    pub config: serde_json::Value,
    /// Whether this source is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Discovery source type.
///
/// YAML examples:
/// ```yaml
/// type:
///   wasm:
///     path: ./plugins/k8s-discovery.wasm
/// type: static
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DiscoverySourceType {
    /// WASM module: `{ wasm: { path: "./path.wasm" } }`
    Wasm { wasm: DiscoveryWasmConfig },
    /// Static inline config: `"static"`
    Static(String),
}

/// WASM discovery source configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryWasmConfig {
    pub path: String,
}

// ════════════════════════════════════════════════
// Codec configuration
// ════════════════════════════════════════════════

fn default_protocol() -> String {
    "openai".to_string()
}

/// Codec / protocol conversion configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodecConfig {
    /// Provider-specific codec chains.
    /// Key: provider_id, Value: list of plugin names to chain.
    #[serde(default)]
    pub provider_chains: HashMap<String, Vec<String>>,
    /// Protocol-pair codec chains.
    /// Key: "inbound:outbound" (e.g. "openai:anthropic"), Value: list of plugin names.
    #[serde(default)]
    pub protocol_chains: HashMap<String, Vec<String>>,
    /// Default inbound protocol (assumed when client doesn't specify).
    #[serde(default = "default_protocol")]
    pub default_inbound_protocol: String,
}

impl Default for CodecConfig {
    fn default() -> Self {
        Self {
            provider_chains: HashMap::new(),
            protocol_chains: HashMap::new(),
            default_inbound_protocol: default_protocol(),
        }
    }
}

// ── Access Log Configuration ──

/// Access log output destination.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AccessLogOutput {
    Stdout,
    File,
    Both,
}

impl Default for AccessLogOutput {
    fn default() -> Self {
        Self::Stdout
    }
}

/// Access log format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AccessLogFormat {
    Json,
    Text,
}

impl Default for AccessLogFormat {
    fn default() -> Self {
        Self::Json
    }
}

fn default_text_format() -> String {
    "[%START_TIME%] \"%METHOD% %PATH%\" %RESPONSE_CODE% %DURATION_MS%ms variant=%VARIANT% provider=%PROVIDER% model=%MODEL% strategy=%ROUTE_STRATEGY% decision=%DECISION_TIME_US%us upstream=%UPSTREAM_HOST% err=%ERROR%".into()
}

/// Access log configuration — Envoy-style `%COMMAND_OPERATOR%` format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessLogConfig {
    /// Enable access logging.
    #[serde(default)]
    pub enabled: bool,
    /// Output destination: stdout, file, or both.
    #[serde(default)]
    pub output: AccessLogOutput,
    /// File path (required when output is "file" or "both").
    #[serde(default)]
    pub file_path: Option<String>,
    /// Output format: json or text.
    #[serde(default)]
    pub format: AccessLogFormat,
    /// Text format string with `%OPERATOR%` placeholders.
    /// Only used when format=text.
    #[serde(default = "default_text_format")]
    pub text_format: String,
    /// Field selection for JSON format.
    /// Each entry is a `%OPERATOR%` string. Empty = all default fields.
    #[serde(default)]
    pub fields: Vec<String>,
}

impl Default for AccessLogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            output: AccessLogOutput::default(),
            file_path: None,
            format: AccessLogFormat::default(),
            text_format: default_text_format(),
            fields: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let yaml = r#"
server:
  http_port: 9090
providers:
  openai:
    base_url: https://api.openai.com/v1
models:
  - id: gpt-4o
    display_name: GPT-4o
    quality_tier: S
variants:
  - id: gpt-4o@openai
    model_id: gpt-4o
    provider_id: openai
pools:
  - id: default
    name: Default
    variants: [gpt-4o@openai]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.server.http_port, 9090);
        assert_eq!(config.server.grpc_port, 9090); // default
        assert_eq!(config.server.proxy_port, 8000); // default
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.models.len(), 1);
        assert_eq!(config.variants.len(), 1);
        assert_eq!(config.pools.len(), 1);
    }

    #[test]
    fn test_parse_empty_config() {
        let yaml = "{}";
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.server.http_port, 8080);
        assert_eq!(config.server.grpc_port, 9090);
        assert_eq!(config.server.proxy_port, 8000);
        assert!(config.providers.is_empty());
        assert!(config.models.is_empty());
        assert!(config.variants.is_empty());
        assert!(config.pools.is_empty());
    }

    #[test]
    fn test_parse_config_all_ports() {
        let yaml = r#"
server:
  http_port: 3000
  grpc_port: 3001
  proxy_port: 3002
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.server.http_port, 3000);
        assert_eq!(config.server.grpc_port, 3001);
        assert_eq!(config.server.proxy_port, 3002);
    }

    #[test]
    fn test_prism_config_default() {
        let config = PrismConfig::default();
        assert_eq!(config.server.http_port, 8080);
        assert_eq!(config.server.grpc_port, 9090);
        assert_eq!(config.server.proxy_port, 8000);
        assert_eq!(config.server.listen, "0.0.0.0:8080");
        assert!(config.providers.is_empty());
        assert!(config.models.is_empty());
        assert!(config.variants.is_empty());
        assert!(config.pools.is_empty());
        assert_eq!(config.routing.default_strategy, "balanced");
        assert!(config.routing.balanced.is_none());
        // New fields have sensible defaults
        assert!(config.rate_limits.is_empty());
        assert!(config.plugins.is_empty());
        assert!(config.discoveries.is_empty());
        assert!(config.codec.provider_chains.is_empty());
        assert!(config.codec.protocol_chains.is_empty());
        assert_eq!(config.codec.default_inbound_protocol, "openai");
    }

    #[test]
    fn test_parse_empty_config_new_fields_default() {
        let yaml = "{}";
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.rate_limits.is_empty());
        assert!(config.plugins.is_empty());
        assert!(config.discoveries.is_empty());
        assert_eq!(config.codec.default_inbound_protocol, "openai");
    }

    #[test]
    fn test_rate_limit_config_serde() {
        let yaml = r#"
rate_limits:
  - name: global
    key_headers: []
    window_secs: 60
    max_requests: 1000
  - name: per-model
    key_headers: [x-model]
    window_secs: 60
    max_requests: 100
    max_tokens: 50000
    overrides:
      - match_headers: { x-model: "gpt-4o" }
        max_requests: 50
        max_tokens: 25000
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.rate_limits.len(), 2);

        let global = &config.rate_limits[0];
        assert_eq!(global.name, "global");
        assert!(global.key_headers.is_empty());
        assert_eq!(global.window_secs, 60);
        assert_eq!(global.max_requests, 1000);
        assert!(global.max_tokens.is_none());
        assert!(global.overrides.is_empty());
        assert!(global.enabled); // default_true

        let per_model = &config.rate_limits[1];
        assert_eq!(per_model.name, "per-model");
        assert_eq!(per_model.key_headers, vec!["x-model".to_string()]);
        assert_eq!(per_model.max_tokens, Some(50000));
        assert_eq!(per_model.overrides.len(), 1);
        assert_eq!(per_model.overrides[0].max_requests, Some(50));
        assert_eq!(per_model.overrides[0].max_tokens, Some(25000));
        assert_eq!(
            per_model.overrides[0].match_headers.get("x-model").unwrap(),
            "gpt-4o"
        );
    }

    #[test]
    fn test_rate_limit_config_enabled_default() {
        let yaml = r#"
rate_limits:
  - name: test
    window_secs: 30
    max_requests: 10
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.rate_limits[0].enabled);
    }

    #[test]
    fn test_plugin_config_serde() {
        let yaml = r#"
plugins:
  - name: openai-compat
    source:
      wasm: ./plugins/openai-compat.wasm
    phases: [proxy]
    enabled: true
    priority: 10
  - name: my-builtin
    source:
      builtin: my-builtin-plugin
    phases: [route, analyze]
    config:
      key: value
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.plugins.len(), 2);

        let wasm_plugin = &config.plugins[0];
        assert_eq!(wasm_plugin.name, "openai-compat");
        match &wasm_plugin.source {
            PluginSourceConfig::Wasm { wasm } => {
                assert_eq!(wasm, "./plugins/openai-compat.wasm")
            }
            _ => panic!("expected Wasm source"),
        }
        assert_eq!(wasm_plugin.phases, vec!["proxy".to_string()]);
        assert!(wasm_plugin.enabled);
        assert_eq!(wasm_plugin.priority, 10);

        let builtin_plugin = &config.plugins[1];
        assert_eq!(builtin_plugin.name, "my-builtin");
        match &builtin_plugin.source {
            PluginSourceConfig::Builtin { builtin } => {
                assert_eq!(builtin, "my-builtin-plugin")
            }
            _ => panic!("expected Builtin source"),
        }
        assert_eq!(
            builtin_plugin.phases,
            vec!["route".to_string(), "analyze".to_string()]
        );
        // default values
        assert!(builtin_plugin.enabled);
        assert_eq!(builtin_plugin.priority, 100);
        assert_eq!(builtin_plugin.config["key"], "value");
    }

    #[test]
    fn test_discovery_config_serde() {
        let yaml = r#"
discoveries:
  - name: k8s-pods
    type:
      wasm:
        path: ./plugins/k8s-discovery.wasm
    config:
      namespace: default
    enabled: true
  - name: static-extra
    type: static
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.discoveries.len(), 2);

        let wasm_disc = &config.discoveries[0];
        assert_eq!(wasm_disc.name, "k8s-pods");
        match &wasm_disc.source_type {
            DiscoverySourceType::Wasm { wasm } => {
                assert_eq!(wasm.path, "./plugins/k8s-discovery.wasm")
            }
            _ => panic!("expected Wasm source type"),
        }
        assert_eq!(wasm_disc.config["namespace"], "default");
        assert!(wasm_disc.enabled);

        let static_disc = &config.discoveries[1];
        assert_eq!(static_disc.name, "static-extra");
        match &static_disc.source_type {
            DiscoverySourceType::Static(_) => {}
            _ => panic!("expected Static source type"),
        }
        assert!(static_disc.enabled); // default_true
    }

    #[test]
    fn test_codec_config_serde() {
        let yaml = r#"
codec:
  default_inbound_protocol: openai
  provider_chains:
    anthropic: [openai-to-anthropic]
  protocol_chains:
    "openai:anthropic": [openai-compat]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.codec.default_inbound_protocol, "openai");
        assert_eq!(
            config.codec.provider_chains.get("anthropic").unwrap(),
            &vec!["openai-to-anthropic".to_string()]
        );
        assert_eq!(
            config
                .codec
                .protocol_chains
                .get("openai:anthropic")
                .unwrap(),
            &vec!["openai-compat".to_string()]
        );
    }

    #[test]
    fn test_codec_config_default() {
        let codec = CodecConfig::default();
        assert!(codec.provider_chains.is_empty());
        assert!(codec.protocol_chains.is_empty());
        assert_eq!(codec.default_inbound_protocol, "openai");
    }

    #[test]
    fn test_variant_config_new_fields() {
        let yaml = r#"
variants:
  - id: deepseek-r1@local
    model_id: deepseek-r1
    provider_id: deepseek-local
    base_url: http://10.0.1.5:8080/v1
    upstream_model_name: deepseek-reasoner
    deployment_name: my-deployment
    account_id: acct-123
    pricing_override:
      input_per_1m_tokens: 0.5
      output_per_1m_tokens: 1.0
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        let v = &config.variants[0];
        assert_eq!(v.id, "deepseek-r1@local");
        assert_eq!(v.base_url.as_deref(), Some("http://10.0.1.5:8080/v1"));
        assert_eq!(
            v.upstream_model_name.as_deref(),
            Some("deepseek-reasoner")
        );
        assert_eq!(v.deployment_name.as_deref(), Some("my-deployment"));
        assert_eq!(v.account_id.as_deref(), Some("acct-123"));
        let pricing = v.pricing_override.as_ref().unwrap();
        assert!((pricing.input_per_1m_tokens - 0.5).abs() < 0.001);
        assert!((pricing.output_per_1m_tokens - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_variant_config_new_fields_default() {
        let yaml = r#"
variants:
  - id: gpt-4o@openai
    model_id: gpt-4o
    provider_id: openai
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        let v = &config.variants[0];
        assert!(v.base_url.is_none());
        assert!(v.upstream_model_name.is_none());
        assert!(v.deployment_name.is_none());
        assert!(v.account_id.is_none());
        assert!(v.pricing_override.is_none());
        assert!(v.weight.is_none());
    }

    #[test]
    fn test_full_config_parse() {
        let yaml = r#"
server:
  http_port: 8080
  grpc_port: 9090
  proxy_port: 8000

providers:
  openai:
    type: openai
    api_key: sk-test
    base_url: https://api.openai.com/v1
    protocols: [openai]
  anthropic:
    type: anthropic
    api_key: sk-ant
    base_url: https://api.anthropic.com
    protocols: [anthropic]
  deepseek-local:
    type: openai
    base_url: http://10.0.1.5:8080/v1
    protocols: [openai]

models:
  - id: gpt-4o
    display_name: GPT-4o
    quality_tier: S
    capabilities:
      max_context_window: 128000
      max_output_tokens: 16384
      supports_streaming: true
      supports_function_calling: true
      supports_vision: true
      supports_json_mode: true
      supports_reasoning: false
    pricing:
      input_per_1m_tokens: 2.5
      output_per_1m_tokens: 10.0
  - id: deepseek-r1
    display_name: DeepSeek R1
    quality_tier: A

variants:
  - id: gpt-4o@openai
    model_id: gpt-4o
    provider_id: openai
  - id: deepseek-r1@local
    model_id: deepseek-r1
    provider_id: deepseek-local
    base_url: http://10.0.1.5:8080/v1
    upstream_model_name: deepseek-reasoner
    pricing_override:
      input_per_1m_tokens: 0.5
      output_per_1m_tokens: 1.0

pools:
  - id: default
    name: Default
    variants: [gpt-4o@openai, deepseek-r1@local]

routing:
  default_strategy: balanced
  balanced:
    cost_weight: 0.3
    quality_weight: 0.5
    latency_weight: 0.2

rate_limits:
  - name: global
    key_headers: []
    window_secs: 60
    max_requests: 1000
  - name: per-model
    key_headers: [x-model]
    window_secs: 60
    max_requests: 100
    overrides:
      - match_headers: { x-model: "gpt-4o" }
        max_requests: 50

plugins:
  - name: openai-compat
    source:
      wasm: ./plugins/openai-compat.wasm
    phases: [proxy]
    enabled: true
    priority: 10

discoveries:
  - name: k8s-pods
    type:
      wasm:
        path: ./plugins/k8s-discovery.wasm
    config:
      namespace: default
    enabled: true

codec:
  default_inbound_protocol: openai
  provider_chains:
    anthropic: [openai-to-anthropic]
  protocol_chains:
    "openai:anthropic": [openai-compat]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();

        // Verify all sections parsed
        assert_eq!(config.providers.len(), 3);
        assert_eq!(config.models.len(), 2);
        assert_eq!(config.variants.len(), 2);
        assert_eq!(config.pools.len(), 1);
        assert_eq!(config.rate_limits.len(), 2);
        assert_eq!(config.plugins.len(), 1);
        assert_eq!(config.discoveries.len(), 1);

        // Verify variant new fields
        let ds_variant = &config.variants[1];
        assert_eq!(ds_variant.base_url.as_deref(), Some("http://10.0.1.5:8080/v1"));
        assert_eq!(
            ds_variant.upstream_model_name.as_deref(),
            Some("deepseek-reasoner")
        );
        assert!(ds_variant.pricing_override.is_some());

        // Verify codec
        assert_eq!(config.codec.default_inbound_protocol, "openai");
        assert_eq!(config.codec.provider_chains.len(), 1);
        assert_eq!(config.codec.protocol_chains.len(), 1);
    }

    #[test]
    fn test_plugin_source_url_serde() {
        let yaml = r#"
plugins:
  - name: remote-scorer
    source:
      url: https://cdn.example.com/plugins/scorer-v1.0.wasm
      sha256: abc123def456
    phases: [route]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.plugins.len(), 1);
        let plugin = &config.plugins[0];
        assert_eq!(plugin.name, "remote-scorer");
        match &plugin.source {
            PluginSourceConfig::Url { url, sha256 } => {
                assert_eq!(url, "https://cdn.example.com/plugins/scorer-v1.0.wasm");
                assert_eq!(sha256.as_deref(), Some("abc123def456"));
            }
            _ => panic!("expected Url source, got {:?}", plugin.source),
        }
    }

    #[test]
    fn test_plugin_source_url_no_sha256() {
        let yaml = r#"
plugins:
  - name: remote-plugin
    source:
      url: https://cdn.example.com/plugin.wasm
    phases: [proxy]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        match &config.plugins[0].source {
            PluginSourceConfig::Url { url, sha256 } => {
                assert_eq!(url, "https://cdn.example.com/plugin.wasm");
                assert!(sha256.is_none());
            }
            _ => panic!("expected Url source"),
        }
    }

    #[test]
    fn test_plugin_source_oci_serde() {
        let yaml = r#"
plugins:
  - name: k8s-router
    source:
      oci: ghcr.io/prism-plugins/k8s-router:v1.2.0
    phases: [route, proxy]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.plugins.len(), 1);
        let plugin = &config.plugins[0];
        assert_eq!(plugin.name, "k8s-router");
        match &plugin.source {
            PluginSourceConfig::Oci { oci, digest } => {
                assert_eq!(oci, "ghcr.io/prism-plugins/k8s-router:v1.2.0");
                assert!(digest.is_none());
            }
            _ => panic!("expected Oci source, got {:?}", plugin.source),
        }
    }

    #[test]
    fn test_plugin_source_oci_with_digest() {
        let yaml = r#"
plugins:
  - name: pinned-plugin
    source:
      oci: ghcr.io/prism/plugin:v1
      digest: sha256:abc123
    phases: [proxy]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        match &config.plugins[0].source {
            PluginSourceConfig::Oci { oci, digest } => {
                assert_eq!(oci, "ghcr.io/prism/plugin:v1");
                assert_eq!(digest.as_deref(), Some("sha256:abc123"));
            }
            _ => panic!("expected Oci source"),
        }
    }

    #[test]
    fn test_full_config_with_all_source_types() {
        let yaml = r#"
plugins:
  - name: local-plugin
    source:
      wasm: ./plugins/local.wasm
    phases: [proxy]
  - name: remote-plugin
    source:
      url: https://cdn.example.com/plugins/remote.wasm
      sha256: deadbeef
    phases: [route]
  - name: oci-plugin
    source:
      oci: ghcr.io/prism-plugins/oci:v1.0
    phases: [route, proxy]
  - name: builtin-plugin
    source:
      builtin: health_filter
    phases: [route]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.plugins.len(), 4);

        assert!(matches!(&config.plugins[0].source, PluginSourceConfig::Wasm { .. }));
        assert!(matches!(&config.plugins[1].source, PluginSourceConfig::Url { .. }));
        assert!(matches!(&config.plugins[2].source, PluginSourceConfig::Oci { .. }));
        assert!(matches!(&config.plugins[3].source, PluginSourceConfig::Builtin { .. }));
    }
}
