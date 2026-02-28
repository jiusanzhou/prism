use std::sync::Arc;
use std::time::Instant;

use crate::analyzer::{AnalyzeContext, ChatMessage, RequestAnalyzer, RuleBasedAnalyzer};
use crate::feedback::{FeedbackCollector, OutcomeReport};
use crate::registry::{InMemoryRegistry, ModelRegistryTrait};
use crate::selector::*;
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
    /// Required protocol for this request (e.g., "openai", "anthropic").
    /// If set, only candidates whose provider supports this protocol are considered.
    pub required_protocol: Option<String>,
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
// PrismRouter — concrete implementation
// ──────────────────────────────────────────────

pub struct PrismRouter {
    pub analyzer: Box<dyn RequestAnalyzer>,
    pub registry: Arc<InMemoryRegistry>,
    pub default_strategy: ModelSelectionStrategy,
    pub variant_strategy: VariantSelectionStrategy,

    // Extensible layer selectors
    pub l1: Box<dyn L1ModelSelector>,
    pub l2: Box<dyn L2ProviderSelector>,
    pub l3: Box<dyn L3AccountSelector>,
    pub l4: Box<dyn L4VariantSelector>,
    pub l5: Box<dyn L5ResiliencePolicy>,

    pub feedback: Arc<FeedbackCollector>,
}

impl PrismRouter {
    pub fn new(registry: InMemoryRegistry) -> Self {
        let feedback = Arc::new(FeedbackCollector::new(1000));
        let registry = Arc::new(registry);

        let l1 = Box::new(ScoredL1Selector::new(Arc::clone(&feedback)));
        let l5 = Box::new(DefaultL5Policy::new(
            ResiliencePolicy::default(),
            Arc::clone(&registry),
        ));

        Self {
            analyzer: Box::new(RuleBasedAnalyzer::new()),
            registry,
            default_strategy: ModelSelectionStrategy::default(),
            variant_strategy: VariantSelectionStrategy::Default,
            l1,
            l2: Box::new(DefaultL2Selector),
            l3: Box::new(AffinityL3Selector),
            l4: Box::new(BestL4Selector),
            l5,
            feedback,
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
        // Update L4 selector to match the variant strategy
        match &self.variant_strategy {
            VariantSelectionStrategy::Canary {
                canary_variant_id,
                canary_pct,
            } => {
                self.l4 = Box::new(CanaryL4Selector {
                    canary_variant_id: canary_variant_id.clone(),
                    canary_pct: *canary_pct,
                });
            }
            VariantSelectionStrategy::Weighted => {
                self.l4 = Box::new(WeightedL4Selector);
            }
            _ => {
                self.l4 = Box::new(BestL4Selector);
            }
        }
        self
    }

    pub fn with_resilience_policy(mut self, rp: ResiliencePolicy) -> Self {
        self.l5 = Box::new(DefaultL5Policy::new(rp, Arc::clone(&self.registry)));
        self
    }

    // Builder methods for custom layer implementations
    pub fn with_l1(mut self, l1: Box<dyn L1ModelSelector>) -> Self {
        self.l1 = l1;
        self
    }

    pub fn with_l2(mut self, l2: Box<dyn L2ProviderSelector>) -> Self {
        self.l2 = l2;
        self
    }

    pub fn with_l3(mut self, l3: Box<dyn L3AccountSelector>) -> Self {
        self.l3 = l3;
        self
    }

    pub fn with_l4(mut self, l4: Box<dyn L4VariantSelector>) -> Self {
        self.l4 = l4;
        self
    }

    pub fn with_l5(mut self, l5: Box<dyn L5ResiliencePolicy>) -> Self {
        self.l5 = l5;
        self
    }

    // ── Public helpers (backward compat) ──

    pub fn record_outcome(&self, report: &OutcomeReport) {
        self.feedback.record(report);
    }

    pub fn record_failure(&self, variant_id: &str) {
        self.l5.record_failure(variant_id);
    }

    pub fn reset_failures(&self, variant_id: &str) {
        self.l5.reset_failures(variant_id);
    }

    pub fn on_failure(
        &self,
        failed_variant_id: &str,
        error_type: FailureType,
        attempt: u32,
    ) -> Result<RecoveryAction, RouterError> {
        self.l5.on_failure(failed_variant_id, &error_type, attempt)
    }

    pub fn plan_hedging(&self, request: &RouteRequest) -> Result<Vec<String>, RouterError> {
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

        let ctx = RouteContext {
            profile: &profile,
            metadata: &request.metadata,
            strategy,
            required_protocol: request.required_protocol.as_deref(),
        };

        let ranked_indices = self.l1.select(&candidates, &ctx)?;
        let ranked: Vec<&ModelCandidate> =
            ranked_indices.iter().map(|&i| &candidates[i]).collect();

        // Determine max_parallel from the hedging config if available
        let max_parallel = self.get_hedging_max_parallel();

        Ok(self.l5.plan_hedging(&ranked, max_parallel))
    }

    /// Try to extract hedging max_parallel. Defaults to 2.
    fn get_hedging_max_parallel(&self) -> u32 {
        // For backward compat with the original plan_hedging which read from resilience_policy.hedging
        // We can't easily downcast, so default to 2.
        2
    }

    /// Compute the score for the winning candidate.
    fn score_winner(
        &self,
        objective: &RoutingObjective,
        profile: &crate::analyzer::RequestProfile,
        candidate: &ModelCandidate,
    ) -> f32 {
        ScoredL1Selector::score_candidate_with_feedback(
            objective,
            profile,
            candidate,
            &self.feedback,
        )
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

        // Protocol filtering
        let candidates: Vec<ModelCandidate> = if let Some(ref protocol) = request.required_protocol
        {
            candidates
                .into_iter()
                .filter(|c| {
                    c.provider.protocols.is_empty() || c.provider.protocols.contains(protocol)
                })
                .collect()
        } else {
            candidates
        };

        if candidates.is_empty() {
            return Err(RouterError::AllFiltered {
                reason: format!(
                    "no candidates support required protocol '{}'",
                    request.required_protocol.as_deref().unwrap_or("unknown")
                ),
            });
        }

        let ctx = RouteContext {
            profile: &profile,
            metadata: &request.metadata,
            strategy,
            required_protocol: request.required_protocol.as_deref(),
        };

        // L1 model selection
        let ranked_indices = self.l1.select(&candidates, &ctx)?;
        let ranked: Vec<&ModelCandidate> =
            ranked_indices.iter().map(|&i| &candidates[i]).collect();

        // L2 provider selection (geo-proximity, quota-awareness)
        let ranked = self.l2.rerank(ranked, &ctx);

        // L3 account selection (affinity, cost isolation)
        let winner_idx = self.l3.select(&ranked, &ctx)?;

        // L4 variant selection strategy (canary, weighted)
        let winner = if matches!(
            self.variant_strategy,
            VariantSelectionStrategy::Default | VariantSelectionStrategy::BestPerformance { .. }
        ) {
            ranked[winner_idx]
        } else {
            self.l4.select(&ranked, &ctx)
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

        let score = self.score_winner(objective, &profile, winner);

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
