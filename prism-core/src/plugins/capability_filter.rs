//! Filter: eliminate candidates that lack required capabilities.

use crate::analyzer::ContextSize;
use crate::plugin::{PluginState, RoutingPlugin};
use crate::strategy::ModelSelectionStrategy;
use crate::types::ModelCandidate;

/// Capability filter plugin.
///
/// Checks vision, reasoning, context window, function calling, and json mode
/// against the request profile and strategy's required_capabilities.
pub struct CapabilityFilterPlugin;

impl RoutingPlugin for CapabilityFilterPlugin {
    fn name(&self) -> &str {
        "capability_filter"
    }

    fn filter(&self, state: &PluginState, candidate: &ModelCandidate) -> bool {
        let caps = &candidate.model.capabilities;

        // Check profile-driven requirements
        if state.profile.requires_vision && !caps.supports_vision {
            return false;
        }
        if state.profile.requires_reasoning && !caps.supports_reasoning {
            return false;
        }

        // Context window check
        let min_ctx = match state.profile.context_window_need {
            ContextSize::Small => 0,
            ContextSize::Medium => 4000,
            ContextSize::Large => 32000,
            ContextSize::XLarge => 128000,
        };
        if caps.max_context_window < min_ctx {
            return false;
        }

        // Strategy-driven required_capabilities
        if let ModelSelectionStrategy::Auto {
            required_capabilities: Some(ref req),
            ..
        } = state.strategy
        {
            if let Some(min_cw) = req.min_context_window {
                if caps.max_context_window < min_cw {
                    return false;
                }
            }
            if let Some(true) = req.function_calling {
                if !caps.supports_function_calling {
                    return false;
                }
            }
            if let Some(true) = req.vision {
                if !caps.supports_vision {
                    return false;
                }
            }
            if let Some(true) = req.json_mode {
                if !caps.supports_json_mode {
                    return false;
                }
            }
            if let Some(true) = req.reasoning {
                if !caps.supports_reasoning {
                    return false;
                }
            }
        }

        true
    }
}
