use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

// ──────────────────────────────────────────────
// Provider layer
// ──────────────────────────────────────────────

/// Health status shared across Provider / Endpoint / Account
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HealthStatus {
    #[default]
    Healthy,
    Degraded,
    Down,
}

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthConfig {
    ApiKey {
        key: String,
        header: String,
    },
    OAuth {
        client_id: String,
        client_secret: String,
        token_url: String,
    },
    Custom {
        headers: HashMap<String, String>,
    },
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self::ApiKey {
            key: String::new(),
            header: "Authorization".to_string(),
        }
    }
}

/// Rate limiting configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimit {
    /// Requests per minute
    pub rpm: Option<u32>,
    /// Tokens per minute
    pub tpm: Option<u32>,
    /// Maximum concurrent requests
    pub concurrent: Option<u32>,
}

/// LLM service provider (e.g. OpenAI, Anthropic, DeepSeek)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub auth: AuthConfig,
    pub base_url: String,
    pub rate_limit: Option<RateLimit>,
    #[serde(default)]
    pub status: HealthStatus,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Supported API protocols (e.g., ["openai"], ["anthropic"], ["openai", "anthropic"]).
    /// Empty means all protocols are supported (backward compatible).
    #[serde(default)]
    pub protocols: Vec<String>,
}

/// An access-point under a Provider (region / deployment)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Endpoint {
    pub id: String,
    pub provider_id: String,
    pub base_url: Option<String>,
    pub region: Option<String>,
    pub auth_override: Option<AuthConfig>,
    pub rate_limit: Option<RateLimit>,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub status: HealthStatus,
}

/// A billing account under an Endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub endpoint_id: String,
    pub auth: AuthConfig,
    pub project_id: Option<String>,
    pub rate_limit: Option<RateLimit>,
    pub billing_tag: Option<String>,
    #[serde(default = "default_weight")]
    pub weight: f32,
    #[serde(default)]
    pub status: HealthStatus,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

fn default_weight() -> f32 {
    1.0
}

// ──────────────────────────────────────────────
// Model layer
// ──────────────────────────────────────────────

/// Quality tier for a model
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum QualityTier {
    S,
    A,
    B,
    C,
}

/// Classification of user request intent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskType {
    SimpleQA,
    CodeGeneration,
    CodeReview,
    Reasoning,
    Creative,
    Translation,
    Summarization,
    DataExtraction,
    Math,
    MultiModal,
    Conversation,
    Unknown,
}

/// What a model can do
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub max_context_window: u32,
    pub max_output_tokens: u32,
    pub supports_streaming: bool,
    pub supports_function_calling: bool,
    pub supports_vision: bool,
    pub supports_json_mode: bool,
    pub supports_reasoning: bool,
}

/// Pricing per 1 M tokens
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_per_1m_tokens: f64,
    pub output_per_1m_tokens: f64,
    #[serde(default = "default_currency")]
    pub currency: String,
}

fn default_currency() -> String {
    "USD".to_string()
}

impl Default for ModelPricing {
    fn default() -> Self {
        Self {
            input_per_1m_tokens: 0.0,
            output_per_1m_tokens: 0.0,
            currency: default_currency(),
        }
    }
}

impl ModelPricing {
    /// Helper: average cost per 1 M tokens (input+output) / 2
    pub fn cost_per_1m(&self) -> f64 {
        (self.input_per_1m_tokens + self.output_per_1m_tokens) / 2.0
    }
}

/// Availability status of a model
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ModelStatus {
    #[default]
    Available,
    Deprecated,
    Disabled,
}

/// Static model metadata (independent of deployment)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub display_name: String,
    pub quality_tier: QualityTier,
    pub capabilities: ModelCapabilities,
    #[serde(default)]
    pub pricing: ModelPricing,
    #[serde(default)]
    pub specialties: Vec<TaskType>,
    pub limits: Option<RateLimit>,
    #[serde(default)]
    pub status: ModelStatus,
}

// ──────────────────────────────────────────────
// Variant layer
// ──────────────────────────────────────────────

/// Deployment status of a variant
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VariantStatus {
    #[default]
    Active,
    Draining,
    Disabled,
}

/// Real-time performance snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPerformance {
    #[serde(default)]
    pub p50_latency_ms: u32,
    #[serde(default)]
    pub p95_latency_ms: u32,
    #[serde(default)]
    pub p99_latency_ms: u32,
    #[serde(default)]
    pub ttft_p50_ms: u32,
    #[serde(default)]
    pub throughput_tps: f32,
    #[serde(default)]
    pub error_rate: f32,
    #[serde(default)]
    pub current_qps: f32,
    #[serde(default)]
    pub queue_depth: u32,
    /// Timestamp of last metrics update (serialisation-unfriendly, so skip)
    #[serde(skip)]
    pub last_updated: Option<Instant>,

    // ── Inference engine metrics ──
    /// GPU utilization (0.0 - 1.0)
    #[serde(default)]
    pub gpu_utilization: f32,

    /// KV cache utilization (0.0 - 1.0), 1.0 means full
    #[serde(default)]
    pub kv_cache_usage: f32,

    /// Number of requests currently being processed (running batch)
    #[serde(default)]
    pub running_requests: u32,

    /// Number of requests waiting in queue
    #[serde(default)]
    pub waiting_requests: u32,

    /// Prefix cache hit rate (0.0 - 1.0)
    #[serde(default)]
    pub prefix_cache_hit_rate: f32,

    /// Number of cached prefix token blocks
    #[serde(default)]
    pub prefix_cache_blocks: u32,
}

impl Default for ModelPerformance {
    fn default() -> Self {
        Self {
            p50_latency_ms: 0,
            p95_latency_ms: 0,
            p99_latency_ms: 0,
            ttft_p50_ms: 0,
            throughput_tps: 0.0,
            error_rate: 0.0,
            current_qps: 0.0,
            queue_depth: 0,
            last_updated: None,
            gpu_utilization: 0.0,
            kv_cache_usage: 0.0,
            running_requests: 0,
            waiting_requests: 0,
            prefix_cache_hit_rate: 0.0,
            prefix_cache_blocks: 0,
        }
    }
}

/// A concrete deployment instance – the smallest scheduling unit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelVariant {
    pub id: String,
    pub model_id: String,
    pub endpoint_id: String,
    pub account_id: Option<String>,
    pub deployment_name: Option<String>,
    #[serde(default = "default_weight")]
    pub weight: f32,
    #[serde(default)]
    pub performance: ModelPerformance,
    pub pricing_override: Option<ModelPricing>,
    #[serde(default)]
    pub status: VariantStatus,
    /// Direct connection URL for this variant (e.g. Pod IP:Port for self-hosted).
    /// When set, overrides the provider/endpoint base_url.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Model name to send upstream in the request body's `model` field.
    /// When set, replaces the user-facing `model_id` before forwarding.
    /// E.g., user sees "deepseek-r1" but upstream expects "deepseek-reasoner".
    #[serde(default)]
    pub upstream_model_name: Option<String>,
}

// ──────────────────────────────────────────────
// Pool layer
// ──────────────────────────────────────────────

/// How variants within a pool are selected
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SelectionMode {
    #[default]
    Scored,
    RoundRobin,
    Random,
    WeightedRandom,
}

/// A logical group of model variants
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPool {
    pub id: String,
    pub name: String,
    pub variants: Vec<String>,
    #[serde(default)]
    pub selection_mode: SelectionMode,
    pub description: Option<String>,
}

// ──────────────────────────────────────────────
// Candidate (flattened for scoring)
// ──────────────────────────────────────────────

/// Assembled by the registry – flattens the hierarchy for the strategy engine.
#[derive(Debug, Clone)]
pub struct ModelCandidate {
    pub variant: ModelVariant,
    pub model: Model,
    pub account: Account,
    pub endpoint: Endpoint,
    pub provider: Provider,
    /// Effective pricing: variant override ▸ model default
    pub effective_pricing: ModelPricing,
}
