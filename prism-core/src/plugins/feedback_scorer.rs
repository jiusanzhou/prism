//! Score: feedback-based quality adjustment.

use std::sync::Arc;

use crate::feedback::FeedbackCollector;
use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Feedback scorer plugin.
///
/// Adjusts scores based on recorded outcome feedback (success rate, user ratings).
/// Returns a value in [0.5, 1.5] mapped to [0.0, 1.0] for the scoring framework.
pub struct FeedbackScorerPlugin {
    feedback: Arc<FeedbackCollector>,
}

impl FeedbackScorerPlugin {
    pub fn new(feedback: Arc<FeedbackCollector>) -> Self {
        Self { feedback }
    }
}

impl RoutingPlugin for FeedbackScorerPlugin {
    fn name(&self) -> &str {
        "feedback_scorer"
    }

    fn score(&self, _state: &PluginState, candidate: &ModelCandidate) -> f32 {
        // quality_adjustment returns [0.5, 1.5] centered at 1.0
        // Map to [0.0, 1.0]: (adj - 0.5) / 1.0 = adj - 0.5
        let adj = self.feedback.quality_adjustment(&candidate.variant.id);
        (adj - 0.5).clamp(0.0, 1.0)
    }
}
