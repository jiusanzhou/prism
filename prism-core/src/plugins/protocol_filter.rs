//! Filter: eliminate candidates that do not support the required protocol.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Protocol filter plugin.
///
/// Eliminates candidates whose provider does not list the required protocol.
/// If the provider's protocol list is empty, it is assumed to support all protocols.
pub struct ProtocolFilterPlugin;

impl RoutingPlugin for ProtocolFilterPlugin {
    fn name(&self) -> &str {
        "protocol_filter"
    }

    fn filter(&self, state: &PluginState, candidate: &ModelCandidate) -> bool {
        match &state.required_protocol {
            Some(protocol) => {
                candidate.provider.protocols.is_empty()
                    || candidate.provider.protocols.contains(protocol)
            }
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{ContextSize, Language, RequestProfile};
    use crate::plugin::PluginState;
    use crate::router::RouteMetadata;
    use crate::strategy::ModelSelectionStrategy;
    use crate::types::*;
    use std::collections::HashMap;

    fn make_state(required_protocol: Option<String>) -> PluginState {
        PluginState::new(
            RequestProfile {
                complexity: 0.3,
                task_type: TaskType::SimpleQA,
                estimated_tokens: 100,
                requires_reasoning: false,
                requires_vision: false,
                language: Language::English,
                is_multi_turn: false,
                context_window_need: ContextSize::Small,
            },
            RouteMetadata::default(),
            ModelSelectionStrategy::default(),
            required_protocol,
        )
    }

    fn make_candidate_with_protocols(protocols: Vec<String>) -> ModelCandidate {
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
                capabilities: ModelCapabilities::default(),
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
                protocols,
            },
            effective_pricing: ModelPricing::default(),
        }
    }

    #[test]
    fn test_filter_protocol_match() {
        let plugin = ProtocolFilterPlugin;
        let state = make_state(Some("openai".into()));
        let candidate = make_candidate_with_protocols(vec!["openai".into()]);
        assert!(plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_filter_protocol_mismatch() {
        let plugin = ProtocolFilterPlugin;
        let state = make_state(Some("anthropic".into()));
        let candidate = make_candidate_with_protocols(vec!["openai".into()]);
        assert!(!plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_empty_protocols_passes() {
        let plugin = ProtocolFilterPlugin;
        let state = make_state(Some("anthropic".into()));
        let candidate = make_candidate_with_protocols(vec![]);
        assert!(plugin.filter(&state, &candidate));
    }
}
