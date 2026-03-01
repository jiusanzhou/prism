//! Static discovery plugin — loads candidates from in-memory configuration.
//!
//! This is the default discovery source, equivalent to the original
//! `build_registry()` logic in config.rs.

use std::collections::HashMap;

use crate::discovery::{Discovery, DiscoveryError, DiscoveryResult};
use crate::types::*;

/// Configuration subset needed by [`StaticDiscovery`].
///
/// Mirrors the relevant parts of `PrismConfig` from `prism-router` so that
/// `prism-core` stays dependency-free from the router crate.
#[derive(Debug, Clone, Default)]
pub struct StaticDiscoveryConfig {
    pub providers: HashMap<String, StaticProviderConfig>,
    pub models: Vec<StaticModelConfig>,
    pub variants: Vec<StaticVariantConfig>,
    pub pools: Vec<StaticPoolConfig>,
}

/// Provider configuration for static discovery.
#[derive(Debug, Clone)]
pub struct StaticProviderConfig {
    pub provider_type: Option<String>,
    pub api_key: Option<String>,
    pub base_url: String,
    pub protocols: Vec<String>,
    /// Custom headers to send with every request to this provider.
    pub headers: HashMap<String, String>,
}

/// Model configuration for static discovery.
#[derive(Debug, Clone)]
pub struct StaticModelConfig {
    pub id: String,
    pub display_name: String,
    pub quality_tier: QualityTier,
    pub capabilities: ModelCapabilities,
    pub pricing: ModelPricing,
    pub specialties: Vec<TaskType>,
}

/// Variant configuration for static discovery.
#[derive(Debug, Clone)]
pub struct StaticVariantConfig {
    pub id: String,
    pub model_id: String,
    pub provider_id: String,
    pub weight: Option<f32>,
    /// Account id for multi-account routing.
    pub account_id: Option<String>,
    /// Base URL override for self-hosted models.
    pub base_url: Option<String>,
    /// Deployment name (for Azure/GCP etc).
    pub deployment_name: Option<String>,
    /// Upstream model name (vendor-specific).
    pub upstream_model_name: Option<String>,
    /// Pricing override.
    pub pricing_override: Option<ModelPricing>,
}

/// Pool configuration for static discovery.
#[derive(Debug, Clone)]
pub struct StaticPoolConfig {
    pub id: String,
    pub name: String,
    pub variants: Vec<String>,
    pub selection_mode: Option<SelectionMode>,
}

// ════════════════════════════════════════════════
// StaticDiscovery
// ════════════════════════════════════════════════

/// Loads scheduling objects from a static configuration struct.
///
/// For each provider, a default endpoint (`{id}-ep`) and account (`{id}-acct`)
/// are created automatically — matching the original `build_registry()` behavior.
pub struct StaticDiscovery {
    config: StaticDiscoveryConfig,
}

impl StaticDiscovery {
    /// Create a new `StaticDiscovery` from configuration.
    pub fn new(config: StaticDiscoveryConfig) -> Self {
        Self { config }
    }

    /// Build the discovery result from the stored config.
    fn build_result(&self) -> DiscoveryResult {
        let mut result = DiscoveryResult::empty();

        // Providers → also generate default endpoint + account
        for (id, pc) in &self.config.providers {
            let auth = if !pc.headers.is_empty() {
                // Custom headers take precedence (e.g. x-api-key)
                AuthConfig::Custom {
                    headers: pc.headers.clone(),
                }
            } else {
                match &pc.api_key {
                    Some(key) => AuthConfig::ApiKey {
                        key: format!("Bearer {}", key),
                        header: "Authorization".to_string(),
                    },
                    None => AuthConfig::default(),
                }
            };

            result.providers.push(Provider {
                id: id.clone(),
                name: pc.provider_type.clone().unwrap_or_else(|| id.clone()),
                auth: auth.clone(),
                base_url: pc.base_url.clone(),
                rate_limit: None,
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
                protocols: pc.protocols.clone(),
            });

            let endpoint_id = format!("{}-ep", id);
            result.endpoints.push(Endpoint {
                id: endpoint_id.clone(),
                provider_id: id.clone(),
                base_url: Some(pc.base_url.clone()),
                region: None,
                auth_override: None,
                rate_limit: None,
                priority: 0,
                status: HealthStatus::Healthy,
            });

            let account_id = format!("{}-acct", id);
            result.accounts.push(Account {
                id: account_id,
                endpoint_id,
                auth,
                project_id: None,
                rate_limit: None,
                billing_tag: None,
                weight: 1.0,
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
            });
        }

        // Models
        for mc in &self.config.models {
            result.models.push(Model {
                id: mc.id.clone(),
                display_name: mc.display_name.clone(),
                quality_tier: mc.quality_tier,
                capabilities: mc.capabilities.clone(),
                pricing: mc.pricing.clone(),
                specialties: mc.specialties.clone(),
                limits: None,
                status: ModelStatus::Available,
            });
        }

        // Variants
        for vc in &self.config.variants {
            let endpoint_id = format!("{}-ep", vc.provider_id);
            let default_account_id = format!("{}-acct", vc.provider_id);

            result.variants.push(ModelVariant {
                id: vc.id.clone(),
                model_id: vc.model_id.clone(),
                endpoint_id,
                account_id: Some(vc.account_id.clone().unwrap_or(default_account_id)),
                deployment_name: vc.deployment_name.clone(),
                weight: vc.weight.unwrap_or(1.0),
                performance: ModelPerformance::default(),
                pricing_override: vc.pricing_override.clone(),
                status: VariantStatus::Active,
                base_url: vc.base_url.clone(),
                upstream_model_name: vc.upstream_model_name.clone(),
            });
        }

        // Pools
        for pc in &self.config.pools {
            result.pools.push(ModelPool {
                id: pc.id.clone(),
                name: pc.name.clone(),
                variants: pc.variants.clone(),
                selection_mode: pc.selection_mode.clone().unwrap_or_default(),
                description: None,
            });
        }

        result
    }
}

impl Discovery for StaticDiscovery {
    fn name(&self) -> &str {
        "static"
    }

    fn discover(&self) -> Result<DiscoveryResult, DiscoveryError> {
        Ok(self.build_result())
    }
}

// ════════════════════════════════════════════════
// Tests
// ════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> StaticDiscoveryConfig {
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            StaticProviderConfig {
                provider_type: Some("openai".to_string()),
                api_key: Some("sk-test".to_string()),
                base_url: "https://api.openai.com/v1".to_string(),
                protocols: vec!["openai".to_string()],
                headers: HashMap::new(),
            },
        );

        StaticDiscoveryConfig {
            providers,
            models: vec![StaticModelConfig {
                id: "gpt-4o".to_string(),
                display_name: "GPT-4o".to_string(),
                quality_tier: QualityTier::S,
                capabilities: ModelCapabilities {
                    max_context_window: 128000,
                    max_output_tokens: 16384,
                    supports_streaming: true,
                    supports_function_calling: true,
                    supports_vision: true,
                    supports_json_mode: true,
                    supports_reasoning: false,
                },
                pricing: ModelPricing {
                    input_per_1m_tokens: 2.5,
                    output_per_1m_tokens: 10.0,
                    currency: "USD".to_string(),
                },
                specialties: vec![],
            }],
            variants: vec![StaticVariantConfig {
                id: "gpt-4o@openai".to_string(),
                model_id: "gpt-4o".to_string(),
                provider_id: "openai".to_string(),
                weight: None,
                account_id: None,
                base_url: None,
                deployment_name: None,
                upstream_model_name: None,
                pricing_override: None,
            }],
            pools: vec![StaticPoolConfig {
                id: "default".to_string(),
                name: "Default Pool".to_string(),
                variants: vec!["gpt-4o@openai".to_string()],
                selection_mode: None,
            }],
        }
    }

    #[test]
    fn test_static_discovery_name() {
        let d = StaticDiscovery::new(StaticDiscoveryConfig::default());
        assert_eq!(d.name(), "static");
    }

    #[test]
    fn test_static_discovery_empty_config() {
        let d = StaticDiscovery::new(StaticDiscoveryConfig::default());
        let result = d.discover().unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_static_discovery_full_config() {
        let d = StaticDiscovery::new(sample_config());
        let result = d.discover().unwrap();

        assert_eq!(result.providers.len(), 1);
        assert_eq!(result.endpoints.len(), 1);
        assert_eq!(result.accounts.len(), 1);
        assert_eq!(result.models.len(), 1);
        assert_eq!(result.variants.len(), 1);
        assert_eq!(result.pools.len(), 1);

        // Check provider details
        assert_eq!(result.providers[0].id, "openai");
        assert_eq!(result.providers[0].protocols, vec!["openai".to_string()]);
        match &result.providers[0].auth {
            AuthConfig::ApiKey { key, .. } => assert_eq!(key, "Bearer sk-test"),
            _ => panic!("expected ApiKey auth"),
        }

        // Check default endpoint/account were created
        assert_eq!(result.endpoints[0].id, "openai-ep");
        assert_eq!(result.endpoints[0].provider_id, "openai");
        assert_eq!(result.accounts[0].id, "openai-acct");
        assert_eq!(result.accounts[0].endpoint_id, "openai-ep");

        // Check variant references
        assert_eq!(result.variants[0].endpoint_id, "openai-ep");
        assert_eq!(
            result.variants[0].account_id,
            Some("openai-acct".to_string())
        );
    }

    #[test]
    fn test_static_discovery_no_api_key() {
        let mut config = StaticDiscoveryConfig::default();
        config.providers.insert(
            "local".to_string(),
            StaticProviderConfig {
                provider_type: None,
                api_key: None,
                base_url: "http://localhost:8000".to_string(),
                protocols: vec![],
                headers: HashMap::new(),
            },
        );

        let d = StaticDiscovery::new(config);
        let result = d.discover().unwrap();

        let provider = &result.providers[0];
        match &provider.auth {
            AuthConfig::ApiKey { key, .. } => assert!(key.is_empty()),
            _ => panic!("expected default ApiKey auth"),
        }
    }

    #[test]
    fn test_static_discovery_variant_weight() {
        let mut config = StaticDiscoveryConfig::default();
        config.providers.insert(
            "p1".to_string(),
            StaticProviderConfig {
                provider_type: None,
                api_key: None,
                base_url: "http://localhost".to_string(),
                protocols: vec![],
                headers: HashMap::new(),
            },
        );
        config.models.push(StaticModelConfig {
            id: "m1".to_string(),
            display_name: "M1".to_string(),
            quality_tier: QualityTier::A,
            capabilities: ModelCapabilities::default(),
            pricing: ModelPricing::default(),
            specialties: vec![],
        });
        config.variants.push(StaticVariantConfig {
            id: "m1@p1".to_string(),
            model_id: "m1".to_string(),
            provider_id: "p1".to_string(),
            weight: Some(0.7),
            account_id: None,
            base_url: None,
            deployment_name: None,
            upstream_model_name: None,
            pricing_override: None,
        });

        let d = StaticDiscovery::new(config);
        let result = d.discover().unwrap();
        assert!((result.variants[0].weight - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_static_discovery_does_not_support_watch() {
        let d = StaticDiscovery::new(StaticDiscoveryConfig::default());
        assert!(!d.supports_watch());
        let err = d.watch(Box::new(|_| {})).unwrap_err();
        match err {
            DiscoveryError::NotSupported(_) => {}
            _ => panic!("expected NotSupported"),
        }
    }
}
