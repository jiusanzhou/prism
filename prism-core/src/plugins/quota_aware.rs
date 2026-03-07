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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{ContextSize, Language, RequestProfile};
    use crate::plugin::{PluginState, RoutingPlugin};
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

    fn make_candidate_with_quota(rpm: Option<u32>, current_qps: f32) -> ModelCandidate {
        ModelCandidate {
            variant: ModelVariant {
                id: "v1".to_string(),
                model_id: "model-1".into(),
                endpoint_id: "ep-1".into(),
                account_id: None,
                deployment_name: None,
                weight: 1.0,
                performance: ModelPerformance {
                    current_qps,
                    ..Default::default()
                },
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
                rate_limit: rpm.map(|r| RateLimit {
                    rpm: Some(r),
                    tpm: None,
                    concurrent: None,
                }),
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
    fn test_low_utilisation_higher_score() {
        let plugin = QuotaAwarePlugin;
        let state = make_state();
        // rpm=600, rps_limit=10, current_qps=1.0 → utilisation=0.1, score=0.9
        let candidate = make_candidate_with_quota(Some(600), 1.0);
        let score = plugin.score(&state, &candidate);
        assert!(
            (score - 0.9).abs() < 0.01,
            "low utilisation should produce score ~0.9, got {}",
            score
        );
    }

    #[test]
    fn test_no_limit() {
        let plugin = QuotaAwarePlugin;
        let state = make_state();
        let candidate = make_candidate_with_quota(None, 5.0);
        let score = plugin.score(&state, &candidate);
        assert!(
            (score - 1.0).abs() < f32::EPSILON,
            "no rate limit should produce score 1.0, got {}",
            score
        );
    }

    #[test]
    fn test_high_utilisation_low_score() {
        let plugin = QuotaAwarePlugin;
        let state = make_state();
        // rpm=600, rps_limit=10, current_qps=9.0 → utilisation=0.9, score=0.1
        let candidate = make_candidate_with_quota(Some(600), 9.0);
        let score = plugin.score(&state, &candidate);
        assert!(
            (score - 0.1).abs() < 0.01,
            "high utilisation should produce score ~0.1, got {}",
            score
        );
    }

    #[test]
    fn test_full_utilisation() {
        let plugin = QuotaAwarePlugin;
        let state = make_state();
        // rpm=600, rps_limit=10, current_qps=10.0 → utilisation=1.0, score=0.0
        let candidate = make_candidate_with_quota(Some(600), 10.0);
        let score = plugin.score(&state, &candidate);
        assert!(
            score.abs() < 0.01,
            "full utilisation should produce score ~0.0, got {}",
            score
        );
    }

    #[test]
    fn test_over_utilisation_clamped() {
        let plugin = QuotaAwarePlugin;
        let state = make_state();
        // rpm=600, rps_limit=10, current_qps=15.0 → utilisation clamped to 1.0, score=0.0
        let candidate = make_candidate_with_quota(Some(600), 15.0);
        let score = plugin.score(&state, &candidate);
        assert!(
            score.abs() < 0.01,
            "over utilisation should produce score ~0.0, got {}",
            score
        );
    }
}
