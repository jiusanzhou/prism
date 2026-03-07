//! Filter: pinned model — only pass candidates matching the pinned model id.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::strategy::ModelSelectionStrategy;
use crate::types::ModelCandidate;

/// Pinned model plugin.
///
/// When the strategy is `Pinned { model_id }`, eliminates candidates
/// that do not match the specified model id. For other strategies,
/// all candidates pass.
pub struct PinnedModelPlugin;

impl RoutingPlugin for PinnedModelPlugin {
    fn name(&self) -> &str {
        "pinned_model"
    }

    fn filter(&self, state: &PluginState, candidate: &ModelCandidate) -> bool {
        match &state.strategy {
            ModelSelectionStrategy::Pinned { model_id } => candidate.model.id == *model_id,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{ContextSize, Language, RequestProfile};
    use crate::plugin::PluginState;
    use crate::router::RouteMetadata;
    use crate::types::*;
    use std::collections::HashMap;

    fn make_state(strategy: ModelSelectionStrategy) -> PluginState {
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
            strategy,
            None,
        )
    }

    fn make_candidate(model_id: &str) -> ModelCandidate {
        ModelCandidate {
            variant: ModelVariant {
                id: "v-1".to_string(),
                model_id: model_id.into(),
                endpoint_id: "ep-1".into(),
                account_id: None,
                deployment_name: None,
                weight: 1.0,
                performance: ModelPerformance::default(),
                pricing_override: None,
                status: VariantStatus::Active,
                base_url: None,
                upstream_model_name: None,
            metadata: std::collections::HashMap::new(),
            },
            model: Model {
                id: model_id.into(),
                display_name: "Model".into(),
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
                protocols: vec![],
            },
            effective_pricing: ModelPricing::default(),
        }
    }

    #[test]
    fn test_filter_matches() {
        let plugin = PinnedModelPlugin;
        let state = make_state(ModelSelectionStrategy::Pinned {
            model_id: "model-1".into(),
        });
        let candidate = make_candidate("model-1");
        assert!(plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_filter_no_match() {
        let plugin = PinnedModelPlugin;
        let state = make_state(ModelSelectionStrategy::Pinned {
            model_id: "other-model".into(),
        });
        let candidate = make_candidate("model-1");
        assert!(!plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_non_pinned_passthrough() {
        let plugin = PinnedModelPlugin;
        let state = make_state(ModelSelectionStrategy::default());
        let candidate = make_candidate("model-1");
        assert!(plugin.filter(&state, &candidate));
    }
}
