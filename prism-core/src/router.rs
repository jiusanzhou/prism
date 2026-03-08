#![allow(clippy::type_complexity)]
use std::sync::Arc;
use std::time::Instant;

use crate::analyzer::{AnalyzeContext, ChatMessage, RequestAnalyzer, RuleBasedAnalyzer};
use crate::feedback::{FeedbackCollector, OutcomeReport};
use crate::plugin::{PluginFramework, PluginRegistry, PluginState, ResilienceHandler};
use crate::plugins::affinity::AffinityPlugin;
use crate::plugins::canary::CanaryPlugin;
use crate::plugins::capability_filter::CapabilityFilterPlugin;
use crate::plugins::cost_scorer::CostScorerPlugin;
use crate::plugins::feedback_scorer::FeedbackScorerPlugin;
use crate::plugins::geo_proximity::GeoProximityPlugin;
use crate::plugins::health_filter::HealthFilterPlugin;
use crate::plugins::inference_load::InferenceLoadPlugin;
use crate::plugins::inflight_load::InFlightLoadPlugin;
use crate::plugins::latency_scorer::LatencyScorerPlugin;
use crate::plugins::pinned_model::PinnedModelPlugin;
use crate::plugins::prefix_affinity::PrefixAffinityPlugin;
use crate::plugins::protocol_filter::ProtocolFilterPlugin;
use crate::plugins::quality_scorer::QualityScorerPlugin;
use crate::plugins::quota_aware::QuotaAwarePlugin;
use crate::plugins::specialty_scorer::SpecialtyScorerPlugin;
use crate::plugins::strategy_scorer::StrategyScorerPlugin;
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
pub struct RouteRequest {
    pub messages: Vec<ChatMessage>,
    pub strategy: Option<ModelSelectionStrategy>,
    pub metadata: RouteMetadata,
    /// Optional context for the analyzer (caller type, source, tags, etc.)
    pub analyze_context: Option<AnalyzeContext>,
    /// Required protocol for this request (e.g., "openai", "anthropic").
    /// If set, only candidates whose provider supports this protocol are considered.
    pub required_protocol: Option<String>,
    /// Requested model id from client (e.g. from request body "model" field).
    /// When set, only candidates matching this model are considered.
    pub requested_model: Option<String>,
    /// In-flight request counts per variant (for load-aware scheduling).
    pub inflight_snapshot: Option<std::collections::HashMap<String, u32>>,
    /// Shared prefix cache index for cache-aware routing.
    pub prefix_cache_index: Option<std::sync::Arc<dyn crate::prefix_cache::PrefixCacheIndex>>,
}

impl std::fmt::Debug for RouteRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouteRequest")
            .field("messages", &format!("[{} msgs]", self.messages.len()))
            .field("requested_model", &self.requested_model)
            .field("has_inflight", &self.inflight_snapshot.is_some())
            .field("has_prefix_cache", &self.prefix_cache_index.is_some())
            .finish()
    }
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
    /// Explicit prefix key for prefix-cache affinity routing.
    /// When set, overrides automatic prefix extraction from messages.
    #[serde(default)]
    pub prefix_key: Option<String>,
}

/// Errors returned by the router.

#[derive(Debug)]
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

    // Inference-aware scheduling plugins
    registry.register(Arc::new(PrefixAffinityPlugin::new()), 1);
    registry.register(Arc::new(InferenceLoadPlugin::new()), 1);
    registry.register(Arc::new(InFlightLoadPlugin::new()), 10);

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

    /// Per-model strategy overrides (model_id → strategy).
    pub model_overrides: Vec<(String, ModelSelectionStrategy)>,
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
            model_overrides: Vec::new(),
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

    pub fn with_model_overrides(
        mut self,
        overrides: Vec<(String, ModelSelectionStrategy)>,
    ) -> Self {
        self.model_overrides = overrides;
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

        let ctx = RouteContext {
            profile: &profile,
            metadata: &request.metadata,
            strategy,
            required_protocol: request.required_protocol.as_deref(),
        };

        let ranked_indices = self.l1.select(&candidates, &ctx)?;
        let ranked: Vec<&ModelCandidate> = ranked_indices.iter().map(|&i| &candidates[i]).collect();

        let max_parallel = self.get_hedging_max_parallel();

        Ok(self.resilience.plan_hedging(&ranked, max_parallel))
    }

    /// Try to extract hedging max_parallel. Defaults to 2.
    fn get_hedging_max_parallel(&self) -> u32 {
        2
    }

    /// Get all candidates for a given strategy (resolves pools).
    fn get_strategy_candidates(&self, strategy: &ModelSelectionStrategy) -> Vec<ModelCandidate> {
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

        let strategy = request.strategy.as_ref().unwrap_or_else(|| {
            // Check per-model overrides if a specific model was requested
            if let Some(ref model_id) = request.requested_model {
                if let Some((_, override_strategy)) =
                    self.model_overrides.iter().find(|(mid, _)| mid == model_id)
                {
                    return override_strategy;
                }
            }
            &self.default_strategy
        });

        // Get candidates
        let candidates = self.get_strategy_candidates(strategy);

        // Filter by requested model if specified
        let candidates = if let Some(ref model_id) = request.requested_model {
            candidates
                .into_iter()
                .filter(|c| c.model.id == *model_id)
                .collect()
        } else {
            candidates
        };
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

        // Store messages for plugins that need raw content (e.g. PrefixAffinityPlugin)
        state.set("request_messages", request.messages.clone());

        // Inject in-flight snapshot for load-aware routing
        if let Some(ref snapshot) = request.inflight_snapshot {
            state.set(
                crate::plugins::inflight_load::INFLIGHT_SNAPSHOT_KEY,
                snapshot.clone(),
            );
        }

        // Inject prefix cache index for cache-aware routing
        if let Some(ref index) = request.prefix_cache_index {
            state.set::<std::sync::Arc<dyn crate::prefix_cache::PrefixCacheIndex>>(
                "prefix_cache_index",
                index.clone(),
            );
        }

        // Run the plugin framework
        let result = self.framework.run_routing(&mut state, &candidates)?;

        let winner = &candidates[result.selected_idx];
        let decision_time = start.elapsed();

        // Record routing result in prefix cache index
        if let Some(ref index) = request.prefix_cache_index {
            if let Some(ref base_url) = winner.variant.base_url {
                if let Some(hashes) = state.get::<Vec<u64>>("prefix_hashes_for_record") {
                    if !hashes.is_empty() {
                        index.record(hashes.as_slice(), &winner.model.id, base_url);
                    }
                }
            }
        }

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
            pd_decision: {
                // Build P/D decision from variant metadata
                let variant_meta: Vec<(String, std::collections::HashMap<String, String>)> =
                    candidates
                        .iter()
                        .map(|c| (c.variant.id.clone(), c.variant.metadata.clone()))
                        .collect();
                let pd_result = crate::plugins::pd_routing::group_pd_variants(&variant_meta);

                if pd_result.pd_groups.is_empty() {
                    None // No P/D split variants, normal routing
                } else {
                    let all_meta: std::collections::HashMap<
                        String,
                        std::collections::HashMap<String, String>,
                    > = variant_meta.into_iter().collect();
                    Some(pd_result.resolve(&winner.variant.id, &all_meta))
                }
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ── Helper: build a minimal but complete registry ──

    fn make_provider(id: &str) -> Provider {
        Provider {
            id: id.into(),
            name: format!("Provider {id}"),
            auth: AuthConfig::default(),
            base_url: "https://api.example.com".into(),
            rate_limit: None,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
            protocols: vec![],
        }
    }

    fn make_endpoint(id: &str, provider_id: &str, region: Option<&str>) -> Endpoint {
        Endpoint {
            id: id.into(),
            provider_id: provider_id.into(),
            base_url: None,
            region: region.map(|r| r.into()),
            auth_override: None,
            rate_limit: None,
            priority: 0,
            status: HealthStatus::Healthy,
        }
    }

    fn make_account(id: &str, endpoint_id: &str) -> Account {
        Account {
            id: id.into(),
            endpoint_id: endpoint_id.into(),
            auth: AuthConfig::default(),
            project_id: None,
            rate_limit: None,
            billing_tag: None,
            weight: 1.0,
            status: HealthStatus::Healthy,
            metadata: HashMap::new(),
        }
    }

    fn make_model(id: &str, tier: QualityTier) -> Model {
        Model {
            id: id.into(),
            display_name: format!("Model {id}"),
            quality_tier: tier,
            capabilities: ModelCapabilities {
                max_context_window: 128_000,
                max_output_tokens: 4_096,
                supports_streaming: true,
                supports_function_calling: true,
                supports_vision: false,
                supports_json_mode: true,
                supports_reasoning: false,
            },
            pricing: ModelPricing {
                input_per_1m_tokens: 3.0,
                output_per_1m_tokens: 15.0,
                currency: "USD".into(),
            },
            specialties: vec![],
            limits: None,
            status: ModelStatus::Available,
        }
    }

    fn make_variant(id: &str, model_id: &str, endpoint_id: &str, account_id: &str) -> ModelVariant {
        ModelVariant {
            id: id.into(),
            model_id: model_id.into(),
            endpoint_id: endpoint_id.into(),
            account_id: Some(account_id.into()),
            deployment_name: None,
            weight: 1.0,
            performance: ModelPerformance::default(),
            pricing_override: None,
            status: VariantStatus::Active,
            base_url: None,
            upstream_model_name: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    fn make_pool(id: &str, variant_ids: Vec<&str>) -> ModelPool {
        ModelPool {
            id: id.into(),
            name: format!("Pool {id}"),
            variants: variant_ids.into_iter().map(|s| s.into()).collect(),
            selection_mode: SelectionMode::Scored,
            description: None,
        }
    }

    /// Build a registry with a single complete chain:
    /// provider-a -> endpoint-a -> account-a -> model-a -> variant-a -> pool-a
    fn setup_registry() -> InMemoryRegistry {
        let mut reg = InMemoryRegistry::new();
        reg.add_provider(make_provider("provider-a"));
        reg.add_endpoint(make_endpoint("endpoint-a", "provider-a", None));
        reg.add_account(make_account("account-a", "endpoint-a"));
        reg.add_model(make_model("model-a", QualityTier::A));
        reg.add_variant(make_variant(
            "variant-a",
            "model-a",
            "endpoint-a",
            "account-a",
        ));
        reg.add_pool(make_pool("pool-a", vec!["variant-a"]));
        reg
    }

    fn basic_request() -> RouteRequest {
        RouteRequest {
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "hello".into(),
            }],
            strategy: None,
            metadata: RouteMetadata::default(),
            analyze_context: None,
            required_protocol: None,
            requested_model: None,
            inflight_snapshot: None,
            prefix_cache_index: None,
        }
    }

    // ── Tests ──

    #[test]
    fn test_router_route_basic() {
        let registry = setup_registry();
        let router = PrismRouter::new(registry);

        let decision = router
            .route(&basic_request())
            .expect("routing should succeed");

        assert_eq!(decision.selected_model, "model-a");
        assert_eq!(decision.selected_variant, "variant-a");
        assert_eq!(decision.selected_account, "account-a");
        assert_eq!(decision.selected_provider, "provider-a");
        assert!(!decision.reason.l1_model.is_empty());
        // decision_time_us is u64, always >= 0
    }

    #[test]
    fn test_router_route_pinned_model() {
        let mut reg = setup_registry();

        // Add a second chain: model-b via provider-a/endpoint-a/account-a
        reg.add_model(make_model("model-b", QualityTier::B));
        reg.add_variant(make_variant(
            "variant-b",
            "model-b",
            "endpoint-a",
            "account-a",
        ));
        reg.add_pool(make_pool("pool-ab", vec!["variant-a", "variant-b"]));

        let router = PrismRouter::new(reg);

        let request = RouteRequest {
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "hello".into(),
            }],
            strategy: Some(ModelSelectionStrategy::Pinned {
                model_id: "model-b".into(),
            }),
            metadata: RouteMetadata::default(),
            analyze_context: None,
            required_protocol: None,
            requested_model: None,
            inflight_snapshot: None,
            prefix_cache_index: None,
        };

        let decision = router
            .route(&request)
            .expect("pinned routing should succeed");
        assert_eq!(
            decision.selected_model, "model-b",
            "pinned strategy must select the requested model"
        );
        assert_eq!(decision.selected_variant, "variant-b");
    }

    #[test]
    fn test_router_route_no_candidates() {
        // Empty registry -- no variants at all.
        let reg = InMemoryRegistry::new();
        let router = PrismRouter::new(reg);

        let result = router.route(&basic_request());
        assert!(result.is_err(), "expected NoCandidates error");

        match result.unwrap_err() {
            RouterError::NoCandidates => {} // correct
            other => panic!("expected NoCandidates, got: {other:?}"),
        }
    }

    #[test]
    fn test_router_route_with_metadata() {
        let mut reg = InMemoryRegistry::new();

        // Provider shared by both endpoints.
        reg.add_provider(make_provider("provider-geo"));

        // Endpoint in us-east
        reg.add_endpoint(make_endpoint("ep-us-east", "provider-geo", Some("us-east")));
        reg.add_account(make_account("acct-us-east", "ep-us-east"));
        reg.add_model(make_model("model-geo", QualityTier::A));
        reg.add_variant(make_variant(
            "var-us-east",
            "model-geo",
            "ep-us-east",
            "acct-us-east",
        ));

        // Endpoint in eu-west
        reg.add_endpoint(make_endpoint("ep-eu-west", "provider-geo", Some("eu-west")));
        reg.add_account(make_account("acct-eu-west", "ep-eu-west"));
        reg.add_variant(make_variant(
            "var-eu-west",
            "model-geo",
            "ep-eu-west",
            "acct-eu-west",
        ));

        reg.add_pool(make_pool("pool-geo", vec!["var-us-east", "var-eu-west"]));

        let router = PrismRouter::new(reg);

        // Request with region preference for us-east.
        let request = RouteRequest {
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "hello from east coast".into(),
            }],
            strategy: None,
            metadata: RouteMetadata {
                region: Some("us-east".into()),
                ..Default::default()
            },
            analyze_context: None,
            required_protocol: None,
            requested_model: None,
            inflight_snapshot: None,
            prefix_cache_index: None,
        };

        let decision = router.route(&request).expect("geo routing should succeed");

        // The GeoProximityPlugin scores exact region matches higher, so the
        // us-east variant should be selected.
        assert_eq!(
            decision.selected_variant, "var-us-east",
            "geo proximity should prefer the endpoint in the requested region"
        );
    }

    #[test]
    fn test_router_feedback_loop() {
        let registry = setup_registry();
        let router = PrismRouter::new(registry);

        // Record positive feedback for variant-a.
        router.record_outcome(&OutcomeReport {
            variant_id: "variant-a".into(),
            success: true,
            latency_ms: 120,
            prompt_tokens: Some(50),
            completion_tokens: Some(100),
            cost: Some(0.002),
            user_rating: Some(5),
        });

        // Routing should still succeed after feedback.
        let decision = router
            .route(&basic_request())
            .expect("routing after positive feedback should succeed");
        assert_eq!(decision.selected_variant, "variant-a");

        // Record negative feedback for a hypothetical other variant (won't
        // affect routing because the variant is not in the registry, but
        // exercises the feedback path).
        router.record_outcome(&OutcomeReport {
            variant_id: "variant-nonexistent".into(),
            success: false,
            latency_ms: 5000,
            prompt_tokens: Some(50),
            completion_tokens: None,
            cost: None,
            user_rating: Some(1),
        });

        // Verify the collector recorded both reports.
        let stats_a = router
            .feedback
            .get_stats("variant-a")
            .expect("stats for variant-a should exist");
        assert_eq!(stats_a.total_requests, 1);
        assert_eq!(stats_a.successful_requests, 1);
        assert_eq!(stats_a.user_ratings, vec![5]);

        let stats_bad = router
            .feedback
            .get_stats("variant-nonexistent")
            .expect("stats for nonexistent variant should exist");
        assert_eq!(stats_bad.total_requests, 1);
        assert_eq!(stats_bad.error_count, 1);
        assert_eq!(stats_bad.user_ratings, vec![1]);

        // Routing still works after mixed feedback.
        let decision2 = router
            .route(&basic_request())
            .expect("routing after mixed feedback should succeed");
        assert_eq!(decision2.selected_variant, "variant-a");
    }

    #[test]
    fn test_router_with_strategy_override() {
        let registry = setup_registry();
        let router = PrismRouter::new(registry).with_strategy(ModelSelectionStrategy::Auto {
            objective: RoutingObjective::CostFirst {
                min_quality_score: 0.0,
                budget_per_request: None,
            },
            pool_ids: None,
            required_capabilities: None,
        });

        let decision = router
            .route(&basic_request())
            .expect("routing should succeed");
        assert_eq!(decision.selected_variant, "variant-a");
    }

    #[test]
    fn test_router_per_model_override_applies() {
        let mut reg = setup_registry();

        // Add a second model + variant
        let mut cheap_model = make_model("model-cheap", QualityTier::C);
        cheap_model.pricing = ModelPricing {
            input_per_1m_tokens: 0.1,
            output_per_1m_tokens: 0.2,
            currency: "USD".into(),
        };
        reg.add_model(cheap_model);
        reg.add_variant(make_variant(
            "variant-cheap",
            "model-cheap",
            "endpoint-a",
            "account-a",
        ));
        reg.add_pool(make_pool("pool-both", vec!["variant-a", "variant-cheap"]));

        // Default strategy is balanced; override model-cheap to use cost_first
        let router = PrismRouter::new(reg).with_model_overrides(vec![(
            "model-cheap".into(),
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::CostFirst {
                    min_quality_score: 0.0,
                    budget_per_request: None,
                },
                pool_ids: None,
                required_capabilities: None,
            },
        )]);

        // Request for model-cheap should use the cost_first override
        let request = RouteRequest {
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "hello".into(),
            }],
            strategy: None,
            metadata: RouteMetadata::default(),
            analyze_context: None,
            required_protocol: None,
            requested_model: Some("model-cheap".into()),
            inflight_snapshot: None,
            prefix_cache_index: None,
        };

        let decision = router.route(&request).expect("routing should succeed");
        assert_eq!(
            decision.selected_model, "model-cheap",
            "per-model override should route to the overridden model"
        );
    }

    #[test]
    fn test_router_per_model_override_not_applied_for_other_model() {
        let mut reg = setup_registry();

        reg.add_model(make_model("model-b", QualityTier::B));
        reg.add_variant(make_variant(
            "variant-b",
            "model-b",
            "endpoint-a",
            "account-a",
        ));

        // Override only model-b
        let router = PrismRouter::new(reg).with_model_overrides(vec![(
            "model-b".into(),
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::CostFirst {
                    min_quality_score: 0.0,
                    budget_per_request: None,
                },
                pool_ids: None,
                required_capabilities: None,
            },
        )]);

        // Request for model-a (no override) should use default strategy
        let request = RouteRequest {
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "hello".into(),
            }],
            strategy: None,
            metadata: RouteMetadata::default(),
            analyze_context: None,
            required_protocol: None,
            requested_model: Some("model-a".into()),
            inflight_snapshot: None,
            prefix_cache_index: None,
        };

        let decision = router.route(&request).expect("routing should succeed");
        assert_eq!(
            decision.selected_model, "model-a",
            "non-overridden model uses default strategy"
        );
    }

    #[test]
    fn test_router_explicit_strategy_overrides_per_model() {
        let reg = setup_registry();

        // Set per-model override for model-a
        let router = PrismRouter::new(reg).with_model_overrides(vec![(
            "model-a".into(),
            ModelSelectionStrategy::Auto {
                objective: RoutingObjective::CostFirst {
                    min_quality_score: 0.0,
                    budget_per_request: None,
                },
                pool_ids: None,
                required_capabilities: None,
            },
        )]);

        // Explicit request strategy should take precedence over per-model override
        let request = RouteRequest {
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "hello".into(),
            }],
            strategy: Some(ModelSelectionStrategy::Pinned {
                model_id: "model-a".into(),
            }),
            metadata: RouteMetadata::default(),
            analyze_context: None,
            required_protocol: None,
            requested_model: Some("model-a".into()),
            inflight_snapshot: None,
            prefix_cache_index: None,
        };

        let decision = router.route(&request).expect("routing should succeed");
        assert_eq!(
            decision.selected_model, "model-a",
            "explicit strategy takes priority over per-model override"
        );
    }

    #[test]
    fn test_router_with_custom_l2_l3_l4() {
        let registry = setup_registry();
        let router = PrismRouter::new(registry)
            .with_l2(create_l2_selector("lowest_latency"))
            .with_l3(create_l3_selector("round_robin"))
            .with_l4(create_l4_selector("weighted"));

        let decision = router
            .route(&basic_request())
            .expect("routing should succeed");
        assert_eq!(decision.selected_variant, "variant-a");
    }

    #[test]
    fn test_router_with_resilience_policy() {
        let registry = setup_registry();
        let custom_policy = ResiliencePolicy {
            retry: RetryPolicy {
                max_retries: 5,
                retry_on: vec![RetryCondition::RateLimit],
                backoff: BackoffStrategy::Fixed { delay_ms: 500 },
                retry_scope: RetryScope::SameProvider,
            },
            ..ResiliencePolicy::default()
        };

        let router = PrismRouter::new(registry).with_resilience_policy(custom_policy);

        let decision = router
            .route(&basic_request())
            .expect("routing should succeed");
        assert_eq!(decision.selected_variant, "variant-a");
    }
}
