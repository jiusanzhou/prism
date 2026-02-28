use std::sync::Arc;
use std::time::Instant;

use crate::analyzer::{AnalyzeContext, ChatMessage, RequestAnalyzer, RuleBasedAnalyzer};
use crate::feedback::{FeedbackCollector, OutcomeReport};
use crate::plugin::{PluginFramework, PluginRegistry, PluginState, ResilienceHandler};
use crate::plugins::affinity::AffinityPlugin;
use crate::plugins::canary::CanaryPlugin;
use crate::plugins::capability_filter::CapabilityFilterPlugin;
use crate::plugins::cost_scorer::CostScorerPlugin;
use crate::plugins::geo_proximity::GeoProximityPlugin;
use crate::plugins::feedback_scorer::FeedbackScorerPlugin;
use crate::plugins::health_filter::HealthFilterPlugin;
use crate::plugins::strategy_scorer::StrategyScorerPlugin;
use crate::plugins::latency_scorer::LatencyScorerPlugin;
use crate::plugins::pinned_model::PinnedModelPlugin;
use crate::plugins::protocol_filter::ProtocolFilterPlugin;
use crate::plugins::quality_scorer::QualityScorerPlugin;
use crate::plugins::quota_aware::QuotaAwarePlugin;
use crate::plugins::specialty_scorer::SpecialtyScorerPlugin;
use crate::plugins::weighted::WeightedPlugin;
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
// Default plugin profile
// ──────────────────────────────────────────────

/// Create the default plugin profile that approximates the original L1–L5 behavior.
///
/// Filter plugins:
///   - ProtocolFilter, CapabilityFilter, HealthFilter, PinnedModel
///
/// Score plugins (with weights approximating Balanced objective):
///   - QualityScorer (5), CostScorer (3), LatencyScorer (2),
///     SpecialtyScorer (4), GeoProximity (1), QuotaAware (2), Affinity (1),
///     FeedbackScorer (3), StrategyScorer (10)
pub fn default_plugin_profile(feedback: Arc<FeedbackCollector>) -> PluginRegistry {
    let mut registry = PluginRegistry::new();

    // Filter plugins (score_weight irrelevant, but defaults to 1)
    registry.register_default(Arc::new(ProtocolFilterPlugin));
    registry.register_default(Arc::new(CapabilityFilterPlugin));
    registry.register_default(Arc::new(HealthFilterPlugin));
    registry.register_default(Arc::new(PinnedModelPlugin));

    // Strategy-aware scorer (dominant weight — provides the primary scoring signal)
    registry.register(Arc::new(StrategyScorerPlugin), 50);

    // Supplementary score plugins with lower weights
    registry.register(Arc::new(QualityScorerPlugin), 2);
    registry.register(Arc::new(CostScorerPlugin), 2);
    registry.register(Arc::new(LatencyScorerPlugin), 1);
    registry.register(Arc::new(SpecialtyScorerPlugin), 2);
    registry.register(Arc::new(GeoProximityPlugin), 1);
    registry.register(Arc::new(QuotaAwarePlugin), 1);
    registry.register(Arc::new(AffinityPlugin), 1);
    registry.register(Arc::new(FeedbackScorerPlugin::new(feedback)), 2);

    registry
}

// ──────────────────────────────────────────────
// DefaultResilienceHandler — wraps the old L5
// ──────────────────────────────────────────────

/// Default resilience handler that wraps a [`DefaultL5Policy`].
struct DefaultResilienceHandler {
    l5: DefaultL5Policy,
}

impl ResilienceHandler for DefaultResilienceHandler {
    fn on_failure(
        &self,
        failed_variant_id: &str,
        error_type: &FailureType,
        attempt: u32,
    ) -> Result<RecoveryAction, RouterError> {
        L5ResiliencePolicy::on_failure(&self.l5, failed_variant_id, error_type, attempt)
    }

    fn plan_hedging(&self, candidates: &[&ModelCandidate], max: u32) -> Vec<String> {
        L5ResiliencePolicy::plan_hedging(&self.l5, candidates, max)
    }

    fn record_failure(&self, variant_id: &str) {
        L5ResiliencePolicy::record_failure(&self.l5, variant_id);
    }

    fn reset_failures(&self, variant_id: &str) {
        L5ResiliencePolicy::reset_failures(&self.l5, variant_id);
    }
}

// ──────────────────────────────────────────────
// PrismRouter — concrete implementation
// ──────────────────────────────────────────────

pub struct PrismRouter {
    pub analyzer: Box<dyn RequestAnalyzer>,
    pub registry: Arc<InMemoryRegistry>,
    pub default_strategy: ModelSelectionStrategy,
    pub variant_strategy: VariantSelectionStrategy,

    // ── Plugin Framework ──
    pub framework: PluginFramework,
    pub resilience: Box<dyn ResilienceHandler>,

    // ── Legacy layer selectors (kept for backward compat) ──
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

        // Build default plugin framework
        let plugin_registry = default_plugin_profile(Arc::clone(&feedback));
        let framework = PluginFramework::new(plugin_registry);

        // Build default resilience handler
        let resilience: Box<dyn ResilienceHandler> = Box::new(DefaultResilienceHandler {
            l5: DefaultL5Policy::new(ResiliencePolicy::default(), Arc::clone(&registry)),
        });

        Self {
            analyzer: Box::new(RuleBasedAnalyzer::new()),
            registry,
            default_strategy: ModelSelectionStrategy::default(),
            variant_strategy: VariantSelectionStrategy::Default,
            framework,
            resilience,
            l1,
            l2: Box::new(DefaultL2Selector),
            l3: Box::new(AffinityL3Selector),
            l4: Box::new(BestL4Selector),
            l5,
            feedback,
        }
    }

    /// Create a router with a custom plugin registry.
    pub fn with_plugins(mut self, registry: PluginRegistry) -> Self {
        self.framework = PluginFramework::new(registry);
        self
    }

    /// Create a router with a custom resilience handler.
    pub fn with_resilience(mut self, handler: Box<dyn ResilienceHandler>) -> Self {
        self.resilience = handler;
        self
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
        // Update L4 selector to match the variant strategy (legacy)
        match &self.variant_strategy {
            VariantSelectionStrategy::Canary {
                canary_variant_id,
                canary_pct,
            } => {
                self.l4 = Box::new(CanaryL4Selector {
                    canary_variant_id: canary_variant_id.clone(),
                    canary_pct: *canary_pct,
                });
                // Rebuild plugin framework with canary plugin
                let mut plugin_registry = default_plugin_profile(Arc::clone(&self.feedback));
                plugin_registry.register(
                    Arc::new(CanaryPlugin::with_canary(
                        canary_variant_id.clone(),
                        *canary_pct,
                    )),
                    1000, // dominant weight to override other scorers
                );
                self.framework = PluginFramework::new(plugin_registry);
            }
            VariantSelectionStrategy::Weighted => {
                self.l4 = Box::new(WeightedL4Selector);
                // Rebuild plugin framework with weighted plugin
                let mut plugin_registry = default_plugin_profile(Arc::clone(&self.feedback));
                plugin_registry.register(
                    Arc::new(WeightedPlugin),
                    1000, // dominant weight
                );
                self.framework = PluginFramework::new(plugin_registry);
            }
            _ => {
                self.l4 = Box::new(BestL4Selector);
            }
        }
        self
    }

    pub fn with_resilience_policy(mut self, rp: ResiliencePolicy) -> Self {
        self.l5 = Box::new(DefaultL5Policy::new(rp.clone(), Arc::clone(&self.registry)));
        self.resilience = Box::new(DefaultResilienceHandler {
            l5: DefaultL5Policy::new(rp, Arc::clone(&self.registry)),
        });
        self
    }

    // Builder methods for custom layer implementations (backward compat)
    #[deprecated(note = "Use with_plugins() to configure routing via plugin framework")]
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
        self.resilience.record_failure(variant_id);
    }

    pub fn reset_failures(&self, variant_id: &str) {
        self.resilience.reset_failures(variant_id);
    }

    pub fn on_failure(
        &self,
        failed_variant_id: &str,
        error_type: FailureType,
        attempt: u32,
    ) -> Result<RecoveryAction, RouterError> {
        self.resilience
            .on_failure(failed_variant_id, &error_type, attempt)
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

        let max_parallel = self.get_hedging_max_parallel();

        Ok(self
            .resilience
            .plan_hedging(&ranked, max_parallel))
    }

    /// Try to extract hedging max_parallel. Defaults to 2.
    fn get_hedging_max_parallel(&self) -> u32 {
        2
    }

    /// Get all candidates for a given strategy (resolves pools).
    fn get_strategy_candidates(
        &self,
        strategy: &ModelSelectionStrategy,
    ) -> Vec<ModelCandidate> {
        let pool_ids = match strategy {
            ModelSelectionStrategy::Auto { pool_ids, .. } => pool_ids.clone(),
            _ => None,
        };

        if let Some(ref pids) = pool_ids {
            self.registry.get_candidates(pids)
        } else {
            self.registry.get_all_candidates()
        }
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

        // Get candidates
        let candidates = self.get_strategy_candidates(strategy);
        if candidates.is_empty() {
            return Err(RouterError::NoCandidates);
        }

        // Build PluginState
        let mut state = PluginState::new(
            profile.clone(),
            request.metadata.clone(),
            strategy.clone(),
            request.required_protocol.clone(),
        );

        // Run the plugin framework
        let result = self.framework.run_routing(&mut state, &candidates)?;

        let winner = &candidates[result.selected_idx];
        let decision_time = start.elapsed();

        Ok(RoutingDecision {
            selected_variant: winner.variant.id.clone(),
            selected_model: winner.model.id.clone(),
            selected_account: winner.account.id.clone(),
            selected_provider: winner.provider.id.clone(),
            fallback_chain: result.fallback_chain,
            reason: RoutingReason {
                l1_model: format!(
                    "task={:?}, complexity={:.2}",
                    profile.task_type, profile.complexity
                ),
                l2_provider: format!("selected {}", winner.provider.id),
                l3_account: format!("selected {}", winner.account.id),
                l4_variant: format!("selected {}", winner.variant.id),
                constraints_applied: Vec::new(),
                candidates_filtered: 0,
            },
            score: result.score,
            decision_time_us: decision_time.as_micros() as u64,
        })
    }
}
