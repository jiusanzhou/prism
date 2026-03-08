//! Score: strategy-aware scorer that adapts to RoutingObjective.
//!
//! This plugin reads the strategy from PluginState and computes a
//! composite score that mirrors the original L1 scoring behavior
//! for Balanced/CostFirst/QualityFirst/SloFirst objectives.

use crate::analyzer::RequestProfile;
use crate::plugin::{PluginState, RoutingPlugin};
use crate::strategy::{ModelSelectionStrategy, RoutingObjective};
use crate::types::{ModelCandidate, QualityTier};

/// Strategy-aware scorer plugin.
///
/// Computes a score based on the routing objective configured in the strategy.
/// This plugin provides the primary scoring signal and should have a high weight.
pub struct StrategyScorerPlugin;

impl StrategyScorerPlugin {
    fn estimate_quality(profile: &RequestProfile, candidate: &ModelCandidate) -> f32 {
        let tier_score: f32 = match candidate.model.quality_tier {
            QualityTier::S => 1.0,
            QualityTier::A => 0.75,
            QualityTier::B => 0.5,
            QualityTier::C => 0.25,
        };

        let specialty_bonus: f32 = if candidate.model.specialties.contains(&profile.task_type) {
            0.15
        } else {
            0.0
        };

        (tier_score + specialty_bonus).min(1.0)
    }
}

impl RoutingPlugin for StrategyScorerPlugin {
    fn name(&self) -> &str {
        "strategy_scorer"
    }

    fn filter(&self, state: &PluginState, candidate: &ModelCandidate) -> bool {
        // SloFirst hard constraints: filter out candidates that violate SLO
        if let ModelSelectionStrategy::Auto { ref objective, .. } = state.strategy {
            match objective {
                RoutingObjective::SloFirst {
                    max_latency_ms,
                    max_ttft_ms,
                    min_quality_score,
                } => {
                    if candidate.variant.performance.p99_latency_ms > *max_latency_ms {
                        return false;
                    }
                    if candidate.variant.performance.ttft_p50_ms > *max_ttft_ms {
                        return false;
                    }
                    let quality = Self::estimate_quality(&state.profile, candidate);
                    if quality < *min_quality_score {
                        return false;
                    }
                }
                RoutingObjective::CostFirst {
                    min_quality_score, ..
                } => {
                    let quality = Self::estimate_quality(&state.profile, candidate);
                    if quality < *min_quality_score {
                        return false;
                    }
                }
                RoutingObjective::QualityFirst {
                    max_latency_ms: Some(max_lat),
                } => {
                    if candidate.variant.performance.p99_latency_ms > *max_lat {
                        return false;
                    }
                }
                RoutingObjective::QualityFirst {
                    max_latency_ms: None,
                } => {}
                _ => {}
            }
        }
        true
    }

    fn score(&self, state: &PluginState, candidate: &ModelCandidate) -> f32 {
        let objective = match &state.strategy {
            ModelSelectionStrategy::Auto { ref objective, .. } => objective,
            _ => return 0.5, // Pinned/Cascade: neutral score
        };

        match objective {
            RoutingObjective::Balanced {
                cost_weight,
                quality_weight,
                latency_weight,
            } => {
                let max_cost = 100.0_f64;
                let max_latency = 10000.0_f32;

                let cost_score =
                    1.0 - (candidate.effective_pricing.cost_per_1m() / max_cost).min(1.0);
                let quality_score = Self::estimate_quality(&state.profile, candidate) as f64;
                let latency_score = 1.0
                    - (candidate.variant.performance.p50_latency_ms as f32 / max_latency).min(1.0);

                (cost_score as f32 * cost_weight)
                    + (quality_score as f32 * quality_weight)
                    + (latency_score * latency_weight)
            }
            RoutingObjective::SloFirst { .. } => {
                // Candidates that pass filter are all valid; score by quality
                Self::estimate_quality(&state.profile, candidate)
            }
            RoutingObjective::CostFirst { .. } => {
                // For CostFirst, the scoring must decisively separate candidates
                // by cost so that supplementary plugins (quality, latency) don't
                // override the cheapest choice.
                //
                // Use reciprocal scaling: score = 1 / (1 + cost_per_1m)
                // This maps: cost 0 → 1.0, cost 0.185 → 0.844, cost 0.375 → 0.727,
                //            cost 3.0 → 0.25, cost 57.5 → 0.017
                // The large spread at low costs ensures the cheapest wins.
                let cost = candidate.effective_pricing.cost_per_1m();
                (1.0 / (1.0 + cost)) as f32
            }
            RoutingObjective::QualityFirst { .. } => {
                Self::estimate_quality(&state.profile, candidate)
            }
            RoutingObjective::Custom { .. } => 0.5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{ContextSize, Language};
    use crate::plugin::RoutingPlugin;
    use crate::router::RouteMetadata;
    use crate::types::*;

    fn make_candidate(
        quality: QualityTier,
        cost_in: f64,
        cost_out: f64,
        p50: u32,
        p99: u32,
        ttft: u32,
        specialties: Vec<TaskType>,
    ) -> ModelCandidate {
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
                    p99_latency_ms: p99,
                    ttft_p50_ms: ttft,
                    ..Default::default()
                },
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
                pricing: ModelPricing {
                    input_per_1m_tokens: cost_in,
                    output_per_1m_tokens: cost_out,
                    currency: "USD".to_string(),
                },
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
            effective_pricing: ModelPricing {
                input_per_1m_tokens: cost_in,
                output_per_1m_tokens: cost_out,
                currency: "USD".to_string(),
            },
        }
    }

    fn make_state(objective: RoutingObjective, task_type: TaskType) -> PluginState {
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
            ModelSelectionStrategy::Auto {
                objective,
                pool_ids: None,
                required_capabilities: None,
            },
            None,
        )
    }

    #[test]
    fn test_balanced_objective_score() {
        let plugin = StrategyScorerPlugin;
        let state = make_state(
            RoutingObjective::Balanced {
                cost_weight: 0.3,
                quality_weight: 0.5,
                latency_weight: 0.2,
            },
            TaskType::SimpleQA,
        );
        // cost_per_1m = (0+0)/2 = 0 => cost_score = 1.0
        // quality_tier = S => estimate_quality = 1.0 (no specialty bonus)
        // p50 = 0 => latency_score = 1.0
        // weighted = 1.0*0.3 + 1.0*0.5 + 1.0*0.2 = 1.0
        let candidate = make_candidate(QualityTier::S, 0.0, 0.0, 0, 0, 0, vec![]);
        let score = plugin.score(&state, &candidate);
        assert!(
            (score - 1.0).abs() < 0.01,
            "balanced objective with perfect candidate should score ~1.0, got {score}"
        );

        // Another candidate: cost=50, quality=B, p50=5000
        // cost_per_1m = (50+50)/2 = 50 => cost_score = 1.0 - 0.5 = 0.5
        // quality_tier = B => 0.5
        // p50 = 5000 => latency_score = 0.5
        // weighted = 0.5*0.3 + 0.5*0.5 + 0.5*0.2 = 0.5
        let candidate2 = make_candidate(QualityTier::B, 50.0, 50.0, 5000, 5000, 0, vec![]);
        let score2 = plugin.score(&state, &candidate2);
        assert!(
            (score2 - 0.5).abs() < 0.05,
            "balanced objective with mid candidate should score ~0.5, got {score2}"
        );
    }

    #[test]
    fn test_cost_first_filters_low_quality() {
        let plugin = StrategyScorerPlugin;
        let state = make_state(
            RoutingObjective::CostFirst {
                min_quality_score: 0.4,
                budget_per_request: None,
            },
            TaskType::SimpleQA,
        );

        // C tier => estimate_quality = 0.25, below min_quality_score of 0.4
        let low_quality = make_candidate(QualityTier::C, 0.0, 0.0, 0, 0, 0, vec![]);
        assert!(
            !plugin.filter(&state, &low_quality),
            "CostFirst should filter out candidate below min_quality_score"
        );

        // A tier => estimate_quality = 0.75, above 0.4
        let high_quality = make_candidate(QualityTier::A, 0.0, 0.0, 0, 0, 0, vec![]);
        assert!(
            plugin.filter(&state, &high_quality),
            "CostFirst should pass candidate above min_quality_score"
        );
    }

    #[test]
    fn test_quality_first_score_by_tier() {
        let plugin = StrategyScorerPlugin;
        let state = make_state(
            RoutingObjective::QualityFirst {
                max_latency_ms: None,
            },
            TaskType::SimpleQA,
        );

        let s_tier = make_candidate(QualityTier::S, 0.0, 0.0, 0, 0, 0, vec![]);
        let c_tier = make_candidate(QualityTier::C, 0.0, 0.0, 0, 0, 0, vec![]);

        let score_s = plugin.score(&state, &s_tier);
        let score_c = plugin.score(&state, &c_tier);

        assert!(
            score_s > score_c,
            "QualityFirst should score S tier ({score_s}) higher than C tier ({score_c})"
        );
        // S tier with no specialty should score 1.0
        assert!(
            (score_s - 1.0).abs() < f32::EPSILON,
            "QualityFirst S tier should score 1.0, got {score_s}"
        );
        // C tier with no specialty should score 0.25
        assert!(
            (score_c - 0.25).abs() < f32::EPSILON,
            "QualityFirst C tier should score 0.25, got {score_c}"
        );
    }

    #[test]
    fn test_slo_first_filters_high_latency() {
        let plugin = StrategyScorerPlugin;
        let state = make_state(
            RoutingObjective::SloFirst {
                max_latency_ms: 200,
                max_ttft_ms: 100,
                min_quality_score: 0.0,
            },
            TaskType::SimpleQA,
        );

        // p99 = 500 > max_latency_ms of 200 => should be filtered
        let high_latency = make_candidate(QualityTier::A, 0.0, 0.0, 100, 500, 50, vec![]);
        assert!(
            !plugin.filter(&state, &high_latency),
            "SloFirst should filter candidate with p99 ({}) > max_latency_ms (200)",
            500
        );

        // p99 = 150 <= 200, ttft = 80 <= 100 => should pass
        let ok_latency = make_candidate(QualityTier::A, 0.0, 0.0, 100, 150, 80, vec![]);
        assert!(
            plugin.filter(&state, &ok_latency),
            "SloFirst should pass candidate within SLO bounds"
        );

        // ttft = 150 > max_ttft_ms of 100 => should be filtered
        let high_ttft = make_candidate(QualityTier::A, 0.0, 0.0, 100, 150, 150, vec![]);
        assert!(
            !plugin.filter(&state, &high_ttft),
            "SloFirst should filter candidate with ttft ({}) > max_ttft_ms (100)",
            150
        );
    }
}
