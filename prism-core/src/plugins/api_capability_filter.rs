//! Filter: eliminate candidates that do not support the required API capability.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::{ApiCapability, ModelCandidate, VARIANT_API_CAPS_KEY};

/// API capability filter plugin.
///
/// When `required_api_capability` is present in [`PluginState`], eliminates
/// candidates whose variant does not advertise that capability.
///
/// Capabilities are stored as a comma-separated string in variant metadata
/// under the [`VARIANT_API_CAPS_KEY`] key. If a variant has no capabilities
/// listed, it is assumed to support all capabilities (backward compatible).
pub struct ApiCapabilityFilterPlugin;

impl RoutingPlugin for ApiCapabilityFilterPlugin {
    fn name(&self) -> &str {
        "api_capability_filter"
    }

    fn filter(&self, state: &PluginState, candidate: &ModelCandidate) -> bool {
        let required = match state.get::<ApiCapability>("required_api_capability") {
            Some(cap) => cap,
            None => return true, // no requirement → pass all
        };

        let tag = required.as_tag();

        match candidate.variant.metadata.get(VARIANT_API_CAPS_KEY) {
            Some(caps_str) => caps_str.split(',').any(|c| c.trim() == tag),
            None => true, // no capabilities declared → assumed universal
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

    fn make_state(cap: Option<ApiCapability>) -> PluginState {
        let mut state = PluginState::new(
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
        );
        if let Some(c) = cap {
            state.set("required_api_capability", c);
        }
        state
    }

    fn make_candidate_with_api_caps(caps: Option<&str>) -> ModelCandidate {
        let mut metadata = HashMap::new();
        if let Some(c) = caps {
            metadata.insert(VARIANT_API_CAPS_KEY.to_string(), c.to_string());
        }
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
                metadata,
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
                protocols: vec![],
            },
            effective_pricing: ModelPricing::default(),
        }
    }

    #[test]
    fn test_no_requirement_passes_all() {
        let plugin = ApiCapabilityFilterPlugin;
        let state = make_state(None);
        let candidate = make_candidate_with_api_caps(Some("chat_completion"));
        assert!(plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_matching_capability_passes() {
        let plugin = ApiCapabilityFilterPlugin;
        let state = make_state(Some(ApiCapability::AudioTranscription));
        let candidate = make_candidate_with_api_caps(Some("chat_completion,audio_transcription"));
        assert!(plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_missing_capability_filtered() {
        let plugin = ApiCapabilityFilterPlugin;
        let state = make_state(Some(ApiCapability::AudioTranscription));
        let candidate = make_candidate_with_api_caps(Some("chat_completion,embedding"));
        assert!(!plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_no_caps_declared_passes() {
        let plugin = ApiCapabilityFilterPlugin;
        let state = make_state(Some(ApiCapability::ChatCompletion));
        let candidate = make_candidate_with_api_caps(None);
        assert!(plugin.filter(&state, &candidate), "no caps = universal");
    }

    #[test]
    fn test_custom_capability() {
        let plugin = ApiCapabilityFilterPlugin;
        let state = make_state(Some(ApiCapability::Custom("code_review".into())));
        let candidate = make_candidate_with_api_caps(Some("code_review,chat_completion"));
        assert!(plugin.filter(&state, &candidate));
    }
}
