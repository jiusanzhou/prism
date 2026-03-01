use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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

impl Default for PrismConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            providers: HashMap::new(),
            models: Vec::new(),
            variants: Vec::new(),
            pools: Vec::new(),
            routing: RoutingConfig::default(),
        }
    }
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
    }
}
