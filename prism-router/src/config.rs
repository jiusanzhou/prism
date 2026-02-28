use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

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
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            http_port: default_http_port(),
        }
    }
}

fn default_listen() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_http_port() -> u16 {
    8080
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
    let mut registry = InMemoryRegistry::new();

    // Register providers — each provider also gets a default endpoint and account
    for (id, pc) in &config.providers {
        let auth = match &pc.api_key {
            Some(key) => AuthConfig::ApiKey {
                key: format!("Bearer {}", key),
                header: "Authorization".to_string(),
            },
            None => AuthConfig::default(),
        };

        let provider = Provider {
            id: id.clone(),
            name: pc.provider_type.clone().unwrap_or_else(|| id.clone()),
            auth: auth.clone(),
            base_url: pc.base_url.clone(),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: pc.protocols.clone(),
        };
        registry.add_provider(provider);

        // Create a default endpoint for this provider
        let endpoint_id = format!("{}-ep", id);
        let endpoint = Endpoint {
            id: endpoint_id.clone(),
            provider_id: id.clone(),
            base_url: Some(pc.base_url.clone()),
            region: None,
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        };
        registry.add_endpoint(endpoint);

        // Create a default account for this endpoint
        let account_id = format!("{}-acct", id);
        let account = Account {
            id: account_id,
            endpoint_id,
            auth,
            project_id: None,
            rate_limit: None,
            billing_tag: None,
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
        };
        registry.add_account(account);
    }

    // Register models
    for mc in &config.models {
        let model = Model {
            id: mc.id.clone(),
            display_name: mc.display_name.clone(),
            quality_tier: mc.quality_tier,
            capabilities: mc.capabilities.clone(),
            pricing: mc.pricing.clone(),
            specialties: mc.specialties.clone(),
            limits: None,
            status: ModelStatus::Available,
        };
        registry.add_model(model);
    }

    // Register variants
    for vc in &config.variants {
        let endpoint_id = format!("{}-ep", vc.provider_id);
        let account_id = format!("{}-acct", vc.provider_id);

        let variant = ModelVariant {
            id: vc.id.clone(),
            model_id: vc.model_id.clone(),
            endpoint_id,
            account_id: Some(account_id),
            deployment_name: None,
            weight: vc.weight.unwrap_or(1.0),
            performance: ModelPerformance::default(),
            pricing_override: None,
            status: VariantStatus::Active,
        };
        registry.add_variant(variant);
    }

    // Register pools
    for pc in &config.pools {
        let pool = ModelPool {
            id: pc.id.clone(),
            name: pc.name.clone(),
            variants: pc.variants.clone(),
            selection_mode: pc.selection_mode.clone().unwrap_or_default(),
            description: None,
        };
        registry.add_pool(pool);
    }

    registry
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
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.models.len(), 1);
        assert_eq!(config.variants.len(), 1);
        assert_eq!(config.pools.len(), 1);
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
}
