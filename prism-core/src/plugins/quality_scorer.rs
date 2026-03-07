//! Score: quality tier scoring.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::{ModelCandidate, QualityTier};

/// Quality scorer plugin.
///
/// Scores candidates based on their model quality tier.
pub struct QualityScorerPlugin;

impl RoutingPlugin for QualityScorerPlugin {
    fn name(&self) -> &str {
        "quality_scorer"
    }

    fn score(&self, _state: &PluginState, candidate: &ModelCandidate) -> f32 {
        match candidate.model.quality_tier {
            QualityTier::S => 1.0,
            QualityTier::A => 0.75,
            QualityTier::B => 0.5,
            QualityTier::C => 0.25,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{ContextSize, Language, RequestProfile};
    use crate::plugin::RoutingPlugin;
    use crate::router::RouteMetadata;
    use crate::strategy::ModelSelectionStrategy;
    use crate::types::*;

    fn make_candidate(quality: QualityTier) -> ModelCandidate {
        ModelCandidate {
            variant: ModelVariant {
                id: "v-1".to_string(),
                model_id: "m-1".to_string(),
                endpoint_id: "ep-1".to_string(),
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
                id: "m-1".to_string(),
                display_name: "m-1".to_string(),
                quality_tier: quality,
                capabilities: ModelCapabilities {
                    max_context_window: 128000,
                    max_output_tokens: 4096,
                    supports_streaming: true,
                    supports_function_calling: true,
                    supports_vision: false,
                    supports_json_mode: false,
                    supports_reasoning: false,
                },
                pricing: ModelPricing::default(),
                specialties: vec![],
                limits: None,
                status: ModelStatus::Available,
            },
            account: Account {
                id: "acc-1".to_string(),
                endpoint_id: "ep-1".to_string(),
                auth: AuthConfig::default(),
                project_id: None,
                rate_limit: None,
                billing_tag: None,
                weight: 1.0,
                status: HealthStatus::Healthy,
                metadata: Default::default(),
            },
            endpoint: Endpoint {
                id: "ep-1".to_string(),
                provider_id: "prov-1".to_string(),
                base_url: None,
                region: None,
                auth_override: None,
                rate_limit: None,
                priority: 0,
                status: HealthStatus::Healthy,
            },
            provider: Provider {
                id: "prov-1".to_string(),
                name: "Test".to_string(),
                auth: AuthConfig::default(),
                base_url: "https://api.test.com".to_string(),
                rate_limit: None,
                status: HealthStatus::Healthy,
                metadata: Default::default(),
                protocols: vec![],
            },
            effective_pricing: ModelPricing::default(),
        }
    }

    fn default_state() -> PluginState {
        PluginState::new(
            RequestProfile {
                complexity: 0.5,
                task_type: crate::types::TaskType::SimpleQA,
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

    #[test]
    fn test_quality_tiers() {
        let plugin = QualityScorerPlugin;
        let state = default_state();

        let s = plugin.score(&state, &make_candidate(QualityTier::S));
        let a = plugin.score(&state, &make_candidate(QualityTier::A));
        let b = plugin.score(&state, &make_candidate(QualityTier::B));
        let c = plugin.score(&state, &make_candidate(QualityTier::C));

        assert!((s - 1.0).abs() < f32::EPSILON, "S tier should be 1.0, got {s}");
        assert!((a - 0.75).abs() < f32::EPSILON, "A tier should be 0.75, got {a}");
        assert!((b - 0.5).abs() < f32::EPSILON, "B tier should be 0.5, got {b}");
        assert!((c - 0.25).abs() < f32::EPSILON, "C tier should be 0.25, got {c}");
    }

    #[test]
    fn test_name() {
        let plugin = QualityScorerPlugin;
        assert_eq!(plugin.name(), "quality_scorer");
    }
}
