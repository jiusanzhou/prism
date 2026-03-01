//! L1–L5 selector traits and built-in implementations.
//!
//! Each layer of the Prism routing stack is abstracted behind a trait so that
//! custom implementations can be plugged in via `PrismRouter::with_l*()`.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rand::Rng;

use crate::analyzer::RequestProfile;
use crate::feedback::FeedbackCollector;
use crate::registry::{InMemoryRegistry, ModelRegistryTrait, RequiredCapabilities};
use crate::router::{RouteMetadata, RouterError};
use crate::strategy::*;
use crate::types::*;

// ════════════════════════════════════════════════
// Shared context
// ════════════════════════════════════════════════

/// Routing context that flows through L1–L5.
pub struct RouteContext<'a> {
    pub profile: &'a RequestProfile,
    pub metadata: &'a RouteMetadata,
    pub strategy: &'a ModelSelectionStrategy,
    pub required_protocol: Option<&'a str>,
}

// ════════════════════════════════════════════════
// L1 — Model Selector
// ════════════════════════════════════════════════

/// Selects and ranks model candidates (L1).
pub trait L1ModelSelector: Send + Sync {
    /// Return indices into `candidates` in descending-score order.
    fn select(
        &self,
        candidates: &[ModelCandidate],
        ctx: &RouteContext,
    ) -> Result<Vec<usize>, RouterError>;

    /// Human-readable name for tracing / reason strings.
    fn name(&self) -> &str;
}

// ──────────────────────────────────────────────
// ScoredL1Selector  (default Auto mode)
// ──────────────────────────────────────────────

/// Default L1 implementation: capability / context filtering + multi-objective scoring.
///
/// Handles `ModelSelectionStrategy::Auto`, `Pinned`, and `Cascade`.
pub struct ScoredL1Selector {
    feedback: Arc<FeedbackCollector>,
}

impl ScoredL1Selector {
    /// Create a new scored selector with a shared feedback collector.
    pub fn new(feedback: Arc<FeedbackCollector>) -> Self {
        Self { feedback }
    }

    // ── scoring helpers (moved from PrismRouter) ──

    pub(crate) fn score_candidate(
        &self,
        objective: &RoutingObjective,
        profile: &RequestProfile,
        candidate: &ModelCandidate,
    ) -> f32 {
        Self::score_candidate_with_feedback(objective, profile, candidate, &self.feedback)
    }

    pub(crate) fn score_candidate_with_feedback(
        objective: &RoutingObjective,
        profile: &RequestProfile,
        candidate: &ModelCandidate,
        feedback: &FeedbackCollector,
    ) -> f32 {
        let base_score = match objective {
            RoutingObjective::Balanced {
                cost_weight,
                quality_weight,
                latency_weight,
            } => {
                let max_cost = 100.0_f64;
                let max_latency = 10000.0_f32;

                let cost_score =
                    1.0 - (candidate.effective_pricing.cost_per_1m() / max_cost).min(1.0);
                let quality_score = Self::estimate_quality(profile, candidate) as f64;
                let latency_score = 1.0
                    - (candidate.variant.performance.p50_latency_ms as f32 / max_latency)
                        .min(1.0);

                (cost_score as f32 * cost_weight)
                    + (quality_score as f32 * quality_weight)
                    + (latency_score * latency_weight)
            }
            RoutingObjective::SloFirst {
                max_latency_ms,
                max_ttft_ms,
                min_quality_score,
            } => {
                if candidate.variant.performance.p99_latency_ms > *max_latency_ms {
                    return 0.0;
                }
                if candidate.variant.performance.ttft_p50_ms > *max_ttft_ms {
                    return 0.0;
                }
                let quality = Self::estimate_quality(profile, candidate);
                if quality < *min_quality_score {
                    return 0.0;
                }
                quality
            }
            RoutingObjective::CostFirst {
                min_quality_score,
                budget_per_request: _,
            } => {
                let quality = Self::estimate_quality(profile, candidate);
                if quality < *min_quality_score {
                    return 0.0;
                }
                let max_cost = 100.0_f64;
                (1.0 - (candidate.effective_pricing.cost_per_1m() / max_cost).min(1.0)) as f32
            }
            RoutingObjective::QualityFirst { max_latency_ms } => {
                if let Some(max_lat) = max_latency_ms {
                    if candidate.variant.performance.p99_latency_ms > *max_lat {
                        return 0.0;
                    }
                }
                Self::estimate_quality(profile, candidate)
            }
            RoutingObjective::Custom { .. } => 0.5,
        };

        let feedback_adj = feedback.quality_adjustment(&candidate.variant.id);
        base_score * feedback_adj
    }

    pub(crate) fn estimate_quality(profile: &RequestProfile, candidate: &ModelCandidate) -> f32 {
        let tier_score: f32 = match candidate.model.quality_tier {
            QualityTier::S => 1.0,
            QualityTier::A => 0.75,
            QualityTier::B => 0.5,
            QualityTier::C => 0.25,
        };

        let specialty_bonus: f32 =
            if candidate.model.specialties.contains(&profile.task_type) {
                0.15
            } else {
                0.0
            };

        (tier_score + specialty_bonus).min(1.0_f32)
    }
}

impl L1ModelSelector for ScoredL1Selector {
    fn select(
        &self,
        candidates: &[ModelCandidate],
        ctx: &RouteContext,
    ) -> Result<Vec<usize>, RouterError> {
        match ctx.strategy {
            ModelSelectionStrategy::Pinned { model_id } => {
                let matched: Vec<usize> = candidates
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.model.id == *model_id)
                    .map(|(i, _)| i)
                    .collect();
                if matched.is_empty() {
                    Err(RouterError::AllFiltered {
                        reason: format!("pinned model '{model_id}' not found"),
                    })
                } else {
                    Ok(matched)
                }
            }
            ModelSelectionStrategy::Auto {
                objective,
                required_capabilities,
                ..
            } => {
                let mut indices: Vec<usize> = (0..candidates.len()).collect();

                // Capability filter
                if let Some(req) = required_capabilities {
                    let req_caps = RequiredCapabilities {
                        min_context_window: req.min_context_window,
                        function_calling: req.function_calling,
                        vision: req.vision,
                        json_mode: req.json_mode,
                        reasoning: req.reasoning,
                    };
                    indices.retain(|&i| req_caps.matches(&candidates[i].model.capabilities));
                }

                if ctx.profile.requires_vision {
                    indices.retain(|&i| candidates[i].model.capabilities.supports_vision);
                }

                if ctx.profile.requires_reasoning {
                    indices.retain(|&i| candidates[i].model.capabilities.supports_reasoning);
                }

                let min_ctx = match ctx.profile.context_window_need {
                    crate::analyzer::ContextSize::Small => 0,
                    crate::analyzer::ContextSize::Medium => 4000,
                    crate::analyzer::ContextSize::Large => 32000,
                    crate::analyzer::ContextSize::XLarge => 128000,
                };
                indices
                    .retain(|&i| candidates[i].model.capabilities.max_context_window >= min_ctx);

                if indices.is_empty() {
                    return Err(RouterError::AllFiltered {
                        reason: "no candidates satisfy capability / context constraints".into(),
                    });
                }

                let mut scored: Vec<(usize, f32)> = indices
                    .into_iter()
                    .map(|i| {
                        let score =
                            self.score_candidate(objective, ctx.profile, &candidates[i]);
                        (i, score)
                    })
                    .collect();
                scored
                    .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                scored.retain(|(_, s)| *s > 0.0);

                if scored.is_empty() {
                    return Err(RouterError::AllFiltered {
                        reason: "all candidates scored 0".into(),
                    });
                }

                Ok(scored.into_iter().map(|(i, _)| i).collect())
            }
            ModelSelectionStrategy::Cascade { .. } => {
                Ok((0..candidates.len()).collect())
            }
        }
    }

    fn name(&self) -> &str {
        "ScoredL1Selector"
    }
}

// ════════════════════════════════════════════════
// L2 — Provider Selector
// ════════════════════════════════════════════════

/// Re-ranks candidates at the provider level (L2).
pub trait L2ProviderSelector: Send + Sync {
    fn rerank<'a>(
        &self,
        candidates: Vec<&'a ModelCandidate>,
        ctx: &RouteContext,
    ) -> Vec<&'a ModelCandidate>;

    fn name(&self) -> &str;
}

// ──────────────────────────────────────────────
// DefaultL2Selector  (geo-proximity + quota-aware)
// ──────────────────────────────────────────────

/// Default L2: geo-proximity boost + quota utilisation penalty.
pub struct DefaultL2Selector;

impl L2ProviderSelector for DefaultL2Selector {
    fn rerank<'a>(
        &self,
        candidates: Vec<&'a ModelCandidate>,
        ctx: &RouteContext,
    ) -> Vec<&'a ModelCandidate> {
        if candidates.is_empty() {
            return candidates;
        }

        let mut scored: Vec<(&ModelCandidate, f32)> = candidates
            .into_iter()
            .map(|c| {
                let mut bonus = 0.0_f32;

                // Geo-proximity
                if let Some(ref req_region) = ctx.metadata.region {
                    if let Some(ref ep_region) = c.endpoint.region {
                        if ep_region == req_region {
                            bonus += 100.0;
                        }
                    }
                }

                // Quota-awareness
                let effective_rpm = c
                    .account
                    .rate_limit
                    .as_ref()
                    .and_then(|rl| rl.rpm)
                    .or_else(|| c.endpoint.rate_limit.as_ref().and_then(|rl| rl.rpm))
                    .or_else(|| c.provider.rate_limit.as_ref().and_then(|rl| rl.rpm));

                if let Some(rpm) = effective_rpm {
                    if rpm > 0 {
                        let rps_limit = rpm as f32 / 60.0;
                        let utilisation =
                            (c.variant.performance.current_qps / rps_limit).min(1.0);
                        bonus -= utilisation * 200.0;
                    }
                }

                (c, bonus)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|(c, _)| c).collect()
    }

    fn name(&self) -> &str {
        "DefaultL2Selector"
    }
}

// ──────────────────────────────────────────────
// LowestLatencyL2Selector
// ──────────────────────────────────────────────

/// L2: re-rank candidates by p50 latency ascending (lowest latency first).
pub struct LowestLatencyL2Selector;

impl L2ProviderSelector for LowestLatencyL2Selector {
    fn rerank<'a>(
        &self,
        mut candidates: Vec<&'a ModelCandidate>,
        _ctx: &RouteContext,
    ) -> Vec<&'a ModelCandidate> {
        candidates.sort_by_key(|c| c.variant.performance.p50_latency_ms);
        candidates
    }

    fn name(&self) -> &str {
        "LowestLatencyL2Selector"
    }
}

// ──────────────────────────────────────────────
// LowestCostL2Selector
// ──────────────────────────────────────────────

/// L2: re-rank candidates by effective pricing cost ascending (cheapest first).
pub struct LowestCostL2Selector;

impl L2ProviderSelector for LowestCostL2Selector {
    fn rerank<'a>(
        &self,
        mut candidates: Vec<&'a ModelCandidate>,
        _ctx: &RouteContext,
    ) -> Vec<&'a ModelCandidate> {
        candidates.sort_by(|a, b| {
            a.effective_pricing
                .cost_per_1m()
                .partial_cmp(&b.effective_pricing.cost_per_1m())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates
    }

    fn name(&self) -> &str {
        "LowestCostL2Selector"
    }
}

// ──────────────────────────────────────────────
// PriorityL2Selector
// ──────────────────────────────────────────────

/// L2: re-rank candidates by endpoint priority ascending (lower number = higher priority).
pub struct PriorityL2Selector;

impl L2ProviderSelector for PriorityL2Selector {
    fn rerank<'a>(
        &self,
        mut candidates: Vec<&'a ModelCandidate>,
        _ctx: &RouteContext,
    ) -> Vec<&'a ModelCandidate> {
        candidates.sort_by_key(|c| c.endpoint.priority);
        candidates
    }

    fn name(&self) -> &str {
        "PriorityL2Selector"
    }
}

// ════════════════════════════════════════════════
// L3 — Account Selector
// ════════════════════════════════════════════════

/// Selects an account-level candidate (L3).
pub trait L3AccountSelector: Send + Sync {
    /// Returns the index into `candidates` of the chosen account.
    fn select(
        &self,
        candidates: &[&ModelCandidate],
        ctx: &RouteContext,
    ) -> Result<usize, RouterError>;

    fn name(&self) -> &str;
}

// ──────────────────────────────────────────────
// AffinityL3Selector  (affinity hash + cost isolation + soft migration)
// ──────────────────────────────────────────────

/// Default L3: tenant cost-isolation → affinity hash → soft migration on overload.
pub struct AffinityL3Selector;

impl AffinityL3Selector {
    fn is_account_overloaded(candidate: &ModelCandidate) -> bool {
        if let Some(ref rl) = candidate.account.rate_limit {
            if let Some(rpm) = rl.rpm {
                if rpm > 0 {
                    let rps_limit = rpm as f32 / 60.0;
                    let utilisation = candidate.variant.performance.current_qps / rps_limit;
                    return utilisation >= 0.9;
                }
            }
        }
        false
    }
}

impl L3AccountSelector for AffinityL3Selector {
    fn select(
        &self,
        candidates: &[&ModelCandidate],
        ctx: &RouteContext,
    ) -> Result<usize, RouterError> {
        if candidates.is_empty() {
            return Err(RouterError::NoCandidates);
        }

        // Cost isolation
        if let Some(ref tenant_id) = ctx.metadata.tenant_id {
            for (i, c) in candidates.iter().enumerate() {
                if let Some(ref tag) = c.account.billing_tag {
                    if tag == tenant_id {
                        return Ok(i);
                    }
                }
            }
        }

        // Affinity routing
        if let Some(ref affinity_key) = ctx.metadata.affinity_key {
            let mut hasher = DefaultHasher::new();
            affinity_key.hash(&mut hasher);
            let hash_val = hasher.finish();

            let mut seen_accounts: Vec<(usize, &str)> = Vec::new();
            for (i, c) in candidates.iter().enumerate() {
                if !seen_accounts
                    .iter()
                    .any(|(_, aid)| *aid == c.account.id.as_str())
                {
                    seen_accounts.push((i, &c.account.id));
                }
            }

            if !seen_accounts.is_empty() {
                let target_idx = (hash_val as usize) % seen_accounts.len();
                let (candidate_idx, _) = seen_accounts[target_idx];

                // Soft migration
                if Self::is_account_overloaded(candidates[candidate_idx]) {
                    for (i, c) in candidates.iter().enumerate() {
                        if i != candidate_idx && !Self::is_account_overloaded(c) {
                            return Ok(i);
                        }
                    }
                }

                return Ok(candidate_idx);
            }
        }

        Ok(0)
    }

    fn name(&self) -> &str {
        "AffinityL3Selector"
    }
}

// ──────────────────────────────────────────────
// RoundRobinL3Selector
// ──────────────────────────────────────────────

/// L3: round-robin selection using an atomic counter. Thread-safe.
pub struct RoundRobinL3Selector {
    counter: AtomicUsize,
}

impl RoundRobinL3Selector {
    pub fn new() -> Self {
        Self {
            counter: AtomicUsize::new(0),
        }
    }
}

impl Default for RoundRobinL3Selector {
    fn default() -> Self {
        Self::new()
    }
}

impl L3AccountSelector for RoundRobinL3Selector {
    fn select(
        &self,
        candidates: &[&ModelCandidate],
        _ctx: &RouteContext,
    ) -> Result<usize, RouterError> {
        if candidates.is_empty() {
            return Err(RouterError::NoCandidates);
        }
        let idx = self.counter.fetch_add(1, Ordering::Relaxed);
        Ok(idx % candidates.len())
    }

    fn name(&self) -> &str {
        "RoundRobinL3Selector"
    }
}

// ──────────────────────────────────────────────
// LeastLoadL3Selector
// ──────────────────────────────────────────────

/// L3: select the candidate with the lowest current_qps.
pub struct LeastLoadL3Selector;

impl L3AccountSelector for LeastLoadL3Selector {
    fn select(
        &self,
        candidates: &[&ModelCandidate],
        _ctx: &RouteContext,
    ) -> Result<usize, RouterError> {
        if candidates.is_empty() {
            return Err(RouterError::NoCandidates);
        }
        let (min_idx, _) = candidates
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.variant
                    .performance
                    .current_qps
                    .partial_cmp(&b.variant.performance.current_qps)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap(); // safe: candidates is non-empty
        Ok(min_idx)
    }

    fn name(&self) -> &str {
        "LeastLoadL3Selector"
    }
}

// ════════════════════════════════════════════════
// L4 — Variant Selector
// ════════════════════════════════════════════════

/// Selects the final variant from the ranked candidates (L4).
pub trait L4VariantSelector: Send + Sync {
    /// Returns a reference to the chosen candidate.
    fn select<'a>(
        &self,
        candidates: &[&'a ModelCandidate],
        ctx: &RouteContext,
    ) -> &'a ModelCandidate;

    fn name(&self) -> &str;
}

// ──────────────────────────────────────────────
// BestL4Selector  (pick first / default)
// ──────────────────────────────────────────────

/// Default L4: pick the first (best-ranked) candidate.
pub struct BestL4Selector;

impl L4VariantSelector for BestL4Selector {
    fn select<'a>(
        &self,
        candidates: &[&'a ModelCandidate],
        _ctx: &RouteContext,
    ) -> &'a ModelCandidate {
        candidates[0]
    }

    fn name(&self) -> &str {
        "BestL4Selector"
    }
}

// ──────────────────────────────────────────────
// CanaryL4Selector
// ──────────────────────────────────────────────

/// L4: split traffic between canary and stable variants.
pub struct CanaryL4Selector {
    pub canary_variant_id: String,
    pub canary_pct: f32,
}

impl L4VariantSelector for CanaryL4Selector {
    fn select<'a>(
        &self,
        candidates: &[&'a ModelCandidate],
        _ctx: &RouteContext,
    ) -> &'a ModelCandidate {
        let mut rng = rand::thread_rng();
        let roll: f32 = rng.gen();

        if roll < self.canary_pct {
            if let Some(canary) = candidates
                .iter()
                .find(|c| c.variant.id == self.canary_variant_id)
            {
                return canary;
            }
        }
        candidates
            .iter()
            .find(|c| c.variant.id != self.canary_variant_id)
            .unwrap_or(&candidates[0])
    }

    fn name(&self) -> &str {
        "CanaryL4Selector"
    }
}

// ──────────────────────────────────────────────
// WeightedL4Selector
// ──────────────────────────────────────────────

/// L4: weighted random selection among candidates.
pub struct WeightedL4Selector;

impl L4VariantSelector for WeightedL4Selector {
    fn select<'a>(
        &self,
        candidates: &[&'a ModelCandidate],
        _ctx: &RouteContext,
    ) -> &'a ModelCandidate {
        let total_weight: f32 = candidates.iter().map(|c| c.variant.weight).sum();
        if total_weight <= 0.0 {
            return candidates[0];
        }
        let mut rng = rand::thread_rng();
        let mut roll: f32 = rng.gen::<f32>() * total_weight;
        for c in candidates {
            roll -= c.variant.weight;
            if roll <= 0.0 {
                return c;
            }
        }
        candidates.last().unwrap()
    }

    fn name(&self) -> &str {
        "WeightedL4Selector"
    }
}

// ════════════════════════════════════════════════
// L5 — Resilience Policy
// ════════════════════════════════════════════════

/// Failure-recovery and hedging policy (L5).
pub trait L5ResiliencePolicy: Send + Sync {
    /// Decide what to do after a variant failure.
    fn on_failure(
        &self,
        failed_variant_id: &str,
        error_type: &FailureType,
        attempt: u32,
    ) -> Result<RecoveryAction, RouterError>;

    /// Plan hedging: return variant IDs to send requests to in parallel.
    fn plan_hedging(&self, candidates: &[&ModelCandidate], max: u32) -> Vec<String>;

    /// Record a failure for circuit-breaker bookkeeping.
    fn record_failure(&self, variant_id: &str);

    /// Reset failure counter (e.g. on success).
    fn reset_failures(&self, variant_id: &str);

    fn name(&self) -> &str;
}

// ──────────────────────────────────────────────
// DefaultL5Policy
// ──────────────────────────────────────────────

/// Circuit-breaker state (per variant).
struct CircuitBreakerState {
    failure_counts: HashMap<String, u32>,
}

impl CircuitBreakerState {
    fn new() -> Self {
        Self {
            failure_counts: HashMap::new(),
        }
    }
}

/// Default L5: circuit-breaker + retry + fallback chain.
pub struct DefaultL5Policy {
    pub resilience_policy: ResiliencePolicy,
    /// Shared reference to the registry for fallback variant look-up.
    registry: Arc<InMemoryRegistry>,
    circuit_breaker_state: Mutex<CircuitBreakerState>,
}

impl DefaultL5Policy {
    pub fn new(
        resilience_policy: ResiliencePolicy,
        registry: Arc<InMemoryRegistry>,
    ) -> Self {
        Self {
            resilience_policy,
            registry,
            circuit_breaker_state: Mutex::new(CircuitBreakerState::new()),
        }
    }

    fn get_failure_count(&self, variant_id: &str) -> u32 {
        let state = self.circuit_breaker_state.lock().unwrap();
        *state.failure_counts.get(variant_id).unwrap_or(&0)
    }

    fn is_circuit_open(&self, variant_id: &str) -> bool {
        let failure_count = self.get_failure_count(variant_id);
        let cb = &self.resilience_policy.circuit_breaker;
        failure_count >= cb.min_requests
    }

    fn compute_delay(&self, attempt: u32) -> u32 {
        match &self.resilience_policy.retry.backoff {
            BackoffStrategy::Fixed { delay_ms } => *delay_ms,
            BackoffStrategy::Exponential { base_ms, max_ms } => {
                let delay = base_ms * 2u32.saturating_pow(attempt.saturating_sub(1));
                delay.min(*max_ms)
            }
            BackoffStrategy::Jittered { base_ms, max_ms } => {
                let delay = base_ms * 2u32.saturating_pow(attempt.saturating_sub(1));
                delay.min(*max_ms)
            }
        }
    }

    fn find_fallback_variant(
        &self,
        failed_variant_id: &str,
        failed_provider_id: Option<&str>,
        cross_provider: bool,
    ) -> Option<String> {
        let candidates = self.registry.get_all_candidates();

        if cross_provider {
            if let Some(failed_prov) = failed_provider_id {
                for c in &candidates {
                    if c.variant.id != failed_variant_id && c.provider.id != failed_prov {
                        return Some(c.variant.id.clone());
                    }
                }
            }
        }

        for c in &candidates {
            if c.variant.id != failed_variant_id {
                return Some(c.variant.id.clone());
            }
        }

        None
    }

    fn get_provider_for_variant(&self, variant_id: &str) -> Option<String> {
        let variant = self.registry.get_variant(variant_id)?;
        let endpoint = self.registry.endpoints.get(&variant.endpoint_id)?;
        Some(endpoint.provider_id.clone())
    }
}

impl L5ResiliencePolicy for DefaultL5Policy {
    fn on_failure(
        &self,
        failed_variant_id: &str,
        error_type: &FailureType,
        attempt: u32,
    ) -> Result<RecoveryAction, RouterError> {
        let policy = &self.resilience_policy;

        // Record this failure for circuit breaker tracking
        self.record_failure(failed_variant_id);

        // Check circuit breaker first
        if self.is_circuit_open(failed_variant_id) {
            let failed_prov = self.get_provider_for_variant(failed_variant_id);
            let cross_provider = matches!(policy.retry.retry_scope, RetryScope::CrossProvider);
            if let Some(fb_variant) = self.find_fallback_variant(
                failed_variant_id,
                failed_prov.as_deref(),
                cross_provider,
            ) {
                return Ok(RecoveryAction::Fallback {
                    variant_id: fb_variant,
                });
            }
            return Ok(RecoveryAction::Reject {
                reason: "circuit breaker open, no fallback available".into(),
            });
        }

        let should_retry = match error_type {
            FailureType::HttpError(status) => {
                policy.retry.retry_on.iter().any(|cond| match cond {
                    RetryCondition::HttpStatus(statuses) => statuses.contains(status),
                    _ => false,
                })
            }
            FailureType::Timeout => policy
                .retry
                .retry_on
                .iter()
                .any(|c| matches!(c, RetryCondition::Timeout)),
            FailureType::ConnectionError => policy
                .retry
                .retry_on
                .iter()
                .any(|c| matches!(c, RetryCondition::ConnectionError)),
            FailureType::RateLimit => policy
                .retry
                .retry_on
                .iter()
                .any(|c| matches!(c, RetryCondition::RateLimit)),
            FailureType::TTFTTimeout => true,
        };

        if should_retry && attempt <= policy.retry.max_retries {
            let delay = self.compute_delay(attempt);

            match &policy.retry.retry_scope {
                RetryScope::SameVariant | RetryScope::SameAccount | RetryScope::SameProvider => {
                    Ok(RecoveryAction::Retry {
                        variant_id: failed_variant_id.to_string(),
                        delay_ms: delay,
                    })
                }
                RetryScope::CrossProvider => {
                    let failed_prov = self.get_provider_for_variant(failed_variant_id);
                    if let Some(alt_variant) = self.find_fallback_variant(
                        failed_variant_id,
                        failed_prov.as_deref(),
                        true,
                    ) {
                        Ok(RecoveryAction::Retry {
                            variant_id: alt_variant,
                            delay_ms: delay,
                        })
                    } else {
                        Ok(RecoveryAction::Retry {
                            variant_id: failed_variant_id.to_string(),
                            delay_ms: delay,
                        })
                    }
                }
            }
        } else {
            let failed_prov = self.get_provider_for_variant(failed_variant_id);
            let cross_provider = matches!(policy.retry.retry_scope, RetryScope::CrossProvider);

            for target in &policy.fallback.chain {
                match target {
                    FallbackTarget::Variant(vid) => {
                        if vid != failed_variant_id {
                            return Ok(RecoveryAction::Fallback {
                                variant_id: vid.clone(),
                            });
                        }
                    }
                    FallbackTarget::Model(model_id) => {
                        let candidates =
                            self.registry.get_candidates_for_model(model_id);
                        for c in &candidates {
                            if c.variant.id != failed_variant_id {
                                return Ok(RecoveryAction::Fallback {
                                    variant_id: c.variant.id.clone(),
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }

            if let Some(fb_variant) = self.find_fallback_variant(
                failed_variant_id,
                failed_prov.as_deref(),
                cross_provider,
            ) {
                Ok(RecoveryAction::Fallback {
                    variant_id: fb_variant,
                })
            } else {
                Ok(RecoveryAction::Reject {
                    reason: "all retries exhausted, no fallback available".into(),
                })
            }
        }
    }

    fn plan_hedging(&self, candidates: &[&ModelCandidate], max: u32) -> Vec<String> {
        let count = (max as usize).min(candidates.len());
        candidates
            .iter()
            .take(count)
            .map(|c| c.variant.id.clone())
            .collect()
    }

    fn record_failure(&self, variant_id: &str) {
        let mut state = self.circuit_breaker_state.lock().unwrap();
        let count = state
            .failure_counts
            .entry(variant_id.to_string())
            .or_insert(0);
        *count += 1;
    }

    fn reset_failures(&self, variant_id: &str) {
        let mut state = self.circuit_breaker_state.lock().unwrap();
        state.failure_counts.remove(variant_id);
    }

    fn name(&self) -> &str {
        "DefaultL5Policy"
    }
}

// ════════════════════════════════════════════════
// Selector Configuration
// ════════════════════════════════════════════════

/// Configuration for which selector implementation to use at each routing layer.
///
/// All fields have defaults, so an empty/missing `SelectorConfig` is fully backward-compatible.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SelectorConfig {
    #[serde(default = "default_l1")]
    pub l1: String,
    #[serde(default = "default_l2")]
    pub l2: String,
    #[serde(default = "default_l3")]
    pub l3: String,
    #[serde(default = "default_l4")]
    pub l4: String,
}

impl Default for SelectorConfig {
    fn default() -> Self {
        Self {
            l1: default_l1(),
            l2: default_l2(),
            l3: default_l3(),
            l4: default_l4(),
        }
    }
}

fn default_l1() -> String { "scored".into() }
fn default_l2() -> String { "default".into() }
fn default_l3() -> String { "affinity".into() }
fn default_l4() -> String { "best".into() }

// ════════════════════════════════════════════════
// Selector Factory
// ════════════════════════════════════════════════

/// Create an L1 selector from a configuration name.
///
/// Currently only `ScoredL1Selector` exists. The factory always returns it
/// regardless of the name, since L1 strategy switching (Auto/Pinned/Cascade)
/// is handled at the request level inside `ScoredL1Selector`.
pub fn create_l1_selector(_name: &str, feedback: Arc<FeedbackCollector>) -> Box<dyn L1ModelSelector> {
    Box::new(ScoredL1Selector::new(feedback))
}

/// Create an L2 selector from a configuration name.
///
/// Supported names: `"default"`, `"lowest_latency"`, `"lowest_cost"`, `"priority"`.
/// Unknown names fall back to `DefaultL2Selector`.
pub fn create_l2_selector(name: &str) -> Box<dyn L2ProviderSelector> {
    match name {
        "lowest_latency" => Box::new(LowestLatencyL2Selector),
        "lowest_cost" => Box::new(LowestCostL2Selector),
        "priority" => Box::new(PriorityL2Selector),
        _ => Box::new(DefaultL2Selector), // "default" or unknown
    }
}

/// Create an L3 selector from a configuration name.
///
/// Supported names: `"affinity"`, `"round_robin"`, `"least_load"`.
/// Unknown names fall back to `AffinityL3Selector`.
pub fn create_l3_selector(name: &str) -> Box<dyn L3AccountSelector> {
    match name {
        "round_robin" => Box::new(RoundRobinL3Selector::new()),
        "least_load" => Box::new(LeastLoadL3Selector),
        _ => Box::new(AffinityL3Selector), // "affinity" or unknown
    }
}

/// Create an L4 selector from a configuration name.
///
/// Supported names: `"best"`, `"weighted"`.
/// Unknown names fall back to `BestL4Selector`.
pub fn create_l4_selector(name: &str) -> Box<dyn L4VariantSelector> {
    match name {
        "weighted" => Box::new(WeightedL4Selector),
        _ => Box::new(BestL4Selector), // "best" or unknown
    }
}
