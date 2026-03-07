use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ──────────────────────────────────────────────
// L1 — Model selection strategy
// ──────────────────────────────────────────────

/// High-level routing objective
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RoutingObjective {
    /// Latency / SLO must be met
    SloFirst {
        max_latency_ms: u32,
        max_ttft_ms: u32,
        min_quality_score: f32,
    },
    /// Cheapest option that meets a quality floor
    CostFirst {
        min_quality_score: f32,
        budget_per_request: Option<f64>,
    },
    /// Best quality regardless of cost
    QualityFirst { max_latency_ms: Option<u32> },
    /// Weighted composite
    Balanced {
        cost_weight: f32,
        quality_weight: f32,
        latency_weight: f32,
    },
    /// Plug-in scorer
    Custom { scorer_id: String },
}

impl Default for RoutingObjective {
    fn default() -> Self {
        Self::Balanced {
            cost_weight: 0.3,
            quality_weight: 0.5,
            latency_weight: 0.2,
        }
    }
}

/// Hard capability constraints – models that do not satisfy are filtered out.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequiredCapabilities {
    pub min_context_window: Option<u32>,
    pub function_calling: Option<bool>,
    pub vision: Option<bool>,
    pub json_mode: Option<bool>,
    pub reasoning: Option<bool>,
}

/// One tier in a cascade strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CascadeTier {
    pub pool_id: String,
    pub quality_threshold: f32,
}

/// How the model is chosen for a request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelSelectionStrategy {
    /// User explicitly pinned a model id
    Pinned { model_id: String },
    /// Automatic selection via scoring
    Auto {
        objective: RoutingObjective,
        pool_ids: Option<Vec<String>>,
        required_capabilities: Option<RequiredCapabilities>,
    },
    /// Cascade: try cheaper tier first, upgrade on quality miss
    Cascade { tiers: Vec<CascadeTier> },
}

impl Default for ModelSelectionStrategy {
    fn default() -> Self {
        Self::Auto {
            objective: RoutingObjective::default(),
            pool_ids: None,
            required_capabilities: None,
        }
    }
}

// ──────────────────────────────────────────────
// L2 — Provider selection strategy
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum ProviderSelectionStrategy {
    Priority,
    LowestCost,
    #[default]
    LowestLatency,
    WeightedRoundRobin,
    GeoProximity {
        region_preferences: Vec<String>,
    },
    QuotaAware,
    Scored {
        cost_weight: f32,
        latency_weight: f32,
        availability_weight: f32,
        quota_weight: f32,
    },
}

// ──────────────────────────────────────────────
// L3 — Account selection strategy
// ──────────────────────────────────────────────

/// Where to extract the affinity key from
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AffinityKeySource {
    Header(String),
    ApiKey,
    ConversationHash,
    Custom(String),
}

/// How strongly to honour affinity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AffinityStrategy {
    None,
    ConsistentHash {
        key_source: AffinityKeySource,
    },
    Soft {
        key_source: AffinityKeySource,
        max_load_ratio: f32,
    },
    Hard {
        key_source: AffinityKeySource,
    },
}

/// Persisted binding (would be stored in Redis in production)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffinityBinding {
    pub affinity_key: String,
    pub account_id: String,
    pub model_id: String,
    pub ttl_seconds: u64,
}

/// How to select among accounts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccountSelectionStrategy {
    AffinityFirst {
        affinity: AffinityStrategy,
        fallback: Box<AccountSelectionStrategy>,
    },
    QuotaBalanced,
    LoadBalanced {
        algorithm: LoadBalanceAlgorithm,
    },
    CostIsolation {
        tenant_key: AffinityKeySource,
        tenant_account_map: HashMap<String, String>,
    },
    WeightedRandom,
}

impl Default for AccountSelectionStrategy {
    fn default() -> Self {
        Self::LoadBalanced {
            algorithm: LoadBalanceAlgorithm::RoundRobin,
        }
    }
}

/// Load-balancing algorithms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LoadBalanceAlgorithm {
    RoundRobin,
    LeastConnections,
    LeastLoad,
    PowerOfTwoChoices,
}

// ──────────────────────────────────────────────
// L4 — Variant selection strategy
// ──────────────────────────────────────────────

/// Which performance dimension to optimise
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PerformanceMetric {
    LatencyP50,
    LatencyP99,
    TTFT,
    Throughput,
    ErrorRate,
    CompositeScore,
}

/// How the final variant is chosen
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum VariantSelectionStrategy {
    BestPerformance {
        metric: PerformanceMetric,
    },
    Weighted,
    Canary {
        canary_variant_id: String,
        canary_pct: f32,
    },
    #[default]
    Default,
}

// ──────────────────────────────────────────────
// L5 — Resilience policies
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RetryScope {
    SameVariant,
    SameAccount,
    SameProvider,
    CrossProvider,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RetryCondition {
    HttpStatus(Vec<u16>),
    Timeout,
    ConnectionError,
    RateLimit,
    ModelOverloaded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackoffStrategy {
    Fixed { delay_ms: u32 },
    Exponential { base_ms: u32, max_ms: u32 },
    Jittered { base_ms: u32, max_ms: u32 },
}

impl Default for BackoffStrategy {
    fn default() -> Self {
        Self::Exponential {
            base_ms: 100,
            max_ms: 5000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub retry_on: Vec<RetryCondition>,
    pub backoff: BackoffStrategy,
    pub retry_scope: RetryScope,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            retry_on: vec![RetryCondition::RateLimit, RetryCondition::Timeout],
            backoff: BackoffStrategy::default(),
            retry_scope: RetryScope::CrossProvider,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FallbackTarget {
    Variant(String),
    Model(String),
    Pool(String),
    CachedResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackPolicy {
    pub chain: Vec<FallbackTarget>,
    pub allow_model_downgrade: bool,
    pub notify_client: bool,
}

impl Default for FallbackPolicy {
    fn default() -> Self {
        Self {
            chain: Vec::new(),
            allow_model_downgrade: true,
            notify_client: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerPolicy {
    pub error_threshold_pct: f32,
    pub window_seconds: u32,
    pub min_requests: u32,
    pub open_duration_seconds: u32,
    pub half_open_requests: u32,
}

impl Default for CircuitBreakerPolicy {
    fn default() -> Self {
        Self {
            error_threshold_pct: 50.0,
            window_seconds: 60,
            min_requests: 10,
            open_duration_seconds: 30,
            half_open_requests: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HedgingPolicy {
    pub max_parallel: u32,
    pub delay_ms: u32,
    pub cancel_remaining: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutPolicy {
    pub connect_ms: u32,
    pub request_ms: u32,
    pub ttft_ms: Option<u32>,
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        Self {
            connect_ms: 3000,
            request_ms: 30000,
            ttft_ms: Some(10000),
        }
    }
}

/// Composite resilience configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResiliencePolicy {
    pub retry: RetryPolicy,
    pub fallback: FallbackPolicy,
    pub circuit_breaker: CircuitBreakerPolicy,
    pub timeout: TimeoutPolicy,
    pub hedging: Option<HedgingPolicy>,
}

// ──────────────────────────────────────────────
// L5 — Failure types and recovery actions
// ──────────────────────────────────────────────

/// Classification of why a variant failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FailureType {
    /// HTTP error status code (e.g. 500, 502, 429)
    HttpError(u16),
    /// Request timed out entirely
    Timeout,
    /// Could not connect to upstream
    ConnectionError,
    /// Upstream returned rate-limit (429 or equivalent)
    RateLimit,
    /// Time-to-first-token exceeded the configured threshold
    TTFTTimeout,
}

/// What the router recommends after a variant failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryAction {
    /// Retry the same or another variant after a delay.
    Retry { variant_id: String, delay_ms: u32 },
    /// Fall back to a different variant (retries exhausted or circuit open).
    Fallback { variant_id: String },
    /// No more options — reject the request.
    Reject { reason: String },
}

// ──────────────────────────────────────────────
// Routing decision (output)
// ──────────────────────────────────────────────

/// Human-readable explanations per layer
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingReason {
    pub l1_model: String,
    pub l2_provider: String,
    pub l3_account: String,
    pub l4_variant: String,
    pub constraints_applied: Vec<String>,
    pub candidates_filtered: u32,
}

/// The final routing decision produced by PrismRouter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub selected_variant: String,
    pub selected_model: String,
    pub selected_account: String,
    pub selected_provider: String,
    pub fallback_chain: Vec<String>,
    pub reason: RoutingReason,
    pub score: f32,
    pub decision_time_us: u64,
    /// P/D disaggregation decision. None for colocated/normal routing.
    pub pd_decision: Option<crate::plugins::pd_routing::PDDecision>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routing_objective_default() {
        let obj = RoutingObjective::default();
        match obj {
            RoutingObjective::Balanced {
                cost_weight,
                quality_weight,
                latency_weight,
            } => {
                assert!((cost_weight - 0.3).abs() < f32::EPSILON);
                assert!((quality_weight - 0.5).abs() < f32::EPSILON);
                assert!((latency_weight - 0.2).abs() < f32::EPSILON);
            }
            _ => panic!("expected Balanced"),
        }
    }

    #[test]
    fn test_model_selection_strategy_default() {
        let s = ModelSelectionStrategy::default();
        match s {
            ModelSelectionStrategy::Auto {
                objective,
                pool_ids,
                required_capabilities,
            } => {
                assert!(pool_ids.is_none());
                assert!(required_capabilities.is_none());
                assert!(matches!(objective, RoutingObjective::Balanced { .. }));
            }
            _ => panic!("expected Auto"),
        }
    }

    #[test]
    fn test_provider_selection_strategy_default() {
        let s = ProviderSelectionStrategy::default();
        assert!(matches!(s, ProviderSelectionStrategy::LowestLatency));
    }

    #[test]
    fn test_account_selection_strategy_default() {
        let s = AccountSelectionStrategy::default();
        match s {
            AccountSelectionStrategy::LoadBalanced { algorithm } => {
                assert!(matches!(algorithm, LoadBalanceAlgorithm::RoundRobin));
            }
            _ => panic!("expected LoadBalanced(RoundRobin)"),
        }
    }

    #[test]
    fn test_variant_selection_strategy_default() {
        let s = VariantSelectionStrategy::default();
        assert!(matches!(s, VariantSelectionStrategy::Default));
    }

    #[test]
    fn test_retry_policy_default() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 2);
        assert!(matches!(p.retry_scope, RetryScope::CrossProvider));
        assert!(p
            .retry_on
            .iter()
            .any(|c| matches!(c, RetryCondition::RateLimit)));
        assert!(p
            .retry_on
            .iter()
            .any(|c| matches!(c, RetryCondition::Timeout)));
    }

    #[test]
    fn test_backoff_strategy_default() {
        let b = BackoffStrategy::default();
        match b {
            BackoffStrategy::Exponential { base_ms, max_ms } => {
                assert_eq!(base_ms, 100);
                assert_eq!(max_ms, 5000);
            }
            _ => panic!("expected Exponential"),
        }
    }

    #[test]
    fn test_resilience_policy_default() {
        let rp = ResiliencePolicy::default();
        assert_eq!(rp.retry.max_retries, 2);
        assert!((rp.circuit_breaker.error_threshold_pct - 50.0).abs() < f32::EPSILON);
        assert_eq!(rp.circuit_breaker.window_seconds, 60);
        assert_eq!(rp.circuit_breaker.min_requests, 10);
        assert_eq!(rp.circuit_breaker.open_duration_seconds, 30);
        assert_eq!(rp.circuit_breaker.half_open_requests, 3);
        assert_eq!(rp.timeout.connect_ms, 3000);
        assert_eq!(rp.timeout.request_ms, 30000);
        assert_eq!(rp.timeout.ttft_ms, Some(10000));
        assert!(rp.fallback.chain.is_empty());
        assert!(rp.fallback.allow_model_downgrade);
        assert!(rp.fallback.notify_client);
        assert!(rp.hedging.is_none());
    }

    #[test]
    fn test_serde_roundtrip_routing_objective() {
        let variants = vec![
            RoutingObjective::Balanced {
                cost_weight: 0.3,
                quality_weight: 0.5,
                latency_weight: 0.2,
            },
            RoutingObjective::CostFirst {
                min_quality_score: 0.5,
                budget_per_request: Some(0.01),
            },
            RoutingObjective::QualityFirst {
                max_latency_ms: Some(500),
            },
            RoutingObjective::SloFirst {
                max_latency_ms: 200,
                max_ttft_ms: 100,
                min_quality_score: 0.7,
            },
            RoutingObjective::Custom {
                scorer_id: "my_scorer".into(),
            },
        ];
        for obj in &variants {
            let json = serde_json::to_string(obj).unwrap();
            let back: RoutingObjective = serde_json::from_str(&json).unwrap();
            // Verify via re-serialisation equality
            let json2 = serde_json::to_string(&back).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn test_serde_roundtrip_provider_strategy() {
        let variants = vec![
            ProviderSelectionStrategy::Priority,
            ProviderSelectionStrategy::LowestCost,
            ProviderSelectionStrategy::LowestLatency,
            ProviderSelectionStrategy::WeightedRoundRobin,
            ProviderSelectionStrategy::QuotaAware,
            ProviderSelectionStrategy::GeoProximity {
                region_preferences: vec!["us-east-1".into()],
            },
            ProviderSelectionStrategy::Scored {
                cost_weight: 0.3,
                latency_weight: 0.3,
                availability_weight: 0.2,
                quota_weight: 0.2,
            },
        ];
        for strat in &variants {
            let json = serde_json::to_string(strat).unwrap();
            let back: ProviderSelectionStrategy = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&back).unwrap();
            assert_eq!(json, json2);
        }
    }
}
