//! Score: quality tier scoring.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::{ModelCandidate, QualityTier};

/// Quality scorer plugin.
///
/// Scores candidates based on their model quality tier.
pub struct QualityScorerPlugin;

impl RoutingPlugin for QualityScorerPlugin {
    fn name(&self) -> &str {
        "quality_scorer"
    }

    fn score(&self, _state: &PluginState, candidate: &ModelCandidate) -> f32 {
        match candidate.model.quality_tier {
            QualityTier::S => 1.0,
            QualityTier::A => 0.75,
            QualityTier::B => 0.5,
            QualityTier::C => 0.25,
        }
    }
}
