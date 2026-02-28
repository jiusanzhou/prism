//! Filter: eliminate unhealthy candidates.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::{HealthStatus, ModelCandidate, VariantStatus};

/// Health filter plugin.
///
/// Eliminates candidates whose provider, endpoint, or account is not Healthy,
/// or whose variant is not Active.
pub struct HealthFilterPlugin;

impl RoutingPlugin for HealthFilterPlugin {
    fn name(&self) -> &str {
        "health_filter"
    }

    fn filter(&self, _state: &PluginState, candidate: &ModelCandidate) -> bool {
        candidate.provider.status == HealthStatus::Healthy
            && candidate.endpoint.status == HealthStatus::Healthy
            && candidate.account.status == HealthStatus::Healthy
            && candidate.variant.status == VariantStatus::Active
    }
}
