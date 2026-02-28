use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use prism_core::discovery::static_discovery::{
    StaticDiscovery, StaticDiscoveryConfig, StaticModelConfig, StaticPoolConfig,
    StaticProviderConfig, StaticVariantConfig,
};
use prism_core::discovery::Discovery;
use prism_core::registry::{InMemoryRegistry, ModelRegistryTrait};
use prism_core::types::*;

/// Top-level configuration file structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingConfig {
    #[serde(default = "default_strategy")]
    pub default_strategy: String,
    #[serde(default)]
    pub balanced: Option<BalancedConfig>,
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

/// Load and parse a YAML configuration file.
pub fn load_config(path: &Path) -> Result<PrismConfig, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read config file {}: {}", path.display(), e))?;

    // Expand environment variables in the form ${VAR_NAME}
    let expanded = expand_env_vars(&content);

    serde_yaml::from_str(&expanded)
        .map_err(|e| format!("failed to parse config file {}: {}", path.display(), e))
}

/// Expand `${VAR_NAME}` patterns in a string using environment variables.
fn expand_env_vars(input: &str) -> String {
    let mut result = input.to_string();
    // Simple regex-free approach: scan for ${...}
    loop {
        if let Some(start) = result.find("${") {
            if let Some(end) = result[start..].find('}') {
                let var_name = &result[start + 2..start + end];
                let value = std::env::var(var_name).unwrap_or_default();
                result = format!("{}{}{}", &result[..start], value, &result[start + end + 1..]);
                continue;
            }
        }
        break;
    }
    result
}

/// Build an `InMemoryRegistry` from a parsed configuration.
pub fn build_registry(config: &PrismConfig) -> InMemoryRegistry {
    let static_config = to_static_discovery_config(config);
    let discovery = StaticDiscovery::new(static_config);
    let result = discovery
        .discover()
        .expect("static discovery should never fail");

    let mut registry = InMemoryRegistry::new();
    registry.apply_discovery(&result);
    registry
}

/// Build an `InMemoryRegistry` from config plus additional discovery sources.
///
/// The static config is always loaded first; then each additional discovery
/// source is applied in order (later sources can overwrite earlier ones).
pub fn build_registry_with_discoveries(
    config: &PrismConfig,
    extra_discoveries: &[Box<dyn Discovery>],
) -> InMemoryRegistry {
    let mut registry = build_registry(config);

    for discovery in extra_discoveries {
        if let Ok(result) = discovery.discover() {
            registry.apply_discovery(&result);
        }
    }

    registry
}

/// Convert a `PrismConfig` into a `StaticDiscoveryConfig`.
fn to_static_discovery_config(config: &PrismConfig) -> StaticDiscoveryConfig {
    let providers = config
        .providers
        .iter()
        .map(|(id, pc)| {
            (
                id.clone(),
                StaticProviderConfig {
                    provider_type: pc.provider_type.clone(),
                    api_key: pc.api_key.clone(),
                    base_url: pc.base_url.clone(),
                    protocols: pc.protocols.clone(),
                },
            )
        })
        .collect();

    let models = config
        .models
        .iter()
        .map(|mc| StaticModelConfig {
            id: mc.id.clone(),
            display_name: mc.display_name.clone(),
            quality_tier: mc.quality_tier,
            capabilities: mc.capabilities.clone(),
            pricing: mc.pricing.clone(),
            specialties: mc.specialties.clone(),
        })
        .collect();

    let variants = config
        .variants
        .iter()
        .map(|vc| StaticVariantConfig {
            id: vc.id.clone(),
            model_id: vc.model_id.clone(),
            provider_id: vc.provider_id.clone(),
            weight: vc.weight,
        })
        .collect();

    let pools = config
        .pools
        .iter()
        .map(|pc| StaticPoolConfig {
            id: pc.id.clone(),
            name: pc.name.clone(),
            variants: pc.variants.clone(),
            selection_mode: pc.selection_mode.clone(),
        })
        .collect();

    StaticDiscoveryConfig {
        providers,
        models,
        variants,
        pools,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_env_vars() {
        std::env::set_var("TEST_PRISM_VAR", "hello");
        let input = "key: ${TEST_PRISM_VAR}";
        let result = expand_env_vars(input);
        assert_eq!(result, "key: hello");
        std::env::remove_var("TEST_PRISM_VAR");
    }

    #[test]
    fn test_expand_env_vars_missing() {
        let input = "key: ${NONEXISTENT_PRISM_VAR_12345}";
        let result = expand_env_vars(input);
        assert_eq!(result, "key: ");
    }

    #[test]
    fn test_expand_env_vars_multiple() {
        std::env::set_var("TEST_A", "aaa");
        std::env::set_var("TEST_B", "bbb");
        let input = "a: ${TEST_A}, b: ${TEST_B}";
        let result = expand_env_vars(input);
        assert_eq!(result, "a: aaa, b: bbb");
        std::env::remove_var("TEST_A");
        std::env::remove_var("TEST_B");
    }

    #[test]
    fn test_expand_env_vars_no_vars() {
        let input = "no variables here";
        let result = expand_env_vars(input);
        assert_eq!(result, "no variables here");
    }

    #[test]
    fn test_expand_env_vars_unclosed_brace() {
        let input = "key: ${UNCLOSED";
        let result = expand_env_vars(input);
        assert_eq!(result, "key: ${UNCLOSED");
    }

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
    fn test_build_registry_from_config() {
        let yaml = r#"
providers:
  openai:
    base_url: https://api.openai.com/v1
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
variants:
  - id: gpt-4o@openai
    model_id: gpt-4o
    provider_id: openai
pools:
  - id: default
    name: Default Pool
    variants: [gpt-4o@openai]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = build_registry(&config);

        // Check that candidates can be assembled
        let candidates = registry.get_all_candidates();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].variant.id, "gpt-4o@openai");
        assert_eq!(candidates[0].model.id, "gpt-4o");
        assert_eq!(candidates[0].provider.id, "openai");
    }

    #[test]
    fn test_build_registry_multi_provider() {
        let yaml = r#"
providers:
  openai:
    base_url: https://api.openai.com/v1
    api_key: sk-openai
    protocols: [openai]
  anthropic:
    base_url: https://api.anthropic.com
    api_key: sk-ant
    protocols: [anthropic]
models:
  - id: gpt-4o
    display_name: GPT-4o
    quality_tier: S
  - id: claude-3
    display_name: Claude 3
    quality_tier: S
variants:
  - id: gpt-4o@openai
    model_id: gpt-4o
    provider_id: openai
  - id: claude-3@anthropic
    model_id: claude-3
    provider_id: anthropic
    weight: 0.5
pools:
  - id: default
    name: Default Pool
    variants: [gpt-4o@openai, claude-3@anthropic]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = build_registry(&config);

        let candidates = registry.get_all_candidates();
        assert_eq!(candidates.len(), 2);

        // Check providers have correct protocols
        let openai = registry.providers.get("openai").unwrap();
        assert_eq!(openai.protocols, vec!["openai".to_string()]);

        let anthropic = registry.providers.get("anthropic").unwrap();
        assert_eq!(anthropic.protocols, vec!["anthropic".to_string()]);

        // Check api_key was set
        match &openai.auth {
            AuthConfig::ApiKey { key, .. } => assert_eq!(key, "Bearer sk-openai"),
            _ => panic!("expected ApiKey auth"),
        }
    }

    #[test]
    fn test_build_registry_no_api_key() {
        let yaml = r#"
providers:
  local:
    base_url: http://localhost:8000
models:
  - id: llama
    display_name: Llama
    quality_tier: B
variants:
  - id: llama@local
    model_id: llama
    provider_id: local
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = build_registry(&config);

        let provider = registry.providers.get("local").unwrap();
        match &provider.auth {
            AuthConfig::ApiKey { key, .. } => assert!(key.is_empty()),
            _ => panic!("expected default ApiKey auth"),
        }
    }

    #[test]
    fn test_build_registry_variant_weight() {
        let yaml = r#"
providers:
  p1:
    base_url: http://localhost
models:
  - id: m1
    display_name: M1
    quality_tier: A
variants:
  - id: m1@p1
    model_id: m1
    provider_id: p1
    weight: 0.7
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = build_registry(&config);

        let variant = registry.variants.get("m1@p1").unwrap();
        assert!((variant.weight - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_build_registry_default_variant_weight() {
        let yaml = r#"
providers:
  p1:
    base_url: http://localhost
models:
  - id: m1
    display_name: M1
    quality_tier: A
variants:
  - id: m1@p1
    model_id: m1
    provider_id: p1
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = build_registry(&config);

        let variant = registry.variants.get("m1@p1").unwrap();
        assert!((variant.weight - 1.0).abs() < 0.001);
    }
}
