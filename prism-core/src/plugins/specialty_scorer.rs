//! Score: specialty matching bonus.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Specialty scorer plugin.
///
/// Awards a score of 1.0 when the model's specialties include the
/// detected task type, 0.0 otherwise.
pub struct SpecialtyScorerPlugin;

impl RoutingPlugin for SpecialtyScorerPlugin {
    fn name(&self) -> &str {
        "specialty_scorer"
    }

    fn score(&self, state: &PluginState, candidate: &ModelCandidate) -> f32 {
        if candidate
            .model
            .specialties
            .contains(&state.profile.task_type)
        {
            1.0
        } else {
            0.0
        }
    }
}
