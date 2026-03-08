//! Score: feedback-based quality adjustment.

use std::sync::Arc;

use crate::feedback::FeedbackCollector;
use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::ModelCandidate;

/// Feedback scorer plugin.
///
/// Adjusts scores based on recorded outcome feedback (success rate, user ratings).
/// Returns a value in [0.5, 1.5] mapped to [0.0, 1.0] for the scoring framework.
pub struct FeedbackScorerPlugin {
    feedback: Arc<FeedbackCollector>,
}

impl FeedbackScorerPlugin {
    pub fn new(feedback: Arc<FeedbackCollector>) -> Self {
        Self { feedback }
    }
}

impl RoutingPlugin for FeedbackScorerPlugin {
    fn name(&self) -> &str {
        "feedback_scorer"
    }

    fn score(&self, _state: &PluginState, candidate: &ModelCandidate) -> f32 {
        // quality_adjustment returns [0.5, 1.5] centered at 1.0
        // Map to [0.0, 1.0]: (adj - 0.5) / 1.0 = adj - 0.5
        let adj = self.feedback.quality_adjustment(&candidate.variant.id);
        (adj - 0.5).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{ContextSize, Language, RequestProfile};
    use crate::feedback::{FeedbackCollector, OutcomeReport};
    use crate::plugin::{PluginState, RoutingPlugin};
    use crate::router::RouteMetadata;
    use crate::strategy::ModelSelectionStrategy;
    use crate::types::*;
    use std::collections::HashMap;
    use std::sync::Arc;

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
    fn test_positive_feedback_boost() {
        let feedback = Arc::new(FeedbackCollector::new(100));
        for _ in 0..10 {
            feedback.record(&OutcomeReport {
                variant_id: "v1".into(),
                success: true,
                latency_ms: 100,
                prompt_tokens: Some(50),
                completion_tokens: Some(50),
                cost: Some(0.01),
                user_rating: Some(5),
            });
        }
        let plugin = FeedbackScorerPlugin::new(feedback);
        let state = make_state();
        let candidate = make_candidate("v1");
        let score = plugin.score(&state, &candidate);
        // quality_adjustment should be > 1.0 for positive feedback
        // so score = (adj - 0.5) should be > 0.5
        assert!(
            score > 0.5,
            "positive feedback should produce score > 0.5, got {}",
            score
        );
    }

    #[test]
    fn test_negative_feedback_penalty() {
        let feedback = Arc::new(FeedbackCollector::new(100));
        for _ in 0..10 {
            feedback.record(&OutcomeReport {
                variant_id: "v1".into(),
                success: false,
                latency_ms: 5000,
                prompt_tokens: Some(50),
                completion_tokens: Some(50),
                cost: Some(0.01),
                user_rating: Some(1),
            });
        }
        let plugin = FeedbackScorerPlugin::new(feedback);
        let state = make_state();
        let candidate = make_candidate("v1");
        let score = plugin.score(&state, &candidate);
        // quality_adjustment should be < 1.0 for negative feedback
        // so score = (adj - 0.5) should be < 0.5
        assert!(
            score < 0.5,
            "negative feedback should produce score < 0.5, got {}",
            score
        );
    }

    #[test]
    fn test_no_feedback_neutral() {
        let feedback = Arc::new(FeedbackCollector::new(100));
        let plugin = FeedbackScorerPlugin::new(feedback);
        let state = make_state();
        let candidate = make_candidate("v1");
        let score = plugin.score(&state, &candidate);
        // No data → quality_adjustment returns 1.0
        // score = (1.0 - 0.5) = 0.5
        assert!(
            (score - 0.5).abs() < f32::EPSILON,
            "no feedback should produce score 0.5, got {}",
            score
        );
    }
}
