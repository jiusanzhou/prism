use prism_core::discovery::static_discovery::{
    StaticDiscovery, StaticDiscoveryConfig, StaticModelConfig, StaticPoolConfig,
    StaticProviderConfig, StaticVariantConfig,
};
use prism_core::discovery::Discovery;
use prism_core::registry::InMemoryRegistry;
#[allow(unused_imports)]
use prism_core::registry::ModelRegistryTrait;
use prism_core::router::PrismRouter;
use prism_core::selector::{create_l2_selector, create_l3_selector, create_l4_selector};
use prism_core::strategy::{
    BackoffStrategy, CircuitBreakerPolicy, ModelSelectionStrategy, ResiliencePolicy, RetryCondition,
    RetryPolicy, RetryScope, RoutingObjective, TimeoutPolicy,
};

use super::PrismConfig;
use super::types::RoutingConfig;

/// Build an `InMemoryRegistry` from a parsed configuration.
pub fn build_registry(config: &PrismConfig) -> InMemoryRegistry {
    let static_config = to_static_discovery_config(config);
    let discovery = StaticDiscovery::new(static_config);
    let result = discovery
        .discover()
        .expect("static discovery should never fail");

    let mut registry = InMemoryRegistry::new();
    registry.apply_discovery(&result);

    // Inject accounts from config (AccountConfig) into registry.
    // These override the default "{provider_id}-acct" accounts created by
    // static discovery, carrying the real API keys and per-account settings.
    for ac in &config.accounts {
        if !ac.enabled {
            continue;
        }
        let endpoint_id = format!("{}-ep", ac.provider_id);
        // Resolve auth_type from the provider config
        let provider_auth_type = config
            .providers
            .get(&ac.provider_id)
            .map(|p| p.auth_type.clone())
            .unwrap_or_default();

        let auth = if let Some(ref key) = ac.api_key {
            match &provider_auth_type {
                crate::config::types::ProviderAuthType::Bearer => {
                    prism_core::types::AuthConfig::ApiKey {
                        key: format!("Bearer {}", key),
                        header: "Authorization".to_string(),
                    }
                }
                crate::config::types::ProviderAuthType::Header => {
                    prism_core::types::AuthConfig::ApiKey {
                        key: key.clone(),
                        header: "Authorization".to_string(),
                    }
                }
                crate::config::types::ProviderAuthType::XApiKey => {
                    prism_core::types::AuthConfig::ApiKey {
                        key: key.clone(),
                        header: "x-api-key".to_string(),
                    }
                }
                crate::config::types::ProviderAuthType::ApiKey => {
                    prism_core::types::AuthConfig::ApiKey {
                        key: key.clone(),
                        header: "api-key".to_string(),
                    }
                }
                crate::config::types::ProviderAuthType::Custom(header_name) => {
                    prism_core::types::AuthConfig::ApiKey {
                        key: key.clone(),
                        header: header_name.clone(),
                    }
                }
                crate::config::types::ProviderAuthType::AwsSigV4 => {
                    // Bedrock uses AWS SigV4 signing, not simple API key headers.
                    // AWS credentials are stored in provider config (aws_access_key, etc).
                    // For now, skip setting auth — proxy layer will handle SigV4 separately.
                    prism_core::types::AuthConfig::default()
                }
            }
        } else if !ac.headers.is_empty() {
            prism_core::types::AuthConfig::Custom {
                headers: ac.headers.clone(),
            }
        } else {
            prism_core::types::AuthConfig::default()
        };
        let account = prism_core::types::Account {
            id: ac.id.clone(),
            endpoint_id,
            auth,
            project_id: ac.project_id.clone(),
            rate_limit: None,
            billing_tag: ac.billing_tag.clone(),
            weight: ac.weight,
            status: prism_core::types::HealthStatus::Healthy,
            metadata: ac.metadata.clone(),
        };
        registry.accounts.insert(ac.id.clone(), account);
    }

    // Update variant account_ids: if a variant references a config account
    // by provider_id (using the default "{provider_id}-acct" placeholder),
    // rebind it to the first matching config account for that provider.
    for variant in registry.variants.values_mut() {
        let default_acct_id = format!("{}-acct", variant.endpoint_id.trim_end_matches("-ep"));
        if variant.account_id.as_deref() == Some(&default_acct_id) {
            // Find the first enabled config account for this provider
            if let Some(ac) = config.accounts.iter().find(|a| {
                a.enabled && format!("{}-ep", a.provider_id) == variant.endpoint_id
            }) {
                variant.account_id = Some(ac.id.clone());
            }
        }
    }

    registry
}

/// Build an `InMemoryRegistry` from config plus additional discovery sources.
///
/// The static config is always loaded first; then each additional discovery
/// source is applied in order (later sources can overwrite earlier ones).
#[allow(dead_code)]
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

/// Convert a strategy name string + RoutingConfig into a `ModelSelectionStrategy`.
pub fn parse_strategy_from_config(
    name: &str,
    routing: &RoutingConfig,
) -> ModelSelectionStrategy {
    match name {
        "cost_first" => ModelSelectionStrategy::Auto {
            objective: RoutingObjective::CostFirst {
                min_quality_score: 0.0,
                budget_per_request: None,
            },
            pool_ids: None,
            required_capabilities: None,
        },
        "quality_first" => ModelSelectionStrategy::Auto {
            objective: RoutingObjective::QualityFirst {
                max_latency_ms: None,
            },
            pool_ids: None,
            required_capabilities: None,
        },
        "latency_first" => ModelSelectionStrategy::Auto {
            objective: RoutingObjective::SloFirst {
                max_latency_ms: 3000,
                max_ttft_ms: 1000,
                min_quality_score: 0.0,
            },
            pool_ids: None,
            required_capabilities: None,
        },
        _ => {
            // "balanced" or unknown
            let balanced = routing.balanced.as_ref();
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::Balanced {
                    cost_weight: balanced.map_or(0.3, |b| b.cost_weight),
                    quality_weight: balanced.map_or(0.5, |b| b.quality_weight),
                    latency_weight: balanced.map_or(0.2, |b| b.latency_weight),
                },
                pool_ids: None,
                required_capabilities: None,
            }
        }
    }
}

/// Convert a `ResilienceConfig` (from YAML) into a core `ResiliencePolicy`.
fn resilience_policy_from_config(
    cfg: &super::types::ResilienceConfig,
) -> ResiliencePolicy {
    let retry_on: Vec<RetryCondition> = cfg
        .retry
        .retry_on
        .iter()
        .filter_map(|s| match s.as_str() {
            "RateLimit" => Some(RetryCondition::RateLimit),
            "Timeout" => Some(RetryCondition::Timeout),
            "ConnectionError" => Some(RetryCondition::ConnectionError),
            "ModelOverloaded" => Some(RetryCondition::ModelOverloaded),
            _ => None,
        })
        .collect();

    let backoff = match cfg.retry.backoff.as_str() {
        "Fixed" => BackoffStrategy::Fixed { delay_ms: 200 },
        "Jittered" => BackoffStrategy::Jittered {
            base_ms: 100,
            max_ms: 5000,
        },
        _ => BackoffStrategy::Exponential {
            base_ms: 100,
            max_ms: 5000,
        },
    };

    let retry_scope = match cfg.retry.retry_scope.as_str() {
        "SameVariant" => RetryScope::SameVariant,
        "SameAccount" => RetryScope::SameAccount,
        "SameProvider" => RetryScope::SameProvider,
        _ => RetryScope::CrossProvider,
    };

    ResiliencePolicy {
        retry: RetryPolicy {
            max_retries: cfg.retry.max_retries,
            retry_on,
            backoff,
            retry_scope,
        },
        circuit_breaker: CircuitBreakerPolicy {
            error_threshold_pct: cfg.circuit_breaker.error_threshold_pct,
            window_seconds: cfg.circuit_breaker.window_seconds,
            open_duration_seconds: cfg.circuit_breaker.open_duration_seconds,
            ..CircuitBreakerPolicy::default()
        },
        timeout: TimeoutPolicy {
            connect_ms: cfg.timeout.connect_ms,
            request_ms: cfg.timeout.request_ms,
            ttft_ms: Some(cfg.timeout.ttft_ms),
        },
        ..ResiliencePolicy::default()
    }
}

/// Build a fully-configured `PrismRouter` from a registry and routing config.
///
/// Applies: default strategy, balanced weights, L2/L3/L4 selectors,
/// per-model strategy overrides, and resilience policy.
pub fn build_router_from_config(
    registry: InMemoryRegistry,
    routing: &RoutingConfig,
) -> PrismRouter {
    let strategy = parse_strategy_from_config(&routing.default_strategy, routing);

    let model_overrides: Vec<(String, ModelSelectionStrategy)> = routing
        .model_overrides
        .iter()
        .map(|o| {
            (
                o.model_id.clone(),
                parse_strategy_from_config(&o.strategy, routing),
            )
        })
        .collect();

    let resilience = resilience_policy_from_config(&routing.resilience);

    PrismRouter::new(registry)
        .with_strategy(strategy)
        .with_l2(create_l2_selector(&routing.l2_selector))
        .with_l3(create_l3_selector(&routing.l3_selector))
        .with_l4(create_l4_selector(&routing.l4_selector))
        .with_resilience_policy(resilience)
        .with_model_overrides(model_overrides)
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
                    headers: pc.headers.clone(),
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
            account_id: vc.account_id.clone(),
            base_url: vc.base_url.clone(),
            deployment_name: vc.deployment_name.clone(),
            upstream_model_name: vc.upstream_model_name.clone(),
            pricing_override: vc.pricing_override.clone(),
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

    #[test]
    fn test_parse_strategy_balanced_default() {
        let routing = RoutingConfig::default();
        let strategy = parse_strategy_from_config("balanced", &routing);
        match strategy {
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::Balanced {
                    cost_weight,
                    quality_weight,
                    latency_weight,
                },
                ..
            } => {
                assert!((cost_weight - 0.3).abs() < 0.01);
                assert!((quality_weight - 0.5).abs() < 0.01);
                assert!((latency_weight - 0.2).abs() < 0.01);
            }
            other => panic!("expected Balanced strategy, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_strategy_balanced_custom_weights() {
        let routing = RoutingConfig {
            balanced: Some(super::super::types::BalancedConfig {
                cost_weight: 0.6,
                quality_weight: 0.3,
                latency_weight: 0.1,
            }),
            ..Default::default()
        };
        let strategy = parse_strategy_from_config("balanced", &routing);
        match strategy {
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::Balanced {
                    cost_weight,
                    quality_weight,
                    latency_weight,
                },
                ..
            } => {
                assert!((cost_weight - 0.6).abs() < 0.01);
                assert!((quality_weight - 0.3).abs() < 0.01);
                assert!((latency_weight - 0.1).abs() < 0.01);
            }
            other => panic!("expected Balanced strategy, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_strategy_cost_first() {
        let routing = RoutingConfig::default();
        let strategy = parse_strategy_from_config("cost_first", &routing);
        match strategy {
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::CostFirst { .. },
                ..
            } => {}
            other => panic!("expected CostFirst strategy, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_strategy_quality_first() {
        let routing = RoutingConfig::default();
        let strategy = parse_strategy_from_config("quality_first", &routing);
        match strategy {
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::QualityFirst { .. },
                ..
            } => {}
            other => panic!("expected QualityFirst strategy, got: {other:?}"),
        }
    }

    #[test]
    fn test_parse_strategy_latency_first() {
        let routing = RoutingConfig::default();
        let strategy = parse_strategy_from_config("latency_first", &routing);
        match strategy {
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::SloFirst { .. },
                ..
            } => {}
            other => panic!("expected SloFirst strategy, got: {other:?}"),
        }
    }

    #[test]
    fn test_build_router_from_config_default() {
        let yaml = r#"
providers:
  openai:
    base_url: https://api.openai.com/v1
    protocols: [openai]
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
    name: Default Pool
    variants: [gpt-4o@openai]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = build_registry(&config);
        let routing = RoutingConfig::default();

        let router = build_router_from_config(registry, &routing);

        // Should use balanced strategy by default
        match &router.default_strategy {
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::Balanced { .. },
                ..
            } => {}
            other => panic!("expected Balanced default strategy, got: {other:?}"),
        }

        // Should have no model overrides
        assert!(router.model_overrides.is_empty());
    }

    #[test]
    fn test_build_router_from_config_cost_first() {
        let yaml = r#"
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
        let registry = build_registry(&config);
        let routing = RoutingConfig {
            default_strategy: "cost_first".into(),
            ..Default::default()
        };

        let router = build_router_from_config(registry, &routing);

        match &router.default_strategy {
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::CostFirst { .. },
                ..
            } => {}
            other => panic!("expected CostFirst default strategy, got: {other:?}"),
        }
    }

    #[test]
    fn test_build_router_from_config_with_model_overrides() {
        use prism_core::router::Router;

        let yaml = r#"
providers:
  openai:
    base_url: https://api.openai.com/v1
models:
  - id: gpt-4o
    display_name: GPT-4o
    quality_tier: S
  - id: gpt-4o-mini
    display_name: GPT-4o Mini
    quality_tier: B
    pricing:
      input_per_1m_tokens: 0.15
      output_per_1m_tokens: 0.6
variants:
  - id: gpt-4o@openai
    model_id: gpt-4o
    provider_id: openai
  - id: gpt-4o-mini@openai
    model_id: gpt-4o-mini
    provider_id: openai
pools:
  - id: default
    name: Default
    variants: [gpt-4o@openai, gpt-4o-mini@openai]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = build_registry(&config);
        let routing = RoutingConfig {
            default_strategy: "balanced".into(),
            model_overrides: vec![super::super::types::ModelStrategyOverride {
                model_id: "gpt-4o-mini".into(),
                strategy: "cost_first".into(),
                pool: None,
            }],
            ..Default::default()
        };

        let router = build_router_from_config(registry, &routing);

        assert_eq!(router.model_overrides.len(), 1);
        assert_eq!(router.model_overrides[0].0, "gpt-4o-mini");

        // Route with requested_model = gpt-4o-mini should use the override
        let request = prism_core::router::RouteRequest {
            messages: vec![prism_core::analyzer::ChatMessage {
                role: "user".into(),
                content: "hello".into(),
            }],
            strategy: None,
            metadata: prism_core::router::RouteMetadata::default(),
            analyze_context: None,
            required_protocol: None,
            requested_model: Some("gpt-4o-mini".into()),
        };

        let decision = router.route(&request).expect("routing should succeed");
        assert_eq!(decision.selected_model, "gpt-4o-mini");
    }

    #[test]
    fn test_build_router_from_config_l2_l3_l4_selectors() {
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
pools:
  - id: default
    name: Default
    variants: [m1@p1]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = build_registry(&config);
        let routing = RoutingConfig {
            l2_selector: "lowest_latency".into(),
            l3_selector: "round_robin".into(),
            l4_selector: "weighted".into(),
            ..Default::default()
        };

        let router = build_router_from_config(registry, &routing);

        // Verify routing works with custom selectors
        use prism_core::router::Router;
        let request = prism_core::router::RouteRequest {
            messages: vec![prism_core::analyzer::ChatMessage {
                role: "user".into(),
                content: "hello".into(),
            }],
            strategy: None,
            metadata: prism_core::router::RouteMetadata::default(),
            analyze_context: None,
            required_protocol: None,
            requested_model: None,
        };

        let decision = router.route(&request).expect("routing should succeed");
        assert_eq!(decision.selected_variant, "m1@p1");
    }

    #[test]
    fn test_build_router_from_config_resilience() {
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
pools:
  - id: default
    name: Default
    variants: [m1@p1]
"#;
        let config: PrismConfig = serde_yaml::from_str(yaml).unwrap();
        let registry = build_registry(&config);
        let routing = RoutingConfig {
            resilience: super::super::types::ResilienceConfig {
                retry: super::super::types::RetryConfig {
                    max_retries: 5,
                    retry_on: vec!["RateLimit".into(), "Timeout".into(), "ConnectionError".into()],
                    backoff: "Jittered".into(),
                    retry_scope: "SameProvider".into(),
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let router = build_router_from_config(registry, &routing);

        // Verify routing works with custom resilience
        use prism_core::router::Router;
        let request = prism_core::router::RouteRequest {
            messages: vec![prism_core::analyzer::ChatMessage {
                role: "user".into(),
                content: "hello".into(),
            }],
            strategy: None,
            metadata: prism_core::router::RouteMetadata::default(),
            analyze_context: None,
            required_protocol: None,
            requested_model: None,
        };

        let decision = router.route(&request).expect("routing should succeed");
        assert_eq!(decision.selected_variant, "m1@p1");
    }
}
