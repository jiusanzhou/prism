//! Score: weighted random selection plugin.
//!
//! Randomly scores candidates to implement weighted random load distribution.
//! Uses the Gumbel-max trick: score = weight * -ln(-ln(U)), which gives
//! exact weighted sampling proportional to variant weights.

use rand::Rng;

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Weighted random selection plugin.
///
/// Uses the Gumbel-max trick for exact weighted random sampling:
/// score = ln(weight) + Gumbel(0,1) = ln(weight) - ln(-ln(U))
/// The candidate with the highest score wins, and the probability
/// of winning equals weight / sum(weights).
pub struct WeightedPlugin;

impl RoutingPlugin for WeightedPlugin {
    fn name(&self) -> &str {
        "weighted"
    }

    fn score(&self, _state: &PluginState, candidate: &ModelCandidate) -> f32 {
        let w = candidate.variant.weight.max(0.001); // avoid log(0)
        let mut rng = rand::thread_rng();
        let u: f64 = rng.gen::<f64>().max(1e-15); // avoid log(0)
                                                  // Gumbel-max: ln(w) - ln(-ln(u))
        let gumbel = -((-u.ln()).ln());
        ((w as f64).ln() + gumbel) as f32
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

    fn make_candidate_with_weight(weight: f32) -> ModelCandidate {
        ModelCandidate {
            variant: ModelVariant {
                id: format!("v-w{}", weight),
                model_id: "m-1".to_string(),
                endpoint_id: "ep-1".to_string(),
                account_id: None,
                deployment_name: None,
                weight,
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
    fn test_higher_weight_wins_more_often() {
        let plugin = WeightedPlugin;
        let state = default_state();
        let heavy = make_candidate_with_weight(10.0);
        let light = make_candidate_with_weight(1.0);

        let mut heavy_wins = 0;
        let iterations = 1000;
        for _ in 0..iterations {
            let score_heavy = plugin.score(&state, &heavy);
            let score_light = plugin.score(&state, &light);
            if score_heavy > score_light {
                heavy_wins += 1;
            }
        }

        let win_rate = heavy_wins as f64 / iterations as f64;
        assert!(
            win_rate > 0.60,
            "heavier weight (10.0) should win >60% of the time, got {:.1}%",
            win_rate * 100.0
        );
    }

    #[test]
    fn test_score_is_not_nan() {
        let plugin = WeightedPlugin;
        let state = default_state();
        let candidate = make_candidate_with_weight(1.0);

        for _ in 0..100 {
            let score = plugin.score(&state, &candidate);
            assert!(!score.is_nan(), "score should never be NaN");
        }
    }
}
