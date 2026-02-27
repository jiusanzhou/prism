use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::time::Instant;

use rand::Rng;

use crate::analyzer::{AnalyzeContext, ChatMessage, RequestAnalyzer, RequestProfile, RuleBasedAnalyzer};
use crate::feedback::{FeedbackCollector, OutcomeReport};
use crate::registry::{InMemoryRegistry, ModelRegistryTrait};
use crate::strategy::*;
use crate::types::*;

/// Trait for the top-level routing engine.
pub trait Router: Send + Sync {
    fn route(&self, request: &RouteRequest) -> Result<RoutingDecision, RouterError>;
}

/// Input to the router.
#[derive(Debug, Clone)]
pub struct RouteRequest {
    pub messages: Vec<ChatMessage>,
    pub strategy: Option<ModelSelectionStrategy>,
    pub metadata: RouteMetadata,
    /// Optional context for the analyzer (caller type, source, tags, etc.)
    pub analyze_context: Option<AnalyzeContext>,
}

/// Extra metadata attached to a routing request.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RouteMetadata {
    /// Affinity key (e.g. session id) for L3 account selection.
    pub affinity_key: Option<String>,
    /// Tenant id for cost-isolation routing.
    pub tenant_id: Option<String>,
    /// Preferred region.
    pub region: Option<String>,
    /// Analyze context passed through for metadata/tracing.
    pub analyze_context: Option<AnalyzeContext>,
}

/// Errors returned by the router.
#[derive(Debug, Clone)]
pub enum RouterError {
    NoCandidates,
    AllFiltered { reason: String },
    InternalError { message: String },
}

impl std::fmt::Display for RouterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCandidates => write!(f, "no candidates available"),
            Self::AllFiltered { reason } => write!(f, "all candidates filtered: {reason}"),
            Self::InternalError { message } => write!(f, "internal error: {message}"),
        }
    }
}

impl std::error::Error for RouterError {}

// ──────────────────────────────────────────────
// Circuit breaker state (per-variant)
// ──────────────────────────────────────────────

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

// ──────────────────────────────────────────────
// PrismRouter — concrete implementation
// ──────────────────────────────────────────────

pub struct PrismRouter {
    pub analyzer: Box<dyn RequestAnalyzer>,
    pub registry: InMemoryRegistry,
    pub default_strategy: ModelSelectionStrategy,
    pub variant_strategy: VariantSelectionStrategy,
    pub resilience_policy: ResiliencePolicy,
    circuit_breaker_state: Mutex<CircuitBreakerState>,
    pub feedback: FeedbackCollector,
}

impl PrismRouter {
    pub fn new(registry: InMemoryRegistry) -> Self {
        Self {
            analyzer: Box::new(RuleBasedAnalyzer::new()),
            registry,
            default_strategy: ModelSelectionStrategy::default(),
            variant_strategy: VariantSelectionStrategy::Default,
            resilience_policy: ResiliencePolicy::default(),
            circuit_breaker_state: Mutex::new(CircuitBreakerState::new()),
            feedback: FeedbackCollector::new(1000),
        }
    }

    pub fn with_strategy(mut self, strategy: ModelSelectionStrategy) -> Self {
        self.default_strategy = strategy;
        self
    }

    pub fn with_analyzer(mut self, analyzer: Box<dyn RequestAnalyzer>) -> Self {
        self.analyzer = analyzer;
        self
    }

    pub fn with_variant_strategy(mut self, vs: VariantSelectionStrategy) -> Self {
        self.variant_strategy = vs;
        self
    }

    pub fn with_resilience_policy(mut self, rp: ResiliencePolicy) -> Self {
        self.resilience_policy = rp;
        self
    }

    // ── L1: Model selection ──

    fn select_model<'a>(
        &self,
        profile: &RequestProfile,
        candidates: &'a [ModelCandidate],
        strategy: &ModelSelectionStrategy,
    ) -> Result<Vec<&'a ModelCandidate>, RouterError> {
        match strategy {
            ModelSelectionStrategy::Pinned { model_id } => {
                let matched: Vec<_> = candidates
                    .iter()
                    .filter(|c| c.model.id == *model_id)
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
                let mut filtered: Vec<_> = candidates.iter().collect();

                if let Some(req) = required_capabilities {
                    let req_caps = crate::registry::RequiredCapabilities {
                        min_context_window: req.min_context_window,
                        function_calling: req.function_calling,
                        vision: req.vision,
                        json_mode: req.json_mode,
                        reasoning: req.reasoning,
                    };
                    filtered.retain(|c| req_caps.matches(&c.model.capabilities));
                }

                if profile.requires_vision {
                    filtered.retain(|c| c.model.capabilities.supports_vision);
                }

                if profile.requires_reasoning {
                    filtered.retain(|c| c.model.capabilities.supports_reasoning);
                }

                let min_ctx = match profile.context_window_need {
                    crate::analyzer::ContextSize::Small => 0,
                    crate::analyzer::ContextSize::Medium => 4000,
                    crate::analyzer::ContextSize::Large => 32000,
                    crate::analyzer::ContextSize::XLarge => 128000,
                };
                filtered.retain(|c| c.model.capabilities.max_context_window >= min_ctx);

                if filtered.is_empty() {
                    return Err(RouterError::AllFiltered {
                        reason: "no candidates satisfy capability / context constraints".into(),
                    });
                }

                let mut scored: Vec<_> = filtered
                    .iter()
                    .map(|c| {
                        let score = self.score_candidate(objective, profile, c);
                        (*c, score)
                    })
                    .collect();
                scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                scored.retain(|(_, s)| *s > 0.0);

                if scored.is_empty() {
                    return Err(RouterError::AllFiltered {
                        reason: "all candidates scored 0".into(),
                    });
                }

                Ok(scored.into_iter().map(|(c, _)| c).collect())
            }
            ModelSelectionStrategy::Cascade { .. } => {
                let all: Vec<_> = candidates.iter().collect();
                Ok(all)
            }
        }
    }

    fn score_candidate(
        &self,
        objective: &RoutingObjective,
        profile: &RequestProfile,
        candidate: &ModelCandidate,
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
                let quality_score = self.estimate_quality(profile, candidate) as f64;
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
                let quality = self.estimate_quality(profile, candidate);
                if quality < *min_quality_score {
                    return 0.0;
                }
                quality
            }
            RoutingObjective::CostFirst {
                min_quality_score,
                budget_per_request: _,
            } => {
                let quality = self.estimate_quality(profile, candidate);
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
                self.estimate_quality(profile, candidate)
            }
            RoutingObjective::Custom { .. } => 0.5,
        };

        // Apply feedback-based quality adjustment
        let feedback_adj = self.feedback.quality_adjustment(&candidate.variant.id);
        base_score * feedback_adj
    }

    fn estimate_quality(&self, profile: &RequestProfile, candidate: &ModelCandidate) -> f32 {
        let tier_score: f32 = match candidate.model.quality_tier {
            QualityTier::S => 1.0,
            QualityTier::A => 0.75,
            QualityTier::B => 0.5,
            QualityTier::C => 0.25,
        };

        let specialty_bonus: f32 = if candidate
            .model
            .specialties
            .contains(&profile.task_type)
        {
            0.15
        } else {
            0.0
        };

        (tier_score + specialty_bonus).min(1.0_f32)
    }

    // ── L2: Provider selection ──

    /// Re-rank candidates for L2 concerns: geo-proximity and quota-awareness.
    fn select_provider<'a>(
        &self,
        candidates: Vec<&'a ModelCandidate>,
        metadata: &RouteMetadata,
    ) -> Vec<&'a ModelCandidate> {
        if candidates.is_empty() {
            return candidates;
        }

        let mut scored: Vec<(&ModelCandidate, f32)> = candidates
            .into_iter()
            .map(|c| {
                let mut bonus = 0.0_f32;

                // Geo-proximity: boost matching region
                if let Some(ref req_region) = metadata.region {
                    if let Some(ref ep_region) = c.endpoint.region {
                        if ep_region == req_region {
                            bonus += 100.0;
                        }
                    }
                }

                // Quota-awareness: penalise candidates near rate limit
                let effective_rpm = c.account.rate_limit.as_ref().and_then(|rl| rl.rpm)
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

    // ── L3: Account selection ──

    /// Apply L3 account-level selection: affinity routing, cost isolation.
    fn select_account(
        &self,
        candidates: &[&ModelCandidate],
        metadata: &RouteMetadata,
    ) -> Result<usize, RouterError> {
        if candidates.is_empty() {
            return Err(RouterError::NoCandidates);
        }

        // Cost isolation: tenant_id → billing_tag
        if let Some(ref tenant_id) = metadata.tenant_id {
            for (i, c) in candidates.iter().enumerate() {
                if let Some(ref tag) = c.account.billing_tag {
                    if tag == tenant_id {
                        return Ok(i);
                    }
                }
            }
        }

        // Affinity routing: consistent hash on affinity_key
        if let Some(ref affinity_key) = metadata.affinity_key {
            let mut hasher = DefaultHasher::new();
            affinity_key.hash(&mut hasher);
            let hash_val = hasher.finish();

            // Deduplicate by account id
            let mut seen_accounts: Vec<(usize, &str)> = Vec::new();
            for (i, c) in candidates.iter().enumerate() {
                if !seen_accounts.iter().any(|(_, aid)| *aid == c.account.id.as_str()) {
                    seen_accounts.push((i, &c.account.id));
                }
            }

            if !seen_accounts.is_empty() {
                let target_idx = (hash_val as usize) % seen_accounts.len();
                let (candidate_idx, _) = seen_accounts[target_idx];

                // Check if target is overloaded (soft affinity migration)
                if self.is_account_overloaded(candidates[candidate_idx]) {
                    for (i, c) in candidates.iter().enumerate() {
                        if i != candidate_idx && !self.is_account_overloaded(c) {
                            return Ok(i);
                        }
                    }
                }

                return Ok(candidate_idx);
            }
        }

        Ok(0)
    }

    /// Check if a candidate's account is overloaded (>= 90% of rate limit).
    fn is_account_overloaded(&self, candidate: &ModelCandidate) -> bool {
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

    // ── L4: Variant selection ──

    fn apply_variant_strategy<'a>(
        &self,
        ranked: &[&'a ModelCandidate],
    ) -> &'a ModelCandidate {
        match &self.variant_strategy {
            VariantSelectionStrategy::Default | VariantSelectionStrategy::BestPerformance { .. } => {
                ranked[0]
            }
            VariantSelectionStrategy::Canary {
                canary_variant_id,
                canary_pct,
            } => {
                let mut rng = rand::thread_rng();
                let roll: f32 = rng.gen();

                if roll < *canary_pct {
                    if let Some(canary) = ranked.iter().find(|c| c.variant.id == *canary_variant_id)
                    {
                        return canary;
                    }
                }
                ranked
                    .iter()
                    .find(|c| c.variant.id != *canary_variant_id)
                    .unwrap_or(&ranked[0])
            }
            VariantSelectionStrategy::Weighted => {
                let total_weight: f32 = ranked.iter().map(|c| c.variant.weight).sum();
                if total_weight <= 0.0 {
                    return ranked[0];
                }
                let mut rng = rand::thread_rng();
                let mut roll: f32 = rng.gen::<f32>() * total_weight;
                for c in ranked {
                    roll -= c.variant.weight;
                    if roll <= 0.0 {
                        return c;
                    }
                }
                ranked.last().unwrap()
            }
        }
    }

    // ── L5: Resilience ──

    pub fn record_outcome(&self, report: &OutcomeReport) {
        self.feedback.record(report);
    }

    pub fn record_failure(&self, variant_id: &str) {
        let mut state = self.circuit_breaker_state.lock().unwrap();
        let count = state.failure_counts.entry(variant_id.to_string()).or_insert(0);
        *count += 1;
    }

    pub fn reset_failures(&self, variant_id: &str) {
        let mut state = self.circuit_breaker_state.lock().unwrap();
        state.failure_counts.remove(variant_id);
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

    pub fn on_failure(
        &self,
        failed_variant_id: &str,
        error_type: FailureType,
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

        let should_retry = match &error_type {
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

    pub fn plan_hedging(&self, request: &RouteRequest) -> Result<Vec<String>, RouterError> {
        let hedging = self.resilience_policy.hedging.as_ref().ok_or_else(|| {
            RouterError::InternalError {
                message: "hedging not configured".into(),
            }
        })?;

        let profile = self.analyzer.analyze(&request.messages);
        let strategy = request
            .strategy
            .as_ref()
            .unwrap_or(&self.default_strategy);

        let pool_ids = match strategy {
            ModelSelectionStrategy::Auto { pool_ids, .. } => pool_ids.clone(),
            _ => None,
        };

        let candidates = if let Some(ref pids) = pool_ids {
            self.registry.get_candidates(pids)
        } else {
            self.registry.get_all_candidates()
        };

        if candidates.is_empty() {
            return Err(RouterError::NoCandidates);
        }

        let ranked = self.select_model(&profile, &candidates, strategy)?;

        let count = (hedging.max_parallel as usize).min(ranked.len());
        let hedging_variants: Vec<String> = ranked
            .iter()
            .take(count)
            .map(|c| c.variant.id.clone())
            .collect();

        Ok(hedging_variants)
    }
}

impl Router for PrismRouter {
    fn route(&self, request: &RouteRequest) -> Result<RoutingDecision, RouterError> {
        let start = Instant::now();

        let profile = match &request.analyze_context {
            Some(ctx) => self.analyzer.analyze_with_context(&request.messages, ctx),
            None => self.analyzer.analyze(&request.messages),
        };

        let strategy = request.strategy.as_ref().unwrap_or(&self.default_strategy);

        let pool_ids = match strategy {
            ModelSelectionStrategy::Auto { pool_ids, .. } => pool_ids.clone(),
            _ => None,
        };

        let candidates = if let Some(ref pids) = pool_ids {
            self.registry.get_candidates(pids)
        } else {
            self.registry.get_all_candidates()
        };

        if candidates.is_empty() {
            return Err(RouterError::NoCandidates);
        }

        // L1 model selection
        let ranked = self.select_model(&profile, &candidates, strategy)?;

        // L2 provider selection (geo-proximity, quota-awareness)
        let ranked = self.select_provider(ranked, &request.metadata);

        // L3 account selection (affinity, cost isolation)
        let winner_idx = self.select_account(&ranked, &request.metadata)?;

        // L4 variant selection strategy (canary, weighted)
        let winner = if matches!(self.variant_strategy, VariantSelectionStrategy::Default | VariantSelectionStrategy::BestPerformance { .. }) {
            ranked[winner_idx]
        } else {
            self.apply_variant_strategy(&ranked)
        };

        let fallback_chain: Vec<String> = ranked
            .iter()
            .enumerate()
            .filter(|(i, c)| *i != winner_idx && c.variant.id != winner.variant.id)
            .take(3)
            .map(|(_, c)| c.variant.id.clone())
            .collect();

        let decision_time = start.elapsed();

        let default_objective = RoutingObjective::default();
        let objective = match strategy {
            ModelSelectionStrategy::Auto { objective, .. } => objective,
            _ => &default_objective,
        };
        let score = self.score_candidate(objective, &profile, winner);

        Ok(RoutingDecision {
            selected_variant: winner.variant.id.clone(),
            selected_model: winner.model.id.clone(),
            selected_account: winner.account.id.clone(),
            selected_provider: winner.provider.id.clone(),
            fallback_chain,
            reason: RoutingReason {
                l1_model: format!(
                    "task={:?}, complexity={:.2}",
                    profile.task_type, profile.complexity
                ),
                l2_provider: format!("selected {}", winner.provider.id),
                l3_account: format!("selected {}", winner.account.id),
                l4_variant: format!("selected {}", winner.variant.id),
                constraints_applied: Vec::new(),
                candidates_filtered: (candidates.len() - ranked.len()) as u32,
            },
            score,
            decision_time_us: decision_time.as_micros() as u64,
        })
    }
}
