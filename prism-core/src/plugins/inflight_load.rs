//! Score plugin: prefer variants with fewer in-flight requests.
//!
//! Reads the in-flight snapshot from PluginState (injected by proxy layer)
//! and scores each candidate inversely proportional to its concurrent load.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;
use std::collections::HashMap;

/// Key used to inject the in-flight snapshot into PluginState.
pub const INFLIGHT_SNAPSHOT_KEY: &str = "inflight_snapshot";

/// Plugin that scores candidates by in-flight request count.
///
/// Score formula: `1.0 / (1.0 + load)` — ranges from 1.0 (idle) to ~0 (heavily loaded).
/// Ties (equal load) get equal scores, letting other plugins break the tie.
pub struct InFlightLoadPlugin;

impl InFlightLoadPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for InFlightLoadPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl RoutingPlugin for InFlightLoadPlugin {
    fn name(&self) -> &str {
        "inflight_load"
    }

    fn score(&self, state: &PluginState, candidate: &ModelCandidate) -> f32 {
        let snapshot = match state.get::<HashMap<String, u32>>(INFLIGHT_SNAPSHOT_KEY) {
            Some(s) => s,
            None => return 0.0, // no in-flight data — neutral
        };

        let load = snapshot.get(&candidate.variant.id).copied().unwrap_or(0);

        // 1/(1+load): idle=1.0, 1 req=0.5, 2 req=0.33, 10 req=0.09
        1.0 / (1.0 + load as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PluginState;
    use crate::types::*;
    use std::collections::HashMap;

    fn make_state_with_inflight(loads: &[(&str, u32)]) -> PluginState {
        use crate::analyzer::{ContextSize, Language, RequestProfile};
        use crate::router::RouteMetadata;
        use crate::strategy::ModelSelectionStrategy;

        let mut state = PluginState::new(
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
        );

        let snapshot: HashMap<String, u32> =
            loads.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        state.set(INFLIGHT_SNAPSHOT_KEY, snapshot);
        state
    }

    fn make_candidate(variant_id: &str) -> ModelCandidate {
        ModelCandidate {
            variant: ModelVariant {
                id: variant_id.to_string(),
                model_id: "m1".into(),
                endpoint_id: "ep1".into(),
                account_id: None,
                deployment_name: None,
                weight: 1.0,
                performance: ModelPerformance::default(),
                pricing_override: None,
                status: VariantStatus::Active,
                base_url: None,
                upstream_model_name: None,
                metadata: HashMap::new(),
            },
            model: Model {
                id: "m1".into(),
                display_name: "M1".into(),
                quality_tier: QualityTier::B,
                capabilities: ModelCapabilities::default(),
                pricing: ModelPricing::default(),
                specialties: vec![],
                limits: None,
                status: ModelStatus::Available,
            },
            account: Account {
                id: "a1".into(),
                endpoint_id: "ep1".into(),
                auth: AuthConfig::default(),
                project_id: None,
                rate_limit: None,
                billing_tag: None,
                weight: 1.0,
                status: HealthStatus::Healthy,
                metadata: Default::default(),
            },
            endpoint: Endpoint {
                id: "ep1".into(),
                provider_id: "p1".into(),
                base_url: None,
                region: None,
                auth_override: None,
                rate_limit: None,
                priority: 0,
                status: HealthStatus::Healthy,
            },
            provider: Provider {
                id: "p1".into(),
                name: "P1".into(),
                auth: AuthConfig::default(),
                base_url: "https://api.example.com".into(),
                rate_limit: None,
                status: HealthStatus::Healthy,
                metadata: Default::default(),
                protocols: vec![],
            },
            effective_pricing: ModelPricing::default(),
        }
    }

    #[test]
    fn idle_variant_scores_highest() {
        let plugin = InFlightLoadPlugin::new();
        let state = make_state_with_inflight(&[("v-idle", 0), ("v-busy", 10)]);

        let idle = make_candidate("v-idle");
        let busy = make_candidate("v-busy");

        let s_idle = plugin.score(&state, &idle);
        let s_busy = plugin.score(&state, &busy);

        assert!(s_idle > s_busy, "idle ({s_idle}) > busy ({s_busy})");
        assert!((s_idle - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn equal_load_equal_score() {
        let plugin = InFlightLoadPlugin::new();
        let state = make_state_with_inflight(&[("v1", 5), ("v2", 5)]);

        let v1 = make_candidate("v1");
        let v2 = make_candidate("v2");

        let s1 = plugin.score(&state, &v1);
        let s2 = plugin.score(&state, &v2);

        assert!((s1 - s2).abs() < f32::EPSILON);
    }

    #[test]
    fn no_snapshot_returns_zero() {
        let plugin = InFlightLoadPlugin::new();
        // state without inflight data
        use crate::analyzer::{ContextSize, Language, RequestProfile};
        use crate::router::RouteMetadata;
        use crate::strategy::ModelSelectionStrategy;
        let state = PluginState::new(
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
        );

        let c = make_candidate("v1");
        assert!((plugin.score(&state, &c) - 0.0).abs() < f32::EPSILON);
    }
}
