//! Filter: pinned model — only pass candidates matching the pinned model id.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::strategy::ModelSelectionStrategy;
use crate::types::ModelCandidate;

/// Pinned model plugin.
///
/// When the strategy is `Pinned { model_id }`, eliminates candidates
/// that do not match the specified model id. For other strategies,
/// all candidates pass.
pub struct PinnedModelPlugin;

impl RoutingPlugin for PinnedModelPlugin {
    fn name(&self) -> &str {
        "pinned_model"
    }

    fn filter(&self, state: &PluginState, candidate: &ModelCandidate) -> bool {
        match &state.strategy {
            ModelSelectionStrategy::Pinned { model_id } => candidate.model.id == *model_id,
            _ => true,
        }
    }
}
