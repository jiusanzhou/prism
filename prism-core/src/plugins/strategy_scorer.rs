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
