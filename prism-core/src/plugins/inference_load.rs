//! Filter + Score: inference-engine load-aware scheduling.
//!
//! Filters out Pods whose KV cache is nearly full and scores the remaining
//! candidates based on GPU utilization, KV cache headroom, queue depth, and
//! running batch size.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Inference load plugin.
///
/// Eliminates candidates with critically high KV cache usage (>0.95) and
/// scores survivors by a weighted combination of inference-engine metrics.
pub struct InferenceLoadPlugin;

impl InferenceLoadPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for InferenceLoadPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl RoutingPlugin for InferenceLoadPlugin {
    fn name(&self) -> &str {
        "inference_load"
    }

    fn filter(&self, _state: &PluginState, candidate: &ModelCandidate) -> bool {
        // Filter out Pods whose KV cache is nearly full (>0.95)
        candidate.variant.performance.kv_cache_usage <= 0.95
    }

    fn score(&self, _state: &PluginState, candidate: &ModelCandidate) -> f32 {
        let perf = &candidate.variant.performance;

        // If no inference-engine metrics are populated (all default zeros),
        // return neutral score so we don't interfere with external API routing.
        let has_inference_metrics = perf.kv_cache_usage > 0.0
            || perf.gpu_utilization > 0.0
            || perf.running_requests > 0
            || perf.waiting_requests > 0;

        if !has_inference_metrics {
            return 0.0; // no inference metrics — zero contribution
        }

        // 1. KV Cache remaining capacity (weight 0.3)
        let kv_score = 1.0 - perf.kv_cache_usage;

        // 2. Waiting requests — fewer is better (weight 0.3)
        //    Normalize: waiting > 10 → score 0
        let wait_score = 1.0 - (perf.waiting_requests as f32 / 10.0).min(1.0);

        // 3. GPU utilization — too high is bad (weight 0.2)
        let gpu_score = 1.0 - perf.gpu_utilization;

        // 4. Running requests — fewer is better (weight 0.2)
        //    Normalize: running > 32 → score 0
        let run_score = 1.0 - (perf.running_requests as f32 / 32.0).min(1.0);

        kv_score * 0.3 + wait_score * 0.3 + gpu_score * 0.2 + run_score * 0.2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::PluginState;
    use crate::types::*;

    fn make_state() -> PluginState {
        use crate::analyzer::{ContextSize, Language, RequestProfile};
        use crate::router::RouteMetadata;
        use crate::strategy::ModelSelectionStrategy;

        PluginState::new(
            RequestProfile {
                complexity: 0.3,
                task_type: crate::types::TaskType::SimpleQA,
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

    fn make_candidate_with_perf(variant_id: &str, perf: ModelPerformance) -> ModelCandidate {
        ModelCandidate {
            variant: ModelVariant {
                id: variant_id.to_string(),
                model_id: "model-1".into(),
                endpoint_id: "ep-1".into(),
                account_id: None,
                deployment_name: None,
                weight: 1.0,
                performance: perf,
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
                metadata: Default::default(),
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
                metadata: Default::default(),
                protocols: vec![],
            },
            effective_pricing: ModelPricing::default(),
        }
    }

    #[test]
    fn filter_rejects_high_kv_cache_usage() {
        let plugin = InferenceLoadPlugin::new();
        let state = make_state();

        let perf = ModelPerformance {
            kv_cache_usage: 0.96,
            ..Default::default()
        };
        let candidate = make_candidate_with_perf("pod-full", perf);

        assert!(
            !plugin.filter(&state, &candidate),
            "should filter out candidate with kv_cache_usage > 0.95"
        );
    }

    #[test]
    fn filter_keeps_kv_cache_at_threshold() {
        let plugin = InferenceLoadPlugin::new();
        let state = make_state();

        let perf = ModelPerformance {
            kv_cache_usage: 0.95,
            ..Default::default()
        };
        let candidate = make_candidate_with_perf("pod-threshold", perf);

        assert!(
            plugin.filter(&state, &candidate),
            "should keep candidate with kv_cache_usage == 0.95"
        );
    }

    #[test]
    fn idle_candidates_score_neutral() {
        let plugin = InferenceLoadPlugin::new();
        let state = make_state();

        // All metrics at 0 (default) → no inference metrics → zero contribution
        let candidate = make_candidate_with_perf("pod-idle", ModelPerformance::default());
        let score = plugin.score(&state, &candidate);

        assert!(
            (score - 0.0).abs() < f32::EPSILON,
            "candidate without inference metrics should score 0.0, got {score}"
        );
    }

    #[test]
    fn high_load_scores_low() {
        let plugin = InferenceLoadPlugin::new();
        let state = make_state();

        let perf = ModelPerformance {
            kv_cache_usage: 0.9,
            waiting_requests: 10,
            gpu_utilization: 0.95,
            running_requests: 32,
            ..Default::default()
        };

        let candidate = make_candidate_with_perf("pod-busy", perf);
        let score = plugin.score(&state, &candidate);

        assert!(
            score < 0.15,
            "heavily loaded candidate should score low, got {score}"
        );
    }

    #[test]
    fn mixed_load_correct_ordering() {
        let plugin = InferenceLoadPlugin::new();
        let state = make_state();

        // Light load
        let light = ModelPerformance {
            kv_cache_usage: 0.2,
            waiting_requests: 1,
            gpu_utilization: 0.3,
            running_requests: 4,
            ..Default::default()
        };

        // Medium load
        let medium = ModelPerformance {
            kv_cache_usage: 0.6,
            waiting_requests: 5,
            gpu_utilization: 0.7,
            running_requests: 16,
            ..Default::default()
        };

        // Heavy load
        let heavy = ModelPerformance {
            kv_cache_usage: 0.9,
            waiting_requests: 9,
            gpu_utilization: 0.9,
            running_requests: 30,
            ..Default::default()
        };

        let light_c = make_candidate_with_perf("pod-light", light);
        let medium_c = make_candidate_with_perf("pod-medium", medium);
        let heavy_c = make_candidate_with_perf("pod-heavy", heavy);

        let s_light = plugin.score(&state, &light_c);
        let s_medium = plugin.score(&state, &medium_c);
        let s_heavy = plugin.score(&state, &heavy_c);

        assert!(
            s_light > s_medium,
            "light ({s_light}) should score higher than medium ({s_medium})"
        );
        assert!(
            s_medium > s_heavy,
            "medium ({s_medium}) should score higher than heavy ({s_heavy})"
        );
    }

    #[test]
    fn gpu_utilization_affects_score() {
        let plugin = InferenceLoadPlugin::new();
        let state = make_state();

        // Only GPU utilization differs
        let low_gpu = ModelPerformance {
            gpu_utilization: 0.1,
            ..Default::default()
        };

        let high_gpu = ModelPerformance {
            gpu_utilization: 0.9,
            ..Default::default()
        };

        let low = make_candidate_with_perf("pod-low-gpu", low_gpu);
        let high = make_candidate_with_perf("pod-high-gpu", high_gpu);

        let s_low = plugin.score(&state, &low);
        let s_high = plugin.score(&state, &high);

        assert!(
            s_low > s_high,
            "low GPU utilization ({s_low}) should score higher than high ({s_high})"
        );
    }
}
