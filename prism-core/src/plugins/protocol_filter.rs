//! Filter: eliminate candidates that do not support the required protocol.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Protocol filter plugin.
///
/// Eliminates candidates whose provider does not list the required protocol.
/// If the provider's protocol list is empty, it is assumed to support all protocols.
pub struct ProtocolFilterPlugin;

impl RoutingPlugin for ProtocolFilterPlugin {
    fn name(&self) -> &str {
        "protocol_filter"
    }

    fn filter(&self, state: &PluginState, candidate: &ModelCandidate) -> bool {
        match &state.required_protocol {
            Some(protocol) => {
                candidate.provider.protocols.is_empty()
                    || candidate.provider.protocols.contains(protocol)
            }
            None => true,
        }
    }
}
