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
                    - (candidate.variant.performance.p50_latency_ms as f32 / max_latency).min(1.0);

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

        let specialty_bonus: f32 = if candidate.model.specialties.contains(&profile.task_type) {
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
                indices.retain(|&i| candidates[i].model.capabilities.max_context_window >= min_ctx);

                if indices.is_empty() {
                    return Err(RouterError::AllFiltered {
                        reason: "no candidates satisfy capability / context constraints".into(),
                    });
                }

                let mut scored: Vec<(usize, f32)> = indices
                    .into_iter()
                    .map(|i| {
                        let score = self.score_candidate(objective, ctx.profile, &candidates[i]);
                        (i, score)
                    })
                    .collect();
                scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                scored.retain(|(_, s)| *s > 0.0);

                if scored.is_empty() {
                    return Err(RouterError::AllFiltered {
                        reason: "all candidates scored 0".into(),
                    });
                }

                Ok(scored.into_iter().map(|(i, _)| i).collect())
            }
            ModelSelectionStrategy::Cascade { .. } => Ok((0..candidates.len()).collect()),
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
                        let utilisation = (c.variant.performance.current_qps / rps_limit).min(1.0);
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
    pub fn new(resilience_policy: ResiliencePolicy, registry: Arc<InMemoryRegistry>) -> Self {
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
            FailureType::HttpError(status) => policy.retry.retry_on.iter().any(|cond| match cond {
                RetryCondition::HttpStatus(statuses) => statuses.contains(status),
                _ => false,
            }),
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
                    if let Some(alt_variant) =
                        self.find_fallback_variant(failed_variant_id, failed_prov.as_deref(), true)
                    {
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
                        let candidates = self.registry.get_candidates_for_model(model_id);
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

fn default_l1() -> String {
    "scored".into()
}
fn default_l2() -> String {
    "default".into()
}
fn default_l3() -> String {
    "affinity".into()
}
fn default_l4() -> String {
    "best".into()
}

// ════════════════════════════════════════════════
// Selector Factory
// ════════════════════════════════════════════════

/// Create an L1 selector from a configuration name.
///
/// Currently only `ScoredL1Selector` exists. The factory always returns it
/// regardless of the name, since L1 strategy switching (Auto/Pinned/Cascade)
/// is handled at the request level inside `ScoredL1Selector`.
pub fn create_l1_selector(
    _name: &str,
    feedback: Arc<FeedbackCollector>,
) -> Box<dyn L1ModelSelector> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{ContextSize, Language, RequestProfile};
    use crate::feedback::FeedbackCollector;
    use crate::registry::InMemoryRegistry;
    use crate::router::RouteMetadata;
    use crate::strategy::{
        BackoffStrategy, CircuitBreakerPolicy, FallbackPolicy, ModelSelectionStrategy,
        ResiliencePolicy, RetryCondition, RetryPolicy, RetryScope, RoutingObjective,
        TimeoutPolicy,
    };
    
    use std::collections::HashMap;
    use std::sync::Arc;

    // ════════════════════════════════════════════════
    // Test helpers
    // ════════════════════════════════════════════════

    struct CandidateOpts<'a> {
        variant_id: &'a str,
        model_id: &'a str,
        provider_id: &'a str,
        account_id: &'a str,
        endpoint_id: &'a str,
        quality_tier: QualityTier,
        input_cost: f64,
        output_cost: f64,
        p50_latency_ms: u32,
        p99_latency_ms: u32,
        ttft_p50_ms: u32,
        current_qps: f32,
        weight: f32,
        supports_vision: bool,
        supports_reasoning: bool,
        max_context_window: u32,
        specialties: Vec<TaskType>,
        region: Option<String>,
        billing_tag: Option<String>,
        rpm: Option<u32>,
        priority: u32,
    }

    impl<'a> Default for CandidateOpts<'a> {
        fn default() -> Self {
            Self {
                variant_id: "v1",
                model_id: "m1",
                provider_id: "p1",
                account_id: "a1",
                endpoint_id: "e1",
                quality_tier: QualityTier::A,
                input_cost: 3.0,
                output_cost: 15.0,
                p50_latency_ms: 200,
                p99_latency_ms: 500,
                ttft_p50_ms: 100,
                current_qps: 5.0,
                weight: 1.0,
                supports_vision: false,
                supports_reasoning: false,
                max_context_window: 128_000,
                specialties: vec![],
                region: None,
                billing_tag: None,
                rpm: None,
                priority: 0,
            }
        }
    }

    fn make_candidate(opts: CandidateOpts) -> ModelCandidate {
        let pricing = ModelPricing {
            input_per_1m_tokens: opts.input_cost,
            output_per_1m_tokens: opts.output_cost,
            currency: "USD".into(),
        };

        ModelCandidate {
            variant: ModelVariant {
                id: opts.variant_id.into(),
                model_id: opts.model_id.into(),
                endpoint_id: opts.endpoint_id.into(),
                account_id: Some(opts.account_id.into()),
                deployment_name: None,
                weight: opts.weight,
                performance: ModelPerformance {
                    p50_latency_ms: opts.p50_latency_ms,
                    p95_latency_ms: opts.p99_latency_ms.saturating_sub(50),
                    p99_latency_ms: opts.p99_latency_ms,
                    ttft_p50_ms: opts.ttft_p50_ms,
                    throughput_tps: 100.0,
                    error_rate: 0.01,
                    current_qps: opts.current_qps,
                    queue_depth: 0,
                    last_updated: None,
                    gpu_utilization: 0.5,
                    kv_cache_usage: 0.3,
                    running_requests: 0,
                    waiting_requests: 0,
                    prefix_cache_hit_rate: 0.0,
                    prefix_cache_blocks: 0,
                },
                pricing_override: None,
                status: VariantStatus::Active,
                base_url: None,
                upstream_model_name: None,
            metadata: std::collections::HashMap::new(),
            },
            model: Model {
                id: opts.model_id.into(),
                display_name: format!("Model {}", opts.model_id),
                quality_tier: opts.quality_tier,
                capabilities: ModelCapabilities {
                    max_context_window: opts.max_context_window,
                    max_output_tokens: 4096,
                    supports_streaming: true,
                    supports_function_calling: true,
                    supports_vision: opts.supports_vision,
                    supports_json_mode: true,
                    supports_reasoning: opts.supports_reasoning,
                },
                pricing: pricing.clone(),
                specialties: opts.specialties,
                limits: None,
                status: ModelStatus::Available,
            },
            account: Account {
                id: opts.account_id.into(),
                endpoint_id: opts.endpoint_id.into(),
                auth: AuthConfig::default(),
                project_id: None,
                rate_limit: opts.rpm.map(|rpm| RateLimit {
                    rpm: Some(rpm),
                    tpm: None,
                    concurrent: None,
                }),
                billing_tag: opts.billing_tag,
                weight: 1.0,
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
            },
            endpoint: Endpoint {
                id: opts.endpoint_id.into(),
                provider_id: opts.provider_id.into(),
                base_url: None,
                region: opts.region,
                auth_override: None,
                rate_limit: None,
                priority: opts.priority,
                status: HealthStatus::Healthy,
            },
            provider: Provider {
                id: opts.provider_id.into(),
                name: format!("Provider {}", opts.provider_id),
                auth: AuthConfig::default(),
                base_url: "https://api.example.com".into(),
                rate_limit: None,
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
                protocols: vec![],
            },
            effective_pricing: pricing,
        }
    }

    fn default_profile() -> RequestProfile {
        RequestProfile {
            complexity: 0.2,
            task_type: TaskType::SimpleQA,
            estimated_tokens: 100,
            requires_reasoning: false,
            requires_vision: false,
            language: Language::English,
            is_multi_turn: false,
            context_window_need: ContextSize::Small,
        }
    }

    fn default_metadata() -> RouteMetadata {
        RouteMetadata {
            affinity_key: None,
            tenant_id: None,
            region: None,
            analyze_context: None,
            prefix_key: None,
        }
    }

    // ════════════════════════════════════════════════
    // L1 ScoredL1Selector tests
    // ════════════════════════════════════════════════

    #[test]
    fn test_l1_pinned_finds_matching_model() {
        let feedback = Arc::new(FeedbackCollector::new(100));
        let selector = ScoredL1Selector::new(feedback);

        let c1 = make_candidate(CandidateOpts {
            variant_id: "v1",
            model_id: "gpt-4",
            ..Default::default()
        });
        let c2 = make_candidate(CandidateOpts {
            variant_id: "v2",
            model_id: "claude-3",
            ..Default::default()
        });
        let candidates = vec![c1, c2];

        let strategy = ModelSelectionStrategy::Pinned {
            model_id: "claude-3".into(),
        };
        let profile = default_profile();
        let metadata = default_metadata();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result = selector.select(&candidates, &ctx).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], 1); // index of claude-3 candidate
    }

    #[test]
    fn test_l1_pinned_errors_on_missing_model() {
        let feedback = Arc::new(FeedbackCollector::new(100));
        let selector = ScoredL1Selector::new(feedback);

        let c1 = make_candidate(CandidateOpts {
            variant_id: "v1",
            model_id: "gpt-4",
            ..Default::default()
        });
        let candidates = vec![c1];

        let strategy = ModelSelectionStrategy::Pinned {
            model_id: "nonexistent-model".into(),
        };
        let profile = default_profile();
        let metadata = default_metadata();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result = selector.select(&candidates, &ctx);
        assert!(result.is_err());
        match result.unwrap_err() {
            RouterError::AllFiltered { reason } => {
                assert!(reason.contains("nonexistent-model"));
            }
            other => panic!("expected AllFiltered, got: {:?}", other),
        }
    }

    #[test]
    fn test_l1_auto_balanced_ranks_by_composite_score() {
        let feedback = Arc::new(FeedbackCollector::new(100));
        let selector = ScoredL1Selector::new(feedback);

        // Candidate A: S tier, low cost, low latency (should rank highest)
        let c_best = make_candidate(CandidateOpts {
            variant_id: "v-best",
            model_id: "m-best",
            quality_tier: QualityTier::S,
            input_cost: 1.0,
            output_cost: 2.0,
            p50_latency_ms: 50,
            p99_latency_ms: 100,
            ..Default::default()
        });

        // Candidate B: A tier, medium cost, medium latency
        let c_mid = make_candidate(CandidateOpts {
            variant_id: "v-mid",
            model_id: "m-mid",
            quality_tier: QualityTier::A,
            input_cost: 10.0,
            output_cost: 30.0,
            p50_latency_ms: 500,
            p99_latency_ms: 1000,
            ..Default::default()
        });

        // Candidate C: C tier, high cost, high latency (should rank lowest)
        let c_worst = make_candidate(CandidateOpts {
            variant_id: "v-worst",
            model_id: "m-worst",
            quality_tier: QualityTier::C,
            input_cost: 50.0,
            output_cost: 80.0,
            p50_latency_ms: 5000,
            p99_latency_ms: 8000,
            ..Default::default()
        });

        let candidates = vec![c_best, c_mid, c_worst];

        let strategy = ModelSelectionStrategy::Auto {
            objective: RoutingObjective::Balanced {
                cost_weight: 0.3,
                quality_weight: 0.5,
                latency_weight: 0.2,
            },
            pool_ids: None,
            required_capabilities: None,
        };
        let profile = default_profile();
        let metadata = default_metadata();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result = selector.select(&candidates, &ctx).unwrap();
        assert_eq!(result.len(), 3);
        // First should be the best candidate (S tier, cheap, fast)
        assert_eq!(result[0], 0, "S tier + cheap + fast should rank first");
        // Last should be the worst candidate (C tier, expensive, slow)
        assert_eq!(result[2], 2, "C tier + expensive + slow should rank last");
    }

    #[test]
    fn test_l1_auto_cost_first_prioritizes_cheapest() {
        let feedback = Arc::new(FeedbackCollector::new(100));
        let selector = ScoredL1Selector::new(feedback);

        // Cheap but A tier
        let c_cheap = make_candidate(CandidateOpts {
            variant_id: "v-cheap",
            model_id: "m-cheap",
            quality_tier: QualityTier::A,
            input_cost: 0.5,
            output_cost: 1.0,
            ..Default::default()
        });

        // Expensive but S tier
        let c_expensive = make_candidate(CandidateOpts {
            variant_id: "v-expensive",
            model_id: "m-expensive",
            quality_tier: QualityTier::S,
            input_cost: 30.0,
            output_cost: 60.0,
            ..Default::default()
        });

        // Very cheap but C tier (below quality floor of 0.4)
        let c_low_quality = make_candidate(CandidateOpts {
            variant_id: "v-low",
            model_id: "m-low",
            quality_tier: QualityTier::C,
            input_cost: 0.1,
            output_cost: 0.2,
            ..Default::default()
        });

        let candidates = vec![c_cheap, c_expensive, c_low_quality];

        let strategy = ModelSelectionStrategy::Auto {
            objective: RoutingObjective::CostFirst {
                min_quality_score: 0.4,
                budget_per_request: None,
            },
            pool_ids: None,
            required_capabilities: None,
        };
        let profile = default_profile();
        let metadata = default_metadata();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result = selector.select(&candidates, &ctx).unwrap();
        // C tier (quality 0.25) should be filtered out since min_quality_score is 0.4
        assert!(
            !result.contains(&2),
            "C tier candidate should be filtered by quality floor"
        );
        // Cheapest qualifying candidate should rank first
        assert_eq!(result[0], 0, "cheapest qualifying candidate should rank first");
    }

    #[test]
    fn test_l1_auto_quality_first_prioritizes_highest_tier() {
        let feedback = Arc::new(FeedbackCollector::new(100));
        let selector = ScoredL1Selector::new(feedback);

        let c_s = make_candidate(CandidateOpts {
            variant_id: "v-s",
            model_id: "m-s",
            quality_tier: QualityTier::S,
            input_cost: 50.0,
            output_cost: 80.0,
            p50_latency_ms: 300,
            p99_latency_ms: 600,
            ..Default::default()
        });

        let c_b = make_candidate(CandidateOpts {
            variant_id: "v-b",
            model_id: "m-b",
            quality_tier: QualityTier::B,
            input_cost: 1.0,
            output_cost: 2.0,
            p50_latency_ms: 50,
            p99_latency_ms: 100,
            ..Default::default()
        });

        let candidates = vec![c_s, c_b];

        let strategy = ModelSelectionStrategy::Auto {
            objective: RoutingObjective::QualityFirst {
                max_latency_ms: None,
            },
            pool_ids: None,
            required_capabilities: None,
        };
        let profile = default_profile();
        let metadata = default_metadata();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result = selector.select(&candidates, &ctx).unwrap();
        assert_eq!(result[0], 0, "S tier should rank first in QualityFirst mode");
    }

    #[test]
    fn test_l1_auto_slo_first_filters_high_latency_and_low_quality() {
        let feedback = Arc::new(FeedbackCollector::new(100));
        let selector = ScoredL1Selector::new(feedback);

        // Good: meets SLO constraints
        let c_good = make_candidate(CandidateOpts {
            variant_id: "v-good",
            model_id: "m-good",
            quality_tier: QualityTier::A,
            p50_latency_ms: 100,
            p99_latency_ms: 200,
            ttft_p50_ms: 50,
            ..Default::default()
        });

        // Too slow (p99 exceeds max_latency_ms)
        let c_slow = make_candidate(CandidateOpts {
            variant_id: "v-slow",
            model_id: "m-slow",
            quality_tier: QualityTier::A,
            p50_latency_ms: 500,
            p99_latency_ms: 2000,
            ttft_p50_ms: 50,
            ..Default::default()
        });

        // Too low quality
        let c_low_q = make_candidate(CandidateOpts {
            variant_id: "v-lowq",
            model_id: "m-lowq",
            quality_tier: QualityTier::C,
            p50_latency_ms: 50,
            p99_latency_ms: 100,
            ttft_p50_ms: 30,
            ..Default::default()
        });

        // TTFT too high
        let c_slow_ttft = make_candidate(CandidateOpts {
            variant_id: "v-slow-ttft",
            model_id: "m-slow-ttft",
            quality_tier: QualityTier::S,
            p50_latency_ms: 100,
            p99_latency_ms: 200,
            ttft_p50_ms: 600,
            ..Default::default()
        });

        let candidates = vec![c_good, c_slow, c_low_q, c_slow_ttft];

        let strategy = ModelSelectionStrategy::Auto {
            objective: RoutingObjective::SloFirst {
                max_latency_ms: 500,
                max_ttft_ms: 200,
                min_quality_score: 0.4,
            },
            pool_ids: None,
            required_capabilities: None,
        };
        let profile = default_profile();
        let metadata = default_metadata();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result = selector.select(&candidates, &ctx).unwrap();
        // Only the good candidate should survive: slow one has p99=2000>500, low quality has 0.25<0.4, slow ttft has 600>200
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], 0, "only the good candidate should survive SLO filtering");
    }

    #[test]
    fn test_l1_capability_filters() {
        let feedback = Arc::new(FeedbackCollector::new(100));
        let selector = ScoredL1Selector::new(feedback);

        // Supports vision
        let c_vision = make_candidate(CandidateOpts {
            variant_id: "v-vision",
            model_id: "m-vision",
            supports_vision: true,
            supports_reasoning: false,
            ..Default::default()
        });

        // Supports reasoning
        let c_reasoning = make_candidate(CandidateOpts {
            variant_id: "v-reasoning",
            model_id: "m-reasoning",
            supports_vision: false,
            supports_reasoning: true,
            ..Default::default()
        });

        // Supports neither
        let c_basic = make_candidate(CandidateOpts {
            variant_id: "v-basic",
            model_id: "m-basic",
            supports_vision: false,
            supports_reasoning: false,
            ..Default::default()
        });

        // Small context window
        let c_small_ctx = make_candidate(CandidateOpts {
            variant_id: "v-small-ctx",
            model_id: "m-small-ctx",
            max_context_window: 2000,
            ..Default::default()
        });

        let candidates = vec![c_vision, c_reasoning, c_basic, c_small_ctx];

        // Test vision requirement from profile
        let strategy = ModelSelectionStrategy::Auto {
            objective: RoutingObjective::QualityFirst {
                max_latency_ms: None,
            },
            pool_ids: None,
            required_capabilities: None,
        };
        let mut profile = default_profile();
        profile.requires_vision = true;
        let metadata = default_metadata();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result = selector.select(&candidates, &ctx).unwrap();
        // Only vision-capable candidate should be returned
        assert!(result.contains(&0), "vision candidate should be included");
        assert!(!result.contains(&1), "non-vision reasoning candidate should be filtered");
        assert!(!result.contains(&2), "basic candidate should be filtered");

        // Test reasoning requirement from profile
        let mut profile2 = default_profile();
        profile2.requires_reasoning = true;
        let ctx2 = RouteContext {
            profile: &profile2,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result2 = selector.select(&candidates, &ctx2).unwrap();
        assert!(result2.contains(&1), "reasoning candidate should be included");
        assert!(!result2.contains(&2), "basic candidate should be filtered out for reasoning");

        // Test context window filtering via XLarge context need
        let mut profile3 = default_profile();
        profile3.context_window_need = ContextSize::XLarge;
        let ctx3 = RouteContext {
            profile: &profile3,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result3 = selector.select(&candidates, &ctx3).unwrap();
        // Small context candidate (2000) should be filtered out for XLarge (min 128000)
        assert!(!result3.contains(&3), "small context candidate should be filtered for XLarge need");

        // Test RequiredCapabilities filter
        let strategy_with_caps = ModelSelectionStrategy::Auto {
            objective: RoutingObjective::QualityFirst {
                max_latency_ms: None,
            },
            pool_ids: None,
            required_capabilities: Some(crate::strategy::RequiredCapabilities {
                min_context_window: None,
                function_calling: None,
                vision: Some(true),
                json_mode: None,
                reasoning: None,
            }),
        };
        let profile4 = default_profile();
        let ctx4 = RouteContext {
            profile: &profile4,
            metadata: &metadata,
            strategy: &strategy_with_caps,
            required_protocol: None,
        };

        let result4 = selector.select(&candidates, &ctx4).unwrap();
        assert!(result4.contains(&0), "vision candidate should survive RequiredCapabilities filter");
        assert!(!result4.contains(&2), "basic candidate should be filtered by RequiredCapabilities");
    }

    #[test]
    fn test_l1_all_filtered_returns_error() {
        let feedback = Arc::new(FeedbackCollector::new(100));
        let selector = ScoredL1Selector::new(feedback);

        // Only a candidate that lacks vision
        let c = make_candidate(CandidateOpts {
            variant_id: "v1",
            model_id: "m1",
            supports_vision: false,
            ..Default::default()
        });
        let candidates = vec![c];

        let strategy = ModelSelectionStrategy::Auto {
            objective: RoutingObjective::QualityFirst {
                max_latency_ms: None,
            },
            pool_ids: None,
            required_capabilities: None,
        };
        let mut profile = default_profile();
        profile.requires_vision = true;
        let metadata = default_metadata();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result = selector.select(&candidates, &ctx);
        assert!(result.is_err());
        match result.unwrap_err() {
            RouterError::AllFiltered { .. } => {}
            other => panic!("expected AllFiltered, got: {:?}", other),
        }
    }

    // ════════════════════════════════════════════════
    // L2 Provider Selector tests
    // ════════════════════════════════════════════════

    #[test]
    fn test_l2_default_geo_proximity_boost_and_quota_penalty() {
        let selector = DefaultL2Selector;

        // Candidate in us-east (matching region), high quota usage
        let c_local_busy = make_candidate(CandidateOpts {
            variant_id: "v-local",
            model_id: "m1",
            endpoint_id: "e-local",
            region: Some("us-east".into()),
            current_qps: 9.0,
            rpm: Some(600), // 10 rps, 9 qps = 0.9 utilization
            ..Default::default()
        });

        // Candidate in eu-west (different region), low quota usage
        let c_remote_idle = make_candidate(CandidateOpts {
            variant_id: "v-remote",
            model_id: "m1",
            endpoint_id: "e-remote",
            region: Some("eu-west".into()),
            current_qps: 1.0,
            rpm: Some(600), // 10 rps, 1 qps = 0.1 utilization
            ..Default::default()
        });

        let profile = default_profile();
        let metadata = RouteMetadata {
            region: Some("us-east".into()),
            ..default_metadata()
        };
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let input = vec![&c_local_busy, &c_remote_idle];
        let result = selector.rerank(input, &ctx);

        // Local candidate gets +100 geo bonus but -180 quota penalty (0.9 * 200) = net -80
        // Remote candidate gets 0 geo bonus but -20 quota penalty (0.1 * 200) = net -20
        // Remote should rank first because its net score (-20) > local net score (-80)
        assert_eq!(
            result[0].variant.id, "v-remote",
            "remote idle candidate should rank first due to quota penalty outweighing geo boost"
        );
    }

    #[test]
    fn test_l2_lowest_latency_sorts_by_p50() {
        let selector = LowestLatencyL2Selector;

        let c_fast = make_candidate(CandidateOpts {
            variant_id: "v-fast",
            p50_latency_ms: 50,
            ..Default::default()
        });
        let c_slow = make_candidate(CandidateOpts {
            variant_id: "v-slow",
            p50_latency_ms: 500,
            ..Default::default()
        });
        let c_medium = make_candidate(CandidateOpts {
            variant_id: "v-medium",
            p50_latency_ms: 200,
            ..Default::default()
        });

        let profile = default_profile();
        let metadata = default_metadata();
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let input = vec![&c_slow, &c_medium, &c_fast];
        let result = selector.rerank(input, &ctx);

        assert_eq!(result[0].variant.id, "v-fast");
        assert_eq!(result[1].variant.id, "v-medium");
        assert_eq!(result[2].variant.id, "v-slow");
    }

    #[test]
    fn test_l2_lowest_cost_sorts_by_cost() {
        let selector = LowestCostL2Selector;

        let c_cheap = make_candidate(CandidateOpts {
            variant_id: "v-cheap",
            input_cost: 0.5,
            output_cost: 1.0,
            ..Default::default()
        });
        let c_expensive = make_candidate(CandidateOpts {
            variant_id: "v-expensive",
            input_cost: 50.0,
            output_cost: 100.0,
            ..Default::default()
        });
        let c_mid = make_candidate(CandidateOpts {
            variant_id: "v-mid",
            input_cost: 10.0,
            output_cost: 20.0,
            ..Default::default()
        });

        let profile = default_profile();
        let metadata = default_metadata();
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let input = vec![&c_expensive, &c_mid, &c_cheap];
        let result = selector.rerank(input, &ctx);

        assert_eq!(result[0].variant.id, "v-cheap");
        assert_eq!(result[1].variant.id, "v-mid");
        assert_eq!(result[2].variant.id, "v-expensive");
    }

    #[test]
    fn test_l2_priority_sorts_by_priority() {
        let selector = PriorityL2Selector;

        let c_low_prio = make_candidate(CandidateOpts {
            variant_id: "v-low-prio",
            priority: 10,
            ..Default::default()
        });
        let c_high_prio = make_candidate(CandidateOpts {
            variant_id: "v-high-prio",
            priority: 0,
            ..Default::default()
        });
        let c_mid_prio = make_candidate(CandidateOpts {
            variant_id: "v-mid-prio",
            priority: 5,
            ..Default::default()
        });

        let profile = default_profile();
        let metadata = default_metadata();
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let input = vec![&c_low_prio, &c_mid_prio, &c_high_prio];
        let result = selector.rerank(input, &ctx);

        assert_eq!(result[0].variant.id, "v-high-prio");
        assert_eq!(result[1].variant.id, "v-mid-prio");
        assert_eq!(result[2].variant.id, "v-low-prio");
    }

    #[test]
    fn test_l2_empty_candidates() {
        let selector = DefaultL2Selector;
        let profile = default_profile();
        let metadata = default_metadata();
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let input: Vec<&ModelCandidate> = vec![];
        let result = selector.rerank(input, &ctx);
        assert!(result.is_empty());

        // Also check LowestLatencyL2 with empty
        let selector2 = LowestLatencyL2Selector;
        let result2 = selector2.rerank(vec![], &ctx);
        assert!(result2.is_empty());
    }

    // ════════════════════════════════════════════════
    // L3 Account Selector tests
    // ════════════════════════════════════════════════

    #[test]
    fn test_l3_affinity_tenant_isolation_via_billing_tag() {
        let selector = AffinityL3Selector;

        let c_tenant_a = make_candidate(CandidateOpts {
            variant_id: "v1",
            account_id: "acct-a",
            billing_tag: Some("tenant-alpha".into()),
            ..Default::default()
        });
        let c_tenant_b = make_candidate(CandidateOpts {
            variant_id: "v2",
            account_id: "acct-b",
            billing_tag: Some("tenant-beta".into()),
            ..Default::default()
        });
        let c_no_tag = make_candidate(CandidateOpts {
            variant_id: "v3",
            account_id: "acct-c",
            billing_tag: None,
            ..Default::default()
        });

        let candidates_refs = vec![&c_tenant_a, &c_tenant_b, &c_no_tag];

        let profile = default_profile();
        let metadata = RouteMetadata {
            tenant_id: Some("tenant-beta".into()),
            ..default_metadata()
        };
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result = selector.select(&candidates_refs, &ctx).unwrap();
        assert_eq!(result, 1, "tenant-beta should route to acct-b");
    }

    #[test]
    fn test_l3_affinity_consistent_hash_for_same_key() {
        let selector = AffinityL3Selector;

        let c1 = make_candidate(CandidateOpts {
            variant_id: "v1",
            account_id: "acct-1",
            ..Default::default()
        });
        let c2 = make_candidate(CandidateOpts {
            variant_id: "v2",
            account_id: "acct-2",
            ..Default::default()
        });
        let c3 = make_candidate(CandidateOpts {
            variant_id: "v3",
            account_id: "acct-3",
            ..Default::default()
        });

        let candidates_refs = vec![&c1, &c2, &c3];

        let profile = default_profile();
        let metadata = RouteMetadata {
            affinity_key: Some("session-12345".into()),
            ..default_metadata()
        };
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        // Same affinity key should produce the same result every time
        let result1 = selector.select(&candidates_refs, &ctx).unwrap();
        let result2 = selector.select(&candidates_refs, &ctx).unwrap();
        let result3 = selector.select(&candidates_refs, &ctx).unwrap();

        assert_eq!(result1, result2);
        assert_eq!(result2, result3);
    }

    #[test]
    fn test_l3_affinity_soft_migration_on_overloaded() {
        let selector = AffinityL3Selector;

        // Overloaded account: qps=9, rpm=600 => rps_limit=10, utilisation=0.9 => overloaded
        let c_overloaded = make_candidate(CandidateOpts {
            variant_id: "v-overloaded",
            account_id: "acct-overloaded",
            current_qps: 9.0,
            rpm: Some(600),
            ..Default::default()
        });

        // Idle account
        let c_idle = make_candidate(CandidateOpts {
            variant_id: "v-idle",
            account_id: "acct-idle",
            current_qps: 1.0,
            rpm: Some(600),
            ..Default::default()
        });

        let candidates_refs = vec![&c_overloaded, &c_idle];

        let profile = default_profile();

        // We need to find an affinity key that hashes to index 0 (the overloaded one).
        // Since we have 2 accounts, we try multiple keys until one hashes to index 0.
        let mut key_to_overloaded = None;
        for i in 0..1000 {
            let test_key = format!("test-key-{}", i);
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            use std::hash::Hash;
            test_key.hash(&mut hasher);
            use std::hash::Hasher;
            let hash_val = hasher.finish();
            if (hash_val as usize) % 2 == 0 {
                key_to_overloaded = Some(test_key);
                break;
            }
        }

        let key = key_to_overloaded.expect("should find a key that hashes to index 0");
        let metadata = RouteMetadata {
            affinity_key: Some(key),
            ..default_metadata()
        };
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result = selector.select(&candidates_refs, &ctx).unwrap();
        // Even though affinity hashes to the overloaded account, soft migration
        // should redirect to the idle account
        assert_eq!(
            result, 1,
            "soft migration should redirect from overloaded to idle account"
        );
    }

    #[test]
    fn test_l3_round_robin_cycles() {
        let selector = RoundRobinL3Selector::new();

        let c1 = make_candidate(CandidateOpts {
            variant_id: "v1",
            account_id: "acct-1",
            ..Default::default()
        });
        let c2 = make_candidate(CandidateOpts {
            variant_id: "v2",
            account_id: "acct-2",
            ..Default::default()
        });
        let c3 = make_candidate(CandidateOpts {
            variant_id: "v3",
            account_id: "acct-3",
            ..Default::default()
        });

        let candidates_refs = vec![&c1, &c2, &c3];

        let profile = default_profile();
        let metadata = default_metadata();
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let r1 = selector.select(&candidates_refs, &ctx).unwrap();
        let r2 = selector.select(&candidates_refs, &ctx).unwrap();
        let r3 = selector.select(&candidates_refs, &ctx).unwrap();
        let r4 = selector.select(&candidates_refs, &ctx).unwrap();

        assert_eq!(r1, 0);
        assert_eq!(r2, 1);
        assert_eq!(r3, 2);
        assert_eq!(r4, 0, "should cycle back to first candidate");
    }

    #[test]
    fn test_l3_least_load_picks_lowest_qps() {
        let selector = LeastLoadL3Selector;

        let c_busy = make_candidate(CandidateOpts {
            variant_id: "v-busy",
            account_id: "acct-busy",
            current_qps: 50.0,
            ..Default::default()
        });
        let c_idle = make_candidate(CandidateOpts {
            variant_id: "v-idle",
            account_id: "acct-idle",
            current_qps: 2.0,
            ..Default::default()
        });
        let c_medium = make_candidate(CandidateOpts {
            variant_id: "v-medium",
            account_id: "acct-medium",
            current_qps: 20.0,
            ..Default::default()
        });

        let candidates_refs = vec![&c_busy, &c_idle, &c_medium];

        let profile = default_profile();
        let metadata = default_metadata();
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result = selector.select(&candidates_refs, &ctx).unwrap();
        assert_eq!(result, 1, "should pick the candidate with lowest current_qps (2.0)");
    }

    #[test]
    fn test_l3_empty_candidates_return_error() {
        let affinity = AffinityL3Selector;
        let round_robin = RoundRobinL3Selector::new();
        let least_load = LeastLoadL3Selector;

        let empty: Vec<&ModelCandidate> = vec![];
        let profile = default_profile();
        let metadata = default_metadata();
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        assert!(affinity.select(&empty, &ctx).is_err());
        assert!(round_robin.select(&empty, &ctx).is_err());
        assert!(least_load.select(&empty, &ctx).is_err());
    }

    // ════════════════════════════════════════════════
    // L4 Variant Selector tests
    // ════════════════════════════════════════════════

    #[test]
    fn test_l4_best_picks_first() {
        let selector = BestL4Selector;

        let c1 = make_candidate(CandidateOpts {
            variant_id: "v-first",
            ..Default::default()
        });
        let c2 = make_candidate(CandidateOpts {
            variant_id: "v-second",
            ..Default::default()
        });

        let candidates_refs = vec![&c1, &c2];
        let profile = default_profile();
        let metadata = default_metadata();
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let result = selector.select(&candidates_refs, &ctx);
        assert_eq!(result.variant.id, "v-first");
    }

    #[test]
    fn test_l4_canary_splits_traffic() {
        let selector = CanaryL4Selector {
            canary_variant_id: "v-canary".into(),
            canary_pct: 0.2,
        };

        let c_stable = make_candidate(CandidateOpts {
            variant_id: "v-stable",
            ..Default::default()
        });
        let c_canary = make_candidate(CandidateOpts {
            variant_id: "v-canary",
            ..Default::default()
        });

        let candidates_refs = vec![&c_stable, &c_canary];
        let profile = default_profile();
        let metadata = default_metadata();
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let iterations = 1000;
        let mut canary_count = 0;
        for _ in 0..iterations {
            let result = selector.select(&candidates_refs, &ctx);
            if result.variant.id == "v-canary" {
                canary_count += 1;
            }
        }

        let canary_pct = canary_count as f64 / iterations as f64;
        assert!(
            canary_pct > 0.10 && canary_pct < 0.35,
            "canary traffic should be roughly 20%, got {:.1}%",
            canary_pct * 100.0
        );
    }

    #[test]
    fn test_l4_weighted_respects_weights() {
        let selector = WeightedL4Selector;

        let c_heavy = make_candidate(CandidateOpts {
            variant_id: "v-heavy",
            weight: 3.0,
            ..Default::default()
        });
        let c_light = make_candidate(CandidateOpts {
            variant_id: "v-light",
            weight: 1.0,
            ..Default::default()
        });

        let candidates_refs = vec![&c_heavy, &c_light];
        let profile = default_profile();
        let metadata = default_metadata();
        let strategy = ModelSelectionStrategy::default();
        let ctx = RouteContext {
            profile: &profile,
            metadata: &metadata,
            strategy: &strategy,
            required_protocol: None,
        };

        let iterations = 1000;
        let mut heavy_count = 0;
        for _ in 0..iterations {
            let result = selector.select(&candidates_refs, &ctx);
            if result.variant.id == "v-heavy" {
                heavy_count += 1;
            }
        }

        let heavy_pct = heavy_count as f64 / iterations as f64;
        // Weight 3.0 vs 1.0 means ~75% should be heavy
        assert!(
            heavy_pct > 0.50 && heavy_pct < 0.95,
            "heavy variant (weight 3.0) should win ~75% of the time, got {:.1}%",
            heavy_pct * 100.0
        );
    }

    // ════════════════════════════════════════════════
    // L5 Resilience Policy tests
    // ════════════════════════════════════════════════

    fn build_l5_registry() -> Arc<InMemoryRegistry> {
        let mut reg = InMemoryRegistry::new();

        reg.providers.insert(
            "prov-a".into(),
            Provider {
                id: "prov-a".into(),
                name: "Provider A".into(),
                auth: AuthConfig::default(),
                base_url: "https://a.example.com".into(),
                rate_limit: None,
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
                protocols: vec![],
            },
        );
        reg.providers.insert(
            "prov-b".into(),
            Provider {
                id: "prov-b".into(),
                name: "Provider B".into(),
                auth: AuthConfig::default(),
                base_url: "https://b.example.com".into(),
                rate_limit: None,
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
                protocols: vec![],
            },
        );

        reg.endpoints.insert(
            "ep-a".into(),
            Endpoint {
                id: "ep-a".into(),
                provider_id: "prov-a".into(),
                base_url: None,
                region: None,
                auth_override: None,
                rate_limit: None,
                priority: 0,
                status: HealthStatus::Healthy,
            },
        );
        reg.endpoints.insert(
            "ep-b".into(),
            Endpoint {
                id: "ep-b".into(),
                provider_id: "prov-b".into(),
                base_url: None,
                region: None,
                auth_override: None,
                rate_limit: None,
                priority: 0,
                status: HealthStatus::Healthy,
            },
        );

        reg.accounts.insert(
            "acct-a".into(),
            Account {
                id: "acct-a".into(),
                endpoint_id: "ep-a".into(),
                auth: AuthConfig::default(),
                project_id: None,
                rate_limit: None,
                billing_tag: None,
                weight: 1.0,
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
            },
        );
        reg.accounts.insert(
            "acct-b".into(),
            Account {
                id: "acct-b".into(),
                endpoint_id: "ep-b".into(),
                auth: AuthConfig::default(),
                project_id: None,
                rate_limit: None,
                billing_tag: None,
                weight: 1.0,
                status: HealthStatus::Healthy,
                metadata: HashMap::new(),
            },
        );

        reg.models.insert(
            "model-x".into(),
            Model {
                id: "model-x".into(),
                display_name: "Model X".into(),
                quality_tier: QualityTier::A,
                capabilities: ModelCapabilities::default(),
                pricing: ModelPricing::default(),
                specialties: vec![],
                limits: None,
                status: ModelStatus::Available,
            },
        );

        reg.variants.insert(
            "var-a".into(),
            ModelVariant {
                id: "var-a".into(),
                model_id: "model-x".into(),
                endpoint_id: "ep-a".into(),
                account_id: Some("acct-a".into()),
                deployment_name: None,
                weight: 1.0,
                performance: ModelPerformance::default(),
                pricing_override: None,
                status: VariantStatus::Active,
                base_url: None,
                upstream_model_name: None,
            metadata: std::collections::HashMap::new(),
            },
        );
        reg.variants.insert(
            "var-b".into(),
            ModelVariant {
                id: "var-b".into(),
                model_id: "model-x".into(),
                endpoint_id: "ep-b".into(),
                account_id: Some("acct-b".into()),
                deployment_name: None,
                weight: 1.0,
                performance: ModelPerformance::default(),
                pricing_override: None,
                status: VariantStatus::Active,
                base_url: None,
                upstream_model_name: None,
            metadata: std::collections::HashMap::new(),
            },
        );

        Arc::new(reg)
    }

    fn default_resilience_policy() -> ResiliencePolicy {
        ResiliencePolicy {
            retry: RetryPolicy {
                max_retries: 2,
                retry_on: vec![
                    RetryCondition::RateLimit,
                    RetryCondition::Timeout,
                    RetryCondition::ConnectionError,
                    RetryCondition::HttpStatus(vec![500, 502, 503]),
                ],
                backoff: BackoffStrategy::Exponential {
                    base_ms: 100,
                    max_ms: 5000,
                },
                retry_scope: RetryScope::CrossProvider,
            },
            fallback: FallbackPolicy::default(),
            circuit_breaker: CircuitBreakerPolicy {
                error_threshold_pct: 50.0,
                window_seconds: 60,
                min_requests: 5,
                open_duration_seconds: 30,
                half_open_requests: 3,
            },
            timeout: TimeoutPolicy::default(),
            hedging: None,
        }
    }

    #[test]
    fn test_l5_retry_on_rate_limit() {
        let registry = build_l5_registry();
        let policy = DefaultL5Policy::new(default_resilience_policy(), registry);

        let result = policy
            .on_failure("var-a", &FailureType::RateLimit, 1)
            .unwrap();

        match result {
            RecoveryAction::Retry { variant_id, delay_ms } => {
                // CrossProvider scope should try a different provider's variant
                assert_eq!(variant_id, "var-b", "should retry on different provider's variant");
                assert!(delay_ms > 0, "delay should be positive");
            }
            other => panic!("expected Retry, got: {:?}", other),
        }
    }

    #[test]
    fn test_l5_retry_on_timeout() {
        let registry = build_l5_registry();
        let policy = DefaultL5Policy::new(default_resilience_policy(), registry);

        let result = policy
            .on_failure("var-a", &FailureType::Timeout, 1)
            .unwrap();

        match result {
            RecoveryAction::Retry { delay_ms, .. } => {
                assert!(delay_ms > 0, "timeout retry should have a delay");
            }
            other => panic!("expected Retry, got: {:?}", other),
        }
    }

    #[test]
    fn test_l5_fallback_after_max_retries() {
        let registry = build_l5_registry();
        let policy = DefaultL5Policy::new(default_resilience_policy(), registry);

        // attempt 3 exceeds max_retries of 2
        let result = policy
            .on_failure("var-a", &FailureType::RateLimit, 3)
            .unwrap();

        match result {
            RecoveryAction::Fallback { variant_id } => {
                assert_ne!(variant_id, "var-a", "fallback should choose a different variant");
            }
            other => panic!("expected Fallback after max retries, got: {:?}", other),
        }
    }

    #[test]
    fn test_l5_circuit_breaker_opens() {
        let registry = build_l5_registry();
        let policy = DefaultL5Policy::new(default_resilience_policy(), registry);

        // Record enough failures to trip the circuit breaker (min_requests = 5)
        for _ in 0..5 {
            policy.record_failure("var-a");
        }

        // Now on_failure should detect circuit is open and fallback
        let result = policy
            .on_failure("var-a", &FailureType::RateLimit, 1)
            .unwrap();

        match result {
            RecoveryAction::Fallback { variant_id } => {
                assert_eq!(
                    variant_id, "var-b",
                    "circuit breaker open should fallback to different variant"
                );
            }
            other => panic!("expected Fallback due to circuit breaker, got: {:?}", other),
        }

        // Reset and verify recovery
        policy.reset_failures("var-a");
        let result_after_reset = policy
            .on_failure("var-a", &FailureType::RateLimit, 1)
            .unwrap();

        match result_after_reset {
            RecoveryAction::Retry { .. } => {
                // Good - circuit breaker is closed again after reset
            }
            other => panic!("expected Retry after circuit reset, got: {:?}", other),
        }
    }

    #[test]
    fn test_l5_backoff_calculation() {
        let registry = build_l5_registry();

        // Test fixed backoff
        let mut rp = default_resilience_policy();
        rp.retry.backoff = BackoffStrategy::Fixed { delay_ms: 500 };
        let policy_fixed = DefaultL5Policy::new(rp, Arc::clone(&registry));

        let delay1 = policy_fixed.compute_delay(1);
        let delay2 = policy_fixed.compute_delay(2);
        let delay3 = policy_fixed.compute_delay(3);
        assert_eq!(delay1, 500);
        assert_eq!(delay2, 500);
        assert_eq!(delay3, 500);

        // Test exponential backoff
        let mut rp2 = default_resilience_policy();
        rp2.retry.backoff = BackoffStrategy::Exponential {
            base_ms: 100,
            max_ms: 5000,
        };
        let policy_exp = DefaultL5Policy::new(rp2, Arc::clone(&registry));

        let delay_a1 = policy_exp.compute_delay(1); // 100 * 2^0 = 100
        let delay_a2 = policy_exp.compute_delay(2); // 100 * 2^1 = 200
        let delay_a3 = policy_exp.compute_delay(3); // 100 * 2^2 = 400
        assert_eq!(delay_a1, 100);
        assert_eq!(delay_a2, 200);
        assert_eq!(delay_a3, 400);

        // Test exponential max cap
        let mut rp3 = default_resilience_policy();
        rp3.retry.backoff = BackoffStrategy::Exponential {
            base_ms: 1000,
            max_ms: 3000,
        };
        let policy_capped = DefaultL5Policy::new(rp3, registry);

        let delay_c3 = policy_capped.compute_delay(3); // 1000 * 2^2 = 4000, capped to 3000
        assert_eq!(delay_c3, 3000);
    }

    #[test]
    fn test_l5_hedging_plan() {
        let registry = build_l5_registry();
        let policy = DefaultL5Policy::new(default_resilience_policy(), registry);

        let c1 = make_candidate(CandidateOpts {
            variant_id: "hedge-1",
            ..Default::default()
        });
        let c2 = make_candidate(CandidateOpts {
            variant_id: "hedge-2",
            ..Default::default()
        });
        let c3 = make_candidate(CandidateOpts {
            variant_id: "hedge-3",
            ..Default::default()
        });

        let candidates_refs: Vec<&ModelCandidate> = vec![&c1, &c2, &c3];

        // max=2, should return first 2
        let plan = policy.plan_hedging(&candidates_refs, 2);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0], "hedge-1");
        assert_eq!(plan[1], "hedge-2");

        // max=5, but only 3 candidates, should return all 3
        let plan_all = policy.plan_hedging(&candidates_refs, 5);
        assert_eq!(plan_all.len(), 3);

        // max=0, should return empty
        let plan_none = policy.plan_hedging(&candidates_refs, 0);
        assert!(plan_none.is_empty());
    }

    // ════════════════════════════════════════════════
    // Factory function tests
    // ════════════════════════════════════════════════

    #[test]
    fn test_create_l2_selector_returns_correct_type() {
        let default = create_l2_selector("default");
        assert_eq!(default.name(), "DefaultL2Selector");

        let latency = create_l2_selector("lowest_latency");
        assert_eq!(latency.name(), "LowestLatencyL2Selector");

        let cost = create_l2_selector("lowest_cost");
        assert_eq!(cost.name(), "LowestCostL2Selector");

        let priority = create_l2_selector("priority");
        assert_eq!(priority.name(), "PriorityL2Selector");

        // Unknown name should fall back to default
        let unknown = create_l2_selector("unknown_name");
        assert_eq!(unknown.name(), "DefaultL2Selector");
    }

    #[test]
    fn test_create_l3_selector_returns_correct_type() {
        let affinity = create_l3_selector("affinity");
        assert_eq!(affinity.name(), "AffinityL3Selector");

        let round_robin = create_l3_selector("round_robin");
        assert_eq!(round_robin.name(), "RoundRobinL3Selector");

        let least_load = create_l3_selector("least_load");
        assert_eq!(least_load.name(), "LeastLoadL3Selector");

        // Unknown name should fall back to affinity
        let unknown = create_l3_selector("unknown_name");
        assert_eq!(unknown.name(), "AffinityL3Selector");
    }

    #[test]
    fn test_create_l4_selector_returns_correct_type() {
        let best = create_l4_selector("best");
        assert_eq!(best.name(), "BestL4Selector");

        let weighted = create_l4_selector("weighted");
        assert_eq!(weighted.name(), "WeightedL4Selector");

        // Unknown name should fall back to best
        let unknown = create_l4_selector("unknown_name");
        assert_eq!(unknown.name(), "BestL4Selector");
    }
}
