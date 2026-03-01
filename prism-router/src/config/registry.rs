use prism_core::discovery::static_discovery::{
    StaticDiscovery, StaticDiscoveryConfig, StaticModelConfig, StaticPoolConfig,
    StaticProviderConfig, StaticVariantConfig,
};
use prism_core::discovery::Discovery;
use prism_core::registry::{InMemoryRegistry, ModelRegistryTrait};

use super::PrismConfig;

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
    use prism_core::types::AuthConfig;

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
