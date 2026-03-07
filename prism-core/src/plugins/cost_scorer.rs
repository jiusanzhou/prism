//! Score: cost-based scoring (cheaper is better).

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Cost scorer plugin.
///
/// Scores candidates inversely proportional to cost per 1M tokens.
pub struct CostScorerPlugin;

impl RoutingPlugin for CostScorerPlugin {
    fn name(&self) -> &str {
        "cost_scorer"
    }

    fn score(&self, _state: &PluginState, candidate: &ModelCandidate) -> f32 {
        let max_cost = 100.0_f64;
        (1.0 - (candidate.effective_pricing.cost_per_1m() / max_cost).min(1.0)) as f32
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

    fn make_candidate(cost_in: f64, cost_out: f64) -> ModelCandidate {
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
                pricing: ModelPricing {
                    input_per_1m_tokens: cost_in,
                    output_per_1m_tokens: cost_out,
                    currency: "USD".to_string(),
                },
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
            effective_pricing: ModelPricing {
                input_per_1m_tokens: cost_in,
                output_per_1m_tokens: cost_out,
                currency: "USD".to_string(),
            },
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
    fn test_free_model_scores_1() {
        let plugin = CostScorerPlugin;
        let state = default_state();
        let candidate = make_candidate(0.0, 0.0);
        let score = plugin.score(&state, &candidate);
        assert!((score - 1.0).abs() < f32::EPSILON, "free model should score 1.0, got {score}");
    }

    #[test]
    fn test_expensive_model_scores_low() {
        let plugin = CostScorerPlugin;
        let state = default_state();
        // cost_per_1m = (100 + 100) / 2 = 100
        let candidate = make_candidate(100.0, 100.0);
        let score = plugin.score(&state, &candidate);
        assert!(score < 0.01, "expensive model (cost=100) should score ~0.0, got {score}");
    }

    #[test]
    fn test_mid_cost_model() {
        let plugin = CostScorerPlugin;
        let state = default_state();
        // cost_per_1m = (10 + 10) / 2 = 10
        let candidate = make_candidate(10.0, 10.0);
        let score = plugin.score(&state, &candidate);
        assert!((score - 0.9).abs() < 0.01, "mid-cost model (cost=10) should score ~0.9, got {score}");
    }
}
