//! Score: canary traffic splitting plugin.
//!
//! When the variant strategy is `Canary`, this plugin randomly boosts the
//! canary variant's score so that it wins ~canary_pct of the time.

use rand::Rng;
use std::sync::RwLock;

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Canary traffic splitting plugin.
///
/// Randomly gives the canary variant the highest possible score so that it
/// wins approximately `canary_pct` of routing decisions.
pub struct CanaryPlugin {
    config: RwLock<Option<CanaryConfig>>,
}

struct CanaryConfig {
    canary_variant_id: String,
    canary_pct: f32,
}

impl Default for CanaryPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl CanaryPlugin {
    pub fn new() -> Self {
        Self {
            config: RwLock::new(None),
        }
    }

    pub fn with_canary(variant_id: String, pct: f32) -> Self {
        Self {
            config: RwLock::new(Some(CanaryConfig {
                canary_variant_id: variant_id,
                canary_pct: pct,
            })),
        }
    }
}

impl RoutingPlugin for CanaryPlugin {
    fn name(&self) -> &str {
        "canary"
    }

    fn score(&self, _state: &PluginState, candidate: &ModelCandidate) -> f32 {
        let config = self.config.read().unwrap();
        let config = match config.as_ref() {
            Some(c) => c,
            None => return 0.5, // neutral when no canary configured
        };

        if candidate.variant.id == config.canary_variant_id {
            let mut rng = rand::thread_rng();
            let roll: f32 = rng.gen();
            if roll < config.canary_pct {
                // Boost canary to win
                return 100.0; // very high score to dominate
            }
            return 0.0; // canary loses this round
        }

        // Non-canary variants get neutral score
        0.5
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

    fn make_candidate(variant_id: &str) -> ModelCandidate {
        ModelCandidate {
            variant: ModelVariant {
                id: variant_id.to_string(),
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
    fn test_no_config_returns_neutral() {
        let plugin = CanaryPlugin::new();
        let state = default_state();
        let candidate = make_candidate("v-any");
        let score = plugin.score(&state, &candidate);
        assert!(
            (score - 0.5).abs() < f32::EPSILON,
            "no canary config should return 0.5, got {score}"
        );
    }

    #[test]
    fn test_canary_traffic_split() {
        let plugin = CanaryPlugin::with_canary("v-canary".to_string(), 0.3);
        let state = default_state();
        let candidate = make_candidate("v-canary");

        let mut boosted_count = 0;
        let iterations = 1000;
        for _ in 0..iterations {
            let score = plugin.score(&state, &candidate);
            if (score - 100.0).abs() < f32::EPSILON {
                boosted_count += 1;
            }
        }

        let pct = boosted_count as f64 / iterations as f64;
        assert!(
            pct > 0.15 && pct < 0.50,
            "canary with 30% should get boosted ~30% of the time, got {:.1}%",
            pct * 100.0
        );
    }

    #[test]
    fn test_non_canary_always_neutral() {
        let plugin = CanaryPlugin::with_canary("v-canary".to_string(), 0.3);
        let state = default_state();
        let candidate = make_candidate("v-stable");

        for _ in 0..100 {
            let score = plugin.score(&state, &candidate);
            assert!(
                (score - 0.5).abs() < f32::EPSILON,
                "non-canary variant should always get 0.5, got {score}"
            );
        }
    }
}
