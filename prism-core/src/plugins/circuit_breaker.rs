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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{ContextSize, Language, RequestProfile};
    use crate::plugin::PluginState;
    use crate::router::RouteMetadata;
    use crate::strategy::ModelSelectionStrategy;
    use crate::types::*;
    use std::collections::HashMap;

    fn make_state() -> PluginState {
        PluginState::new(
            RequestProfile {
                complexity: 0.3,
                task_type: TaskType::SimpleQA,
                estimated_tokens: 100,
                requires_reasoning: false,
                requires_vision: false,
                language: Language::English,
                is_multi_turn: false,
                context_window_need: ContextSize::Small,
            },
            RouteMetadata::default(),
            ModelSelectionStrategy::default(),
            None,
        )
    }

    fn make_candidate(variant_id: &str) -> ModelCandidate {
        ModelCandidate {
            variant: ModelVariant {
                id: variant_id.to_string(),
                model_id: "model-1".into(),
                endpoint_id: "ep-1".into(),
                account_id: None,
                deployment_name: None,
                weight: 1.0,
                performance: ModelPerformance::default(),
                pricing_override: None,
                status: VariantStatus::Active,
                base_url: None,
                upstream_model_name: None,
            metadata: std::collections::HashMap::new(),
            },
            model: Model {
                id: "model-1".into(),
                display_name: "Model 1".into(),
                quality_tier: QualityTier::A,
                capabilities: ModelCapabilities::default(),
                pricing: ModelPricing::default(),
                specialties: vec![],
                limits: None,
                status: ModelStatus::Available,
            },
            account: Account {
                id: "acc-1".into(),
                endpoint_id: "ep-1".into(),
                auth: AuthConfig::default(),
                project_id: None,
                rate_limit: None,
                billing_tag: None,
                weight: 1.0,
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
            },
            endpoint: Endpoint {
                id: "ep-1".into(),
                provider_id: "prov-1".into(),
                base_url: None,
                region: None,
                auth_override: None,
                rate_limit: None,
                priority: 0,
                status: HealthStatus::Healthy,
            },
            provider: Provider {
                id: "prov-1".into(),
                name: "Provider 1".into(),
                auth: AuthConfig::default(),
                base_url: "https://api.example.com".into(),
                rate_limit: None,
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
                protocols: vec![],
            },
            effective_pricing: ModelPricing::default(),
        }
    }

    #[test]
    fn test_filter_open() {
        let plugin = CircuitBreakerPlugin::new(3);
        plugin.record_failure("v-1");
        plugin.record_failure("v-1");
        plugin.record_failure("v-1");
        let state = make_state();
        let candidate = make_candidate("v-1");
        assert!(!plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_filter_closed() {
        let plugin = CircuitBreakerPlugin::new(3);
        plugin.record_failure("v-1");
        plugin.record_failure("v-1");
        let state = make_state();
        let candidate = make_candidate("v-1");
        assert!(plugin.filter(&state, &candidate));
    }

    #[test]
    fn test_reset_clears_counter() {
        let plugin = CircuitBreakerPlugin::new(3);
        plugin.record_failure("v-1");
        plugin.record_failure("v-1");
        plugin.record_failure("v-1");
        assert_eq!(plugin.failure_count("v-1"), 3);
        plugin.reset("v-1");
        assert_eq!(plugin.failure_count("v-1"), 0);
        let state = make_state();
        let candidate = make_candidate("v-1");
        assert!(plugin.filter(&state, &candidate));
    }
}
