//! Score: geographic proximity scoring.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Geographic proximity plugin.
///
/// Scores 1.0 when the endpoint region matches the requested region,
/// 0.0 otherwise.
pub struct GeoProximityPlugin;

impl RoutingPlugin for GeoProximityPlugin {
    fn name(&self) -> &str {
        "geo_proximity"
    }

    fn score(&self, state: &PluginState, candidate: &ModelCandidate) -> f32 {
        if let Some(ref req_region) = state.metadata.region {
            if let Some(ref ep_region) = candidate.endpoint.region {
                if ep_region == req_region {
                    return 1.0;
                }
            }
        }
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{ContextSize, Language, RequestProfile};
    use crate::plugin::{PluginState, RoutingPlugin};
    use crate::router::RouteMetadata;
    use crate::strategy::ModelSelectionStrategy;
    use crate::types::*;
    use std::collections::HashMap;

    fn make_profile() -> RequestProfile {
        RequestProfile {
            complexity: 0.3,
            task_type: TaskType::SimpleQA,
            estimated_tokens: 100,
            requires_reasoning: false,
            requires_vision: false,
            language: Language::English,
            is_multi_turn: false,
            context_window_need: ContextSize::Small,
        }
    }

    fn make_state_with_region(region: Option<&str>) -> PluginState {
        PluginState::new(
            make_profile(),
            RouteMetadata {
                region: region.map(|r| r.to_string()),
                ..Default::default()
            },
            ModelSelectionStrategy::default(),
            None,
        )
    }

    fn make_candidate_with_region(region: Option<&str>) -> ModelCandidate {
        ModelCandidate {
            variant: ModelVariant {
                id: "v1".to_string(),
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
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
            },
            endpoint: Endpoint {
                id: "ep-1".into(),
                provider_id: "prov-1".into(),
                base_url: None,
                region: region.map(|r| r.to_string()),
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
    fn test_same_region_bonus() {
        let plugin = GeoProximityPlugin;
        let state = make_state_with_region(Some("us-east-1"));
        let candidate = make_candidate_with_region(Some("us-east-1"));
        let score = plugin.score(&state, &candidate);
        assert!(
            (score - 1.0).abs() < f32::EPSILON,
            "same region should produce score 1.0, got {}",
            score
        );
    }

    #[test]
    fn test_different_region() {
        let plugin = GeoProximityPlugin;
        let state = make_state_with_region(Some("us-east-1"));
        let candidate = make_candidate_with_region(Some("eu-west-1"));
        let score = plugin.score(&state, &candidate);
        assert!(
            score.abs() < f32::EPSILON,
            "different region should produce score 0.0, got {}",
            score
        );
    }

    #[test]
    fn test_no_metadata_region() {
        let plugin = GeoProximityPlugin;
        let state = make_state_with_region(None);
        let candidate = make_candidate_with_region(Some("us-east-1"));
        let score = plugin.score(&state, &candidate);
        assert!(
            score.abs() < f32::EPSILON,
            "no metadata region should produce score 0.0, got {}",
            score
        );
    }

    #[test]
    fn test_no_endpoint_region() {
        let plugin = GeoProximityPlugin;
        let state = make_state_with_region(Some("us-east-1"));
        let candidate = make_candidate_with_region(None);
        let score = plugin.score(&state, &candidate);
        assert!(
            score.abs() < f32::EPSILON,
            "no endpoint region should produce score 0.0, got {}",
            score
        );
    }
}
