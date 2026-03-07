//! Score: latency-based scoring (lower is better).

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Latency scorer plugin.
///
/// Scores candidates inversely proportional to p50 latency.
pub struct LatencyScorerPlugin;

impl RoutingPlugin for LatencyScorerPlugin {
    fn name(&self) -> &str {
        "latency_scorer"
    }

    fn score(&self, _state: &PluginState, candidate: &ModelCandidate) -> f32 {
        let max_latency = 10000.0_f32;
        1.0 - (candidate.variant.performance.p50_latency_ms as f32 / max_latency).min(1.0)
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

    fn make_candidate(p50: u32) -> ModelCandidate {
        ModelCandidate {
            variant: ModelVariant {
                id: "v-1".to_string(),
                model_id: "m-1".to_string(),
                endpoint_id: "ep-1".to_string(),
                account_id: None,
                deployment_name: None,
                weight: 1.0,
                performance: ModelPerformance {
                    p50_latency_ms: p50,
                    ..Default::default()
                },
                pricing_override: None,
                status: VariantStatus::Active,
                base_url: None,
                upstream_model_name: None,
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
    fn test_zero_latency_scores_1() {
        let plugin = LatencyScorerPlugin;
        let state = default_state();
        let candidate = make_candidate(0);
        let score = plugin.score(&state, &candidate);
        assert!((score - 1.0).abs() < f32::EPSILON, "zero latency should score 1.0, got {score}");
    }

    #[test]
    fn test_max_latency_scores_0() {
        let plugin = LatencyScorerPlugin;
        let state = default_state();
        let candidate = make_candidate(10000);
        let score = plugin.score(&state, &candidate);
        assert!(score.abs() < f32::EPSILON, "max latency (10000ms) should score 0.0, got {score}");
    }

    #[test]
    fn test_mid_latency() {
        let plugin = LatencyScorerPlugin;
        let state = default_state();
        let candidate = make_candidate(5000);
        let score = plugin.score(&state, &candidate);
        assert!((score - 0.5).abs() < 0.01, "mid latency (5000ms) should score ~0.5, got {score}");
    }
}
