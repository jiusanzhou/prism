//! Scorer: content-aware routing for multi-modal requests.

use crate::plugin::{PluginState, RoutingPlugin};
use crate::types::{ContentHint, ModelCandidate};

/// Content-aware scoring plugin.
///
/// Uses [`ContentHint`] (stored in `PluginState` under key `"content_hint"`)
/// to make smarter routing decisions for multi-modal payloads:
///
/// - **Large audio** → prefer variants with higher throughput.
/// - **High-res images** → prefer variants with larger context windows (proxy for resolution support).
/// - **Long text** → prefer variants with large context windows.
/// - **Video** → prefer variants with high throughput and capacity.
pub struct ContentAwarePlugin;

impl RoutingPlugin for ContentAwarePlugin {
    fn name(&self) -> &str {
        "content_aware"
    }

    fn score(&self, state: &PluginState, candidate: &ModelCandidate) -> f32 {
        let hint = match state.get::<ContentHint>("content_hint") {
            Some(h) => h,
            None => return 0.5, // neutral score when no hint
        };

        match hint {
            ContentHint::Text { char_count } => {
                // Prefer models with larger context windows for long text
                let ctx = candidate.model.capabilities.max_context_window as f32;
                let need = (*char_count as f32) / 4.0; // rough char→token
                if ctx < need {
                    0.1 // penalize insufficient context
                } else {
                    // Score higher when context window comfortably fits the text
                    let headroom = (ctx - need) / ctx;
                    0.5 + headroom * 0.4
                }
            }
            ContentHint::Audio {
                size_bytes,
                duration_secs,
                ..
            } => {
                // Prefer higher throughput for large audio files
                let throughput = candidate.variant.performance.throughput_tps;
                let size_factor = (*size_bytes as f32) / (10.0 * 1024.0 * 1024.0); // normalize to 10MB
                let duration_factor = duration_secs.unwrap_or(0.0) / 60.0; // normalize to 1 min

                let load = size_factor.max(duration_factor).min(1.0);
                if throughput > 0.0 {
                    let tps_score = (throughput / 100.0).min(1.0); // normalize
                    0.5 + tps_score * 0.3 * load
                } else {
                    0.5 // no throughput info → neutral
                }
            }
            ContentHint::Image {
                width,
                height,
                size_bytes,
            } => {
                // Prefer vision-capable models; score by resolution support
                if !candidate.model.capabilities.supports_vision {
                    return 0.2; // penalize non-vision models
                }
                let pixels = width.unwrap_or(0) as u64 * height.unwrap_or(0) as u64;
                let _size = *size_bytes;
                // Higher resolution → prefer higher-capacity models
                let res_factor = (pixels as f32 / (4096.0 * 4096.0)).min(1.0);
                0.6 + res_factor * 0.3
            }
            ContentHint::Video {
                duration_secs,
                size_bytes,
            } => {
                // Video is expensive; prefer high-throughput, low-load variants
                let throughput = candidate.variant.performance.throughput_tps;
                let load_factor = duration_secs.unwrap_or(0.0) / 120.0
                    + size_bytes.unwrap_or(0) as f32 / (100.0 * 1024.0 * 1024.0);
                let load_factor = load_factor.min(1.0);

                if throughput > 0.0 {
                    let tps_score = (throughput / 100.0).min(1.0);
                    0.5 + tps_score * 0.4 * load_factor
                } else {
                    0.5
                }
            }
            ContentHint::None => 0.5,
        }
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

    fn make_state(hint: Option<ContentHint>) -> PluginState {
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
        if let Some(h) = hint {
            state.set("content_hint", h);
        }
        state
    }

    fn make_candidate(ctx_window: u32, vision: bool, throughput: f32) -> ModelCandidate {
        ModelCandidate {
            variant: ModelVariant {
                id: "v-1".to_string(),
                model_id: "model-1".into(),
                endpoint_id: "ep-1".into(),
                account_id: None,
                deployment_name: None,
                weight: 1.0,
                performance: ModelPerformance {
                    throughput_tps: throughput,
                    ..Default::default()
                },
                pricing_override: None,
                status: VariantStatus::Active,
                base_url: None,
                upstream_model_name: None,
                metadata: HashMap::new(),
            },
            model: Model {
                id: "model-1".into(),
                display_name: "Model 1".into(),
                quality_tier: QualityTier::A,
                capabilities: ModelCapabilities {
                    max_context_window: ctx_window,
                    supports_vision: vision,
                    ..Default::default()
                },
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
    fn test_no_hint_neutral_score() {
        let plugin = ContentAwarePlugin;
        let state = make_state(None);
        let candidate = make_candidate(128_000, false, 0.0);
        let score = plugin.score(&state, &candidate);
        assert!((score - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_text_large_context_preferred() {
        let plugin = ContentAwarePlugin;
        let state = make_state(Some(ContentHint::Text {
            char_count: 100_000,
        }));

        let big_ctx = make_candidate(128_000, false, 0.0);
        let small_ctx = make_candidate(4_000, false, 0.0);

        let score_big = plugin.score(&state, &big_ctx);
        let score_small = plugin.score(&state, &small_ctx);

        assert!(
            score_big > score_small,
            "big context ({score_big}) should score higher than small ({score_small})"
        );
    }

    #[test]
    fn test_text_insufficient_context_penalized() {
        let plugin = ContentAwarePlugin;
        // ~250k tokens needed (1M chars / 4)
        let state = make_state(Some(ContentHint::Text {
            char_count: 1_000_000,
        }));
        let candidate = make_candidate(4_000, false, 0.0);
        let score = plugin.score(&state, &candidate);
        assert!(
            score < 0.3,
            "insufficient context should be penalized: {score}"
        );
    }

    #[test]
    fn test_image_vision_preferred() {
        let plugin = ContentAwarePlugin;
        let state = make_state(Some(ContentHint::Image {
            width: Some(1024),
            height: Some(1024),
            size_bytes: 500_000,
        }));

        let vision = make_candidate(128_000, true, 0.0);
        let no_vision = make_candidate(128_000, false, 0.0);

        let score_v = plugin.score(&state, &vision);
        let score_nv = plugin.score(&state, &no_vision);

        assert!(
            score_v > score_nv,
            "vision model ({score_v}) should score higher than non-vision ({score_nv})"
        );
    }

    #[test]
    fn test_audio_high_throughput_preferred() {
        let plugin = ContentAwarePlugin;
        let state = make_state(Some(ContentHint::Audio {
            duration_secs: Some(120.0),
            format: Some("mp3".into()),
            size_bytes: 20 * 1024 * 1024,
        }));

        let high_tp = make_candidate(128_000, false, 200.0);
        let low_tp = make_candidate(128_000, false, 10.0);

        let score_high = plugin.score(&state, &high_tp);
        let score_low = plugin.score(&state, &low_tp);

        assert!(
            score_high > score_low,
            "high throughput ({score_high}) should score higher than low ({score_low})"
        );
    }

    #[test]
    fn test_content_hint_none_neutral() {
        let plugin = ContentAwarePlugin;
        let state = make_state(Some(ContentHint::None));
        let candidate = make_candidate(128_000, false, 0.0);
        let score = plugin.score(&state, &candidate);
        assert!((score - 0.5).abs() < f32::EPSILON);
    }
}
