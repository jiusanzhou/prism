//! Filter: eliminate candidates that lack required capabilities.

use crate::analyzer::ContextSize;
use crate::plugin::{PluginState, RoutingPlugin};
use crate::strategy::ModelSelectionStrategy;
use crate::types::ModelCandidate;

/// Capability filter plugin.
///
/// Checks vision, reasoning, context window, function calling, and json mode
/// against the request profile and strategy's required_capabilities.
pub struct CapabilityFilterPlugin;

impl RoutingPlugin for CapabilityFilterPlugin {
    fn name(&self) -> &str {
        "capability_filter"
    }

    fn filter(&self, state: &PluginState, candidate: &ModelCandidate) -> bool {
        let caps = &candidate.model.capabilities;

        // Check profile-driven requirements
        if state.profile.requires_vision && !caps.supports_vision {
            return false;
        }
        if state.profile.requires_reasoning && !caps.supports_reasoning {
            return false;
        }

        // Context window check
        let min_ctx = match state.profile.context_window_need {
            ContextSize::Small => 0,
            ContextSize::Medium => 4000,
            ContextSize::Large => 32000,
            ContextSize::XLarge => 128000,
        };
        if caps.max_context_window < min_ctx {
            return false;
        }

        // Strategy-driven required_capabilities
        if let ModelSelectionStrategy::Auto {
            required_capabilities: Some(ref req),
            ..
        } = state.strategy
        {
            if let Some(min_cw) = req.min_context_window {
                if caps.max_context_window < min_cw {
                    return false;
                }
            }
            if let Some(true) = req.function_calling {
                if !caps.supports_function_calling {
                    return false;
                }
            }
            if let Some(true) = req.vision {
                if !caps.supports_vision {
                    return false;
                }
            }
            if let Some(true) = req.json_mode {
                if !caps.supports_json_mode {
                    return false;
                }
            }
            if let Some(true) = req.reasoning {
                if !caps.supports_reasoning {
                    return false;
                }
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{Language, RequestProfile};
    use crate::plugin::PluginState;
    use crate::router::RouteMetadata;
    use crate::types::*;
    use std::collections::HashMap;

    fn make_state_with(requires_vision: bool, requires_reasoning: bool, ctx: ContextSize) -> PluginState {
        PluginState::new(
            RequestProfile {
                complexity: 0.3,
                task_type: TaskType::SimpleQA,
                estimated_tokens: 100,
                requires_reasoning,
                requires_vision,
                language: Language::English,
                is_multi_turn: false,
                context_window_need: ctx,
            },
            RouteMetadata::default(),
            ModelSelectionStrategy::default(),
            None,
        )
    }

    fn make_candidate_with_caps(caps: ModelCapabilities) -> ModelCandidate {
        ModelCandidate {
            variant: ModelVariant {
                id: "v-1".to_string(),
                model_id: "model-1".into(),
                endpoint_id: "ep-1".into(),
                account_id: None,
                deployment_name: None,
                weight: 1.0,
                performance: ModelPerformance::default(),
                pricing_override: None,
                status: VariantStatus::Active,
                base_url: None,
                upstream_model_name: None,
            },
            model: Model {
                id: "model-1".into(),
                display_name: "Model 1".into(),
                quality_tier: QualityTier::A,
                capabilities: caps,
                pricing: ModelPricing::default(),
                specialties: vec![],
                limits: None,
                status: ModelStatus::Available,
            },
            account: Account {
                id: "acc-1".into(),
                endpoint_id: "ep-1".into(),
                auth: AuthConfig::default(),
                project_id: None,
                rate_limit: None,
                billing_tag: None,
                weight: 1.0,
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
            },
            endpoint: Endpoint {
                id: "ep-1".into(),
                provider_id: "prov-1".into(),
                base_url: None,
                region: None,
                auth_override: None,
                rate_limit: None,
                priority: 0,
                status: HealthStatus::Healthy,
            },
            provider: Provider {
                id: "prov-1".into(),
                name: "Provider 1".into(),
                auth: AuthConfig::default(),
                base_url: "https://api.example.com".into(),
                rate_limit: None,
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
                protocols: vec![],
            },
            effective_pricing: ModelPricing::default(),
        }
    }

    #[test]
    fn test_filter_vision() {
        let plugin = CapabilityFilterPlugin;
        let state = make_state_with(true, false, ContextSize::Small);
        let candidate = make_candidate_with_caps(ModelCapabilities {
            supports_vision: false,
            ..Default::default()
        });
        assert!(!plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_filter_context_window() {
        let plugin = CapabilityFilterPlugin;
        let state = make_state_with(false, false, ContextSize::XLarge);
        let candidate = make_candidate_with_caps(ModelCapabilities {
            max_context_window: 32000,
            ..Default::default()
        });
        assert!(!plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_passthrough() {
        let plugin = CapabilityFilterPlugin;
        let state = make_state_with(false, false, ContextSize::Small);
        let candidate = make_candidate_with_caps(ModelCapabilities::default());
        assert!(plugin.filter(&state, &candidate));
    }
}
