use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use prism_core::registry::InMemoryRegistry;
use prism_core::types::*;

/// Top-level configuration for prism-proxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub server: ProxyServerConfig,
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

/// Server configuration for the proxy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyServerConfig {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    #[serde(default = "default_upstream_timeout")]
    pub upstream_timeout_ms: u64,
}

impl Default for ProxyServerConfig {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            http_port: default_http_port(),
            upstream_timeout_ms: default_upstream_timeout(),
        }
    }
}

fn default_listen() -> String {
    "0.0.0.0:8080".to_string()
}

fn default_http_port() -> u16 {
    8080
}

fn default_upstream_timeout() -> u64 {
    30000
}

/// Provider configuration.
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
}

/// Routing configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingConfig {
    #[serde(default = "default_strategy")]
    pub default_strategy: String,
}

fn default_strategy() -> String {
    "balanced".to_string()
}

/// Load and parse a YAML configuration file.
pub fn load_config(path: &Path) -> Result<ProxyConfig, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read config file {}: {}", path.display(), e))?;

    // Expand environment variables
    let expanded = expand_env_vars(&content);

    serde_yaml::from_str(&expanded)
        .map_err(|e| format!("failed to parse config file {}: {}", path.display(), e))
}

/// Expand `${VAR_NAME}` patterns using environment variables.
fn expand_env_vars(input: &str) -> String {
    let mut result = input.to_string();
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

/// Build an `InMemoryRegistry` from a parsed proxy configuration.
pub fn build_registry(config: &ProxyConfig) -> InMemoryRegistry {
    let mut registry = InMemoryRegistry::new();

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

    for pc in &config.pools {
        let pool = ModelPool {
            id: pc.id.clone(),
            name: pc.name.clone(),
            variants: pc.variants.clone(),
            selection_mode: SelectionMode::default(),
            description: None,
        };
        registry.add_pool(pool);
    }

    registry
}
