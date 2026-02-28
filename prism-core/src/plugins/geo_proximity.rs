//! Score: geographic proximity scoring.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Geographic proximity plugin.
///
/// Scores 1.0 when the endpoint region matches the requested region,
/// 0.0 otherwise.
pub struct GeoProximityPlugin;

impl RoutingPlugin for GeoProximityPlugin {
    fn name(&self) -> &str {
        "geo_proximity"
    }

    fn score(&self, state: &PluginState, candidate: &ModelCandidate) -> f32 {
        if let Some(ref req_region) = state.metadata.region {
            if let Some(ref ep_region) = candidate.endpoint.region {
                if ep_region == req_region {
                    return 1.0;
                }
            }
        }
        0.0
    }
}
