//! Score: specialty matching bonus.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Specialty scorer plugin.
///
/// Awards a score of 1.0 when the model's specialties include the
/// detected task type, 0.0 otherwise.
pub struct SpecialtyScorerPlugin;

impl RoutingPlugin for SpecialtyScorerPlugin {
    fn name(&self) -> &str {
        "specialty_scorer"
    }

    fn score(&self, state: &PluginState, candidate: &ModelCandidate) -> f32 {
        if candidate
            .model
            .specialties
            .contains(&state.profile.task_type)
        {
            1.0
        } else {
            0.0
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

    fn make_candidate_with_specialties(specialties: Vec<TaskType>) -> ModelCandidate {
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
                quality_tier: QualityTier::A,
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
                specialties,
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

    fn make_state_with_task(task_type: TaskType) -> PluginState {
        PluginState::new(
            RequestProfile {
                complexity: 0.5,
                task_type,
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
    fn test_matching_specialty_scores_1() {
        let plugin = SpecialtyScorerPlugin;
        let state = make_state_with_task(TaskType::CodeGeneration);
        let candidate =
            make_candidate_with_specialties(vec![TaskType::CodeGeneration, TaskType::Reasoning]);
        let score = plugin.score(&state, &candidate);
        assert!(
            (score - 1.0).abs() < f32::EPSILON,
            "matching specialty should score 1.0, got {score}"
        );
    }

    #[test]
    fn test_no_matching_specialty_scores_0() {
        let plugin = SpecialtyScorerPlugin;
        let state = make_state_with_task(TaskType::CodeGeneration);
        let candidate =
            make_candidate_with_specialties(vec![TaskType::Translation, TaskType::Creative]);
        let score = plugin.score(&state, &candidate);
        assert!(
            score.abs() < f32::EPSILON,
            "non-matching specialty should score 0.0, got {score}"
        );
    }

    #[test]
    fn test_empty_specialties() {
        let plugin = SpecialtyScorerPlugin;
        let state = make_state_with_task(TaskType::CodeGeneration);
        let candidate = make_candidate_with_specialties(vec![]);
        let score = plugin.score(&state, &candidate);
        assert!(
            score.abs() < f32::EPSILON,
            "empty specialties should score 0.0, got {score}"
        );
    }
}
