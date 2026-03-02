//! Score: quota-aware scoring (lower utilisation → higher score).

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Quota-aware scorer plugin.
///
/// Scores candidates inversely proportional to their rate-limit utilisation.
/// Candidates with no rate limit receive a full score of 1.0.
pub struct QuotaAwarePlugin;

impl RoutingPlugin for QuotaAwarePlugin {
    fn name(&self) -> &str {
        "quota_aware"
    }

    fn score(&self, _state: &PluginState, candidate: &ModelCandidate) -> f32 {
        let effective_rpm = candidate
            .account
            .rate_limit
            .as_ref()
            .and_then(|rl| rl.rpm)
            .or_else(|| candidate.endpoint.rate_limit.as_ref().and_then(|rl| rl.rpm))
            .or_else(|| candidate.provider.rate_limit.as_ref().and_then(|rl| rl.rpm));

        match effective_rpm {
            Some(rpm) if rpm > 0 => {
                let rps_limit = rpm as f32 / 60.0;
                let utilisation = (candidate.variant.performance.current_qps / rps_limit).min(1.0);
                1.0 - utilisation
            }
            _ => 1.0, // no rate limit → full score
        }
    }
}
