//! Score: prefix-cache affinity routing for self-hosted inference engines.
//!
//! Uses consistent hashing to route requests with the same prompt prefix to the
//! same Pod, maximising prefix-cache reuse in engines like vLLM / SGLang.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::analyzer::ChatMessage;
use crate::plugin::{PluginState, RoutingPlugin};
use crate::router::RouterError;
use crate::types::ModelCandidate;

/// Prefix affinity plugin.
///
/// Extracts a prefix from the request messages (system prompt or explicit
/// `prefix_key` in metadata), hashes it, and uses jump consistent hashing
/// to pick a preferred candidate. Candidates with high `prefix_cache_hit_rate`
/// also receive bonus scores.
pub struct PrefixAffinityPlugin;

impl PrefixAffinityPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PrefixAffinityPlugin {
    fn default() -> Self {
        Self::new()
    }
}

/// Jump consistent hash (Lamping & Veach, 2014).
///
/// Maps a 64-bit key to one of `num_buckets` buckets with excellent uniformity
/// and minimal reassignment when `num_buckets` changes.
fn jump_consistent_hash(mut key: u64, num_buckets: u32) -> u32 {
    if num_buckets <= 1 {
        return 0;
    }
    let mut b: i64 = -1;
    let mut j: i64 = 0;
    while j < num_buckets as i64 {
        b = j;
        key = key.wrapping_mul(2862933555777941757).wrapping_add(1);
        j = ((b + 1) as f64 * (((1u64 << 31) as f64) / ((key >> 33) + 1) as f64)) as i64;
    }
    b as u32
}

/// Extract prefix text from messages: use the system prompt if present,
/// otherwise concatenate the first two messages.
fn extract_prefix(messages: &[ChatMessage]) -> String {
    // Try system prompt first
    for msg in messages {
        if msg.role == "system" {
            return msg.content.clone();
        }
    }
    // Fallback: first two messages
    messages
        .iter()
        .take(2)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compute a 64-bit hash of the prefix string.
fn hash_prefix(prefix: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    prefix.hash(&mut hasher);
    hasher.finish()
}

/// Threshold above which we consider a candidate overloaded and skip affinity.
const OVERLOAD_KV_THRESHOLD: f32 = 0.90;
const OVERLOAD_WAITING_THRESHOLD: u32 = 8;

impl RoutingPlugin for PrefixAffinityPlugin {
    fn name(&self) -> &str {
        "prefix_affinity"
    }

    fn pre_filter(
        &self,
        state: &mut PluginState,
        candidates: &[ModelCandidate],
    ) -> Result<(), RouterError> {
        // Only activate for explicit prefix_key or when candidates have base_url
        // (self-hosted pods). External API providers don't benefit from prefix affinity.
        let has_explicit_key = state.metadata.prefix_key.is_some();
        let has_self_hosted = candidates.iter().any(|c| c.variant.base_url.is_some());

        if !has_explicit_key && !has_self_hosted {
            return Ok(());
        }

        // Determine prefix text
        let prefix_text = if let Some(ref key) = state.metadata.prefix_key {
            key.clone()
        } else if let Some(messages) = state.get::<Vec<ChatMessage>>("request_messages") {
            extract_prefix(messages)
        } else {
            String::new()
        };

        if prefix_text.is_empty() {
            // No prefix — nothing to route on
            return Ok(());
        }

        let prefix_hash = hash_prefix(&prefix_text);
        let target_idx = jump_consistent_hash(prefix_hash, candidates.len() as u32) as usize;

        state.set("prefix_hash", prefix_hash);
        state.set("prefix_target_idx", target_idx);
        state.set("prefix_candidate_count", candidates.len());

        // Store the target variant id for score() to use
        if target_idx < candidates.len() {
            state.set(
                "prefix_target_variant_id",
                candidates[target_idx].variant.id.clone(),
            );
        }

        Ok(())
    }

    fn score(&self, state: &PluginState, candidate: &ModelCandidate) -> f32 {
        // Only score when prefix was extracted; otherwise completely neutral
        let target_variant_id = match state.get::<String>("prefix_target_variant_id") {
            Some(id) => id,
            None => return 0.0, // no prefix extracted — zero contribution
        };

        let is_target = candidate.variant.id == *target_variant_id;
        let perf = &candidate.variant.performance;

        // Check if the target is overloaded
        let overloaded = perf.kv_cache_usage > OVERLOAD_KV_THRESHOLD
            || perf.waiting_requests > OVERLOAD_WAITING_THRESHOLD;

        if is_target {
            if overloaded {
                return 0.3; // reduce affinity for overloaded targets
            }
            return 1.0;
        }

        // Not the target — bonus for high prefix cache hit rate
        if perf.prefix_cache_hit_rate > 0.5 {
            return 0.5 + perf.prefix_cache_hit_rate * 0.3;
        }

        0.0
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
                // Self-hosted pod — has base_url so prefix affinity activates
                base_url: Some(format!("http://{variant_id}:8000")),
                upstream_model_name: None,
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

    fn make_candidate_with_perf(variant_id: &str, perf: ModelPerformance) -> ModelCandidate {
        let mut c = make_candidate(variant_id);
        c.variant.performance = perf;
        c
    }

    #[test]
    fn same_prefix_routes_to_same_candidate() {
        let plugin = PrefixAffinityPlugin::new();
        let candidates: Vec<_> = (0..5)
            .map(|i| make_candidate(&format!("pod-{i}")))
            .collect();

        let messages = vec![ChatMessage {
            role: "system".into(),
            content: "You are a helpful assistant.".into(),
        }];

        // Run twice with same prefix
        let mut state1 = make_state();
        state1.set("request_messages", messages.clone());
        plugin.pre_filter(&mut state1, &candidates).unwrap();
        let target1 = state1.get::<usize>("prefix_target_idx").copied();

        let mut state2 = make_state();
        state2.set("request_messages", messages);
        plugin.pre_filter(&mut state2, &candidates).unwrap();
        let target2 = state2.get::<usize>("prefix_target_idx").copied();

        assert_eq!(target1, target2);
        assert!(target1.is_some());
    }

    #[test]
    fn different_prefix_may_route_differently() {
        let plugin = PrefixAffinityPlugin::new();
        let candidates: Vec<_> = (0..10)
            .map(|i| make_candidate(&format!("pod-{i}")))
            .collect();

        let mut state1 = make_state();
        state1.set(
            "request_messages",
            vec![ChatMessage {
                role: "system".into(),
                content: "You are a coding assistant.".into(),
            }],
        );
        plugin.pre_filter(&mut state1, &candidates).unwrap();

        let mut state2 = make_state();
        state2.set(
            "request_messages",
            vec![ChatMessage {
                role: "system".into(),
                content: "You are a creative writer specializing in fiction.".into(),
            }],
        );
        plugin.pre_filter(&mut state2, &candidates).unwrap();

        let t1 = state1.get::<usize>("prefix_target_idx").copied().unwrap();
        let t2 = state2.get::<usize>("prefix_target_idx").copied().unwrap();

        // Both must be valid indices
        assert!(t1 < 10);
        assert!(t2 < 10);
        // With 10 candidates and different prefixes, they are very likely different
        // but we can't guarantee it due to hash collisions.
    }

    #[test]
    fn empty_messages_no_panic_neutral_score() {
        let plugin = PrefixAffinityPlugin::new();
        let candidates = vec![make_candidate("pod-0")];

        let mut state = make_state();
        state.set("request_messages", Vec::<ChatMessage>::new());
        plugin.pre_filter(&mut state, &candidates).unwrap();

        // No prefix extracted → no target stored
        assert!(state.get::<usize>("prefix_target_idx").is_none());

        // Score should be zero (no prefix = no contribution)
        let score = plugin.score(&state, &candidates[0]);
        assert!((score - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn overloaded_pod_reduces_affinity_score() {
        let plugin = PrefixAffinityPlugin::new();

        let overloaded_perf = ModelPerformance {
            kv_cache_usage: 0.95,
            waiting_requests: 10,
            ..Default::default()
        };

        let candidates = vec![make_candidate_with_perf("pod-0", overloaded_perf)];

        let mut state = make_state();
        state.set(
            "request_messages",
            vec![ChatMessage {
                role: "system".into(),
                content: "You are a helpful assistant.".into(),
            }],
        );
        plugin.pre_filter(&mut state, &candidates).unwrap();

        let score = plugin.score(&state, &candidates[0]);
        // Overloaded target should get reduced score (0.3)
        assert!(
            score < 0.5,
            "overloaded pod should have reduced score, got {score}"
        );
    }

    #[test]
    fn single_candidate_gets_max_score() {
        let plugin = PrefixAffinityPlugin::new();
        let candidates = vec![make_candidate("pod-0")];

        let mut state = make_state();
        state.set(
            "request_messages",
            vec![ChatMessage {
                role: "system".into(),
                content: "You are a helpful assistant.".into(),
            }],
        );
        plugin.pre_filter(&mut state, &candidates).unwrap();

        let score = plugin.score(&state, &candidates[0]);
        assert!((score - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn prefix_key_metadata_overrides_messages() {
        let plugin = PrefixAffinityPlugin::new();
        let candidates: Vec<_> = (0..5)
            .map(|i| make_candidate(&format!("pod-{i}")))
            .collect();

        // With explicit prefix_key
        let mut state = make_state();
        state.metadata.prefix_key = Some("custom-prefix-key-abc".into());
        state.set(
            "request_messages",
            vec![ChatMessage {
                role: "system".into(),
                content: "This should be ignored.".into(),
            }],
        );
        plugin.pre_filter(&mut state, &candidates).unwrap();
        let target_with_key = state.get::<usize>("prefix_target_idx").copied();

        // Same prefix_key again
        let mut state2 = make_state();
        state2.metadata.prefix_key = Some("custom-prefix-key-abc".into());
        plugin.pre_filter(&mut state2, &candidates).unwrap();
        let target_with_key2 = state2.get::<usize>("prefix_target_idx").copied();

        assert_eq!(target_with_key, target_with_key2);
        assert!(target_with_key.is_some());
    }

    #[test]
    fn high_cache_hit_rate_gets_bonus() {
        let plugin = PrefixAffinityPlugin::new();

        let high_cache_perf = ModelPerformance {
            prefix_cache_hit_rate: 0.8,
            prefix_cache_blocks: 100,
            ..Default::default()
        };

        let candidates = vec![
            make_candidate("pod-target"),
            make_candidate_with_perf("pod-cached", high_cache_perf),
        ];

        let mut state = make_state();
        state.set(
            "request_messages",
            vec![ChatMessage {
                role: "system".into(),
                content: "You are a helpful assistant.".into(),
            }],
        );
        plugin.pre_filter(&mut state, &candidates).unwrap();

        // Find which one is the target
        let target_idx = state.get::<usize>("prefix_target_idx").copied().unwrap();
        let non_target_idx = if target_idx == 0 { 1 } else { 0 };

        if candidates[non_target_idx].variant.id == "pod-cached" {
            let score = plugin.score(&state, &candidates[non_target_idx]);
            // Should get bonus from high cache hit rate (> 0.0)
            assert!(
                score > 0.0,
                "cached pod should get bonus score, got {score}"
            );
        }
    }
}
