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
