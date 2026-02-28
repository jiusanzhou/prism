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
