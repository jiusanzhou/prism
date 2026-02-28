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
