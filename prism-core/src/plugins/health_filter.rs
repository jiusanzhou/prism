//! Filter: eliminate unhealthy candidates.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::{HealthStatus, ModelCandidate, VariantStatus};

/// Health filter plugin.
///
/// Eliminates candidates whose provider, endpoint, or account is not Healthy,
/// or whose variant is not Active.
pub struct HealthFilterPlugin;

impl RoutingPlugin for HealthFilterPlugin {
    fn name(&self) -> &str {
        "health_filter"
    }

    fn filter(&self, _state: &PluginState, candidate: &ModelCandidate) -> bool {
        candidate.provider.status == HealthStatus::Healthy
            && candidate.endpoint.status == HealthStatus::Healthy
            && candidate.account.status == HealthStatus::Healthy
            && candidate.variant.status == VariantStatus::Active
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

    fn make_state() -> PluginState {
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
            None,
        )
    }

    fn make_candidate_health(
        provider_status: HealthStatus,
        endpoint_status: HealthStatus,
        account_status: HealthStatus,
        variant_status: VariantStatus,
    ) -> ModelCandidate {
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
                status: variant_status,
                base_url: None,
                upstream_model_name: None,
            metadata: std::collections::HashMap::new(),
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
                status: account_status,
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
                status: endpoint_status,
            },
            provider: Provider {
                id: "prov-1".into(),
                name: "Provider 1".into(),
                auth: AuthConfig::default(),
                base_url: "https://api.example.com".into(),
                rate_limit: None,
                status: provider_status,
                metadata: HashMap::new(),
                protocols: vec![],
            },
            effective_pricing: ModelPricing::default(),
        }
    }

    #[test]
    fn test_filter_healthy() {
        let plugin = HealthFilterPlugin;
        let state = make_state();
        let candidate = make_candidate_health(
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            VariantStatus::Active,
        );
        assert!(plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_filter_unhealthy_provider() {
        let plugin = HealthFilterPlugin;
        let state = make_state();
        let candidate = make_candidate_health(
            HealthStatus::Down,
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            VariantStatus::Active,
        );
        assert!(!plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_filter_variant_disabled() {
        let plugin = HealthFilterPlugin;
        let state = make_state();
        let candidate = make_candidate_health(
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            HealthStatus::Healthy,
            VariantStatus::Disabled,
        );
        assert!(!plugin.filter(&state, &candidate));
    }
}
