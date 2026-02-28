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
