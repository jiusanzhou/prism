//! Filter: circuit breaker — eliminates candidates that have tripped.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Circuit breaker plugin.
///
/// Tracks per-variant failure counts and eliminates candidates whose
/// failure count meets or exceeds the threshold.
pub struct CircuitBreakerPlugin {
    state: Mutex<HashMap<String, u32>>,
    threshold: u32,
}

impl CircuitBreakerPlugin {
    pub fn new(threshold: u32) -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
            threshold,
        }
    }

    /// Record a failure for a variant.
    pub fn record_failure(&self, variant_id: &str) {
        let mut s = self.state.lock().unwrap();
        *s.entry(variant_id.to_string()).or_insert(0) += 1;
    }

    /// Reset the failure counter for a variant.
    pub fn reset(&self, variant_id: &str) {
        let mut s = self.state.lock().unwrap();
        s.remove(variant_id);
    }

    /// Get current failure count for a variant.
    pub fn failure_count(&self, variant_id: &str) -> u32 {
        let s = self.state.lock().unwrap();
        s.get(variant_id).copied().unwrap_or(0)
    }
}

impl RoutingPlugin for CircuitBreakerPlugin {
    fn name(&self) -> &str {
        "circuit_breaker"
    }

    fn filter(&self, _state: &PluginState, candidate: &ModelCandidate) -> bool {
        let s = self.state.lock().unwrap();
        let count = s.get(&candidate.variant.id).copied().unwrap_or(0);
        count < self.threshold
    }
}
