//! Score: affinity routing (tenant isolation + session binding).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Affinity plugin.
///
/// Provides tenant cost-isolation (exact match on billing_tag) and
/// hash-based affinity for session binding.
pub struct AffinityPlugin;

impl RoutingPlugin for AffinityPlugin {
    fn name(&self) -> &str {
        "affinity"
    }

    fn score(&self, state: &PluginState, candidate: &ModelCandidate) -> f32 {
        // Tenant cost isolation: strong preference for matching billing_tag
        if let Some(ref tenant_id) = state.metadata.tenant_id {
            if let Some(ref tag) = candidate.account.billing_tag {
                if tag == tenant_id {
                    return 1.0;
                }
            }
        }

        // Affinity hash: deterministic but distributed
        if let Some(ref affinity_key) = state.metadata.affinity_key {
            let mut hasher = DefaultHasher::new();
            affinity_key.hash(&mut hasher);
            candidate.account.id.hash(&mut hasher);
            let h = hasher.finish();
            return (h % 100) as f32 / 100.0;
        }

        0.5 // neutral
    }
}
