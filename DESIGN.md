# Prism — 大模型智能路由服务

> 一束请求进来，根据策略折射到最优模型。

## 1. 项目概述

Prism 是一个高性能的 LLM 智能路由服务，能够根据用户请求的内容、上下文以及预设策略（SLO 优先、最低成本、均衡模式等），自动选择最合适的大模型进行推理。

### 1.1 核心目标

- **智能路由**: 基于请求内容自动判断复杂度，选择最优模型
- **多策略支持**: SLO 优先 / 最低成本 / 质量优先 / 均衡模式，请求级可切换
- **高性能**: 路由决策延迟 < 5ms (P99)，不成为推理链路瓶颈
- **可观测**: 全链路 metrics、成本追踪、质量评估
- **易集成**: OpenAI 兼容 API，零改造接入

### 1.2 技术选型

| 组件 | 语言 | 理由 |
|------|------|------|
| **路由引擎 (prism-core)** | Rust | 极致性能，路由决策在热路径上，需要亚毫秒级延迟 |
| **路由服务 (prism-router)** | Rust (Axum + tonic + Pingora) | HTTP API + gRPC + 反向代理一体化，全链路 Rust 无性能损耗 |
| **分类器推理** | Rust (ONNX Runtime, 规划中) | 嵌入式推理，避免额外网络调用 |
| **管理面 (prism-admin)** | 规划中 | 配置管理、Dashboard API |

### 1.3 为什么全 Rust？

- **路由引擎**: 路由决策是每个请求必经的热路径，Rust 的零成本抽象和无 GC 特性保证稳定低延迟
- **服务层**: Axum (HTTP) + tonic (gRPC) + Pingora (反向代理) 都是成熟的 Rust 生态，避免 FFI 开销
- **一体化**: 单一二进制部署，路由决策 + 请求转发在同一进程内，延迟最低
- **C FFI 接口**: prism-core 仍然导出 C FFI，支持未来嵌入到 Go/Python 等语言的网关中

### 1.4 部署模式

Prism 当前以 prism-router 单一 Rust 二进制部署，同时暴露三种接口：

| 接口 | Feature Flag | 默认端口 | 用途 |
|------|-------------|----------|------|
| **HTTP API** | *(始终开启)* | 8080 | REST 路由决策、健康检查、Prometheus 6 维度指标 |
| **gRPC** | `grpc` | 9090 | 高性能路由服务，与现有网关 (Higress/Envoy/APISIX) 集成 |
| **反向代理** | `proxy` | 8081 | Pingora 驱动的 OpenAI 兼容反向代理，路由 + 转发一体 |

```
prism-router (Rust binary)
  ├── HTTP API (Axum)        → 路由决策 + 监控指标
  ├── gRPC (tonic)           → 高性能路由服务（可选）
  └── Reverse Proxy (Pingora)→ OpenAI 兼容代理（可选）
```

**集成现有网关（gRPC 模式）:**
```
Client ──> Higress / Envoy / APISIX ──[gRPC]──> prism-router
                    │                               │
                    │         RouteResponse ◄────────┘
                    │         (upstream info)
                    └──> Provider (根据 RouteResponse 转发)
```

**独立反向代理模式（最简部署）:**
```
Client ──> prism-router (Proxy, port 8081)
               │
               ├─ 路由决策 (prism-core, 进程内)
               └─ 转发到 Provider (OpenAI/Anthropic/DeepSeek/...)
```

**嵌入模式（C FFI，规划中）:**
```
Client ──> Go/Python 网关 ──[FFI]──> libprism_core.so
               │
               └──> Provider
```

---

## 2. 系统架构

```
                          ┌─────────────────────────────────────────────┐
                          │           prism-router (Rust)               │
                          │                                             │
  Client Request          │  ┌───────────────────────────────────────┐  │
  (OpenAI API)  ─────────▶│  │         HTTP API (Axum)               │  │
                          │  │  /v1/route   — 路由决策               │  │
                          │  │  /v1/outcomes — 结果上报               │  │
                          │  │  /health     — 健康检查               │  │
                          │  │  /metrics    — Prometheus 指标        │  │
                          │  └──────────────────┬────────────────────┘  │
                          │                     │                       │
                          │  ┌──────────────────┼────────────────────┐  │
                          │  │     prism-core (Rust, 进程内库)       │  │
                          │  │                  │                    │  │
                          │  │  ┌───────────┐   │   ┌────────────┐  │  │
                          │  │  │  Request  │   │   │  Plugin    │  │  │
                          │  │  │  Analyzer │───┤   │  Pipeline  │  │  │
                          │  │  └───────────┘   │   └────────────┘  │  │
                          │  │                  │                    │  │
                          │  │  ┌───────────────▼────────────────┐  │  │
                          │  │  │     L1-L5 Strategy Engine      │  │  │
                          │  │  │  → Model Registry              │  │  │
                          │  │  │  → DataLayer (metrics store)   │  │  │
                          │  │  │  → Feedback Loop               │  │  │
                          │  │  └───────────────┬────────────────┘  │  │
                          │  └──────────────────┼────────────────────┘  │
                          │                     │                       │
                          │  ┌──────────────────┼────────────────────┐  │
                          │  │  Reverse Proxy (Pingora, 可选)        │  │
                          │  │  :8081 — OpenAI 兼容，路由+转发一体   │  │
                          │  └──────────────────┼────────────────────┘  │
                          │                     │                       │
                          │  ┌──────────────────┼────────────────────┐  │
                          │  │  gRPC Service (tonic, 可选)           │  │
                          │  │  :9090 — 供外部网关调用               │  │
                          │  └──────────────────┼────────────────────┘  │
                          │                     │                       │
                          │  ┌──────────────────▼────────────────────┐  │
                          │  │       MetricsCollector                │  │
                          │  │  6 维度 Prometheus 指标 + 周期性刷新  │  │
                          │  └──────────────────────────────────────┘  │
                          └─────────────────────────────────────────────┘
                                            │
                              ┌─────────────┼──────────────┐
                              │             │              │
                              ▼             ▼              ▼
                           OpenAI      Anthropic       DeepSeek
                                                    (任何 OpenAI 兼容)
```

---

## 3. 核心模块设计

### 3.1 prism-core (Rust)

路由引擎核心，编译为 `libprism_core.so/.dylib`，通过 FFI 供 Go 调用。

#### 3.1.1 Request Analyzer — 请求分析器

负责从请求中提取路由决策所需的特征。

```rust
/// 请求分析结果
pub struct RequestProfile {
    /// 请求复杂度评分 [0.0, 1.0]
    pub complexity: f32,
    /// 请求类型分类
    pub task_type: TaskType,
    /// 预估 token 数（input + output）
    pub estimated_tokens: TokenEstimate,
    /// 是否需要推理能力
    pub requires_reasoning: bool,
    /// 语言
    pub language: Language,
    /// 是否多轮对话
    pub is_multi_turn: bool,
    /// 上下文窗口需求
    pub context_window_need: ContextSize,
}

pub enum TaskType {
    SimpleQA,         // 简单问答
    CodeGeneration,   // 代码生成
    CodeReview,       // 代码审查
    Reasoning,        // 逻辑推理
    Creative,         // 创意写作
    Translation,      // 翻译
    Summarization,    // 摘要
    DataExtraction,   // 数据提取
    Math,             // 数学计算
    MultiModal,       // 多模态
    Conversation,     // 闲聊
    Unknown,          // 未知
}

pub enum ContextSize {
    Small,   // < 4K tokens
    Medium,  // 4K - 32K
    Large,   // 32K - 128K
    XLarge,  // > 128K
}
```

**分析方法（分层，逐层加精度）:**

1. **规则层 (< 0.1ms)**: 基于关键词、message 数量、token 长度的快速分类
2. **特征层 (< 0.5ms)**: TF-IDF / n-gram 特征提取 + 轻量级分类器
3. **模型层 (< 2ms, 可选)**: 嵌入式 ONNX 小模型推理，用于高精度场景

```rust
pub trait RequestAnalyzer: Send + Sync {
    fn analyze(&self, request: &ChatRequest) -> RequestProfile;
}

/// 分层分析器
pub struct TieredAnalyzer {
    rule_analyzer: RuleBasedAnalyzer,
    feature_analyzer: FeatureAnalyzer,
    model_analyzer: Option<OnnxAnalyzer>,  // 可选，需要加载模型
    tier: AnalysisTier,
}
```

#### 3.1.2 Strategy Engine — 多层调度策略引擎

策略引擎基于调度对象的层级关系，分五层逐层收敛决策：

```
请求进入
  │
  ▼
┌─────────────────────────────────────────────┐
│ L1. 模型选择策略 — 选哪个模型？             │
│     (请求分析 → 模型匹配)                    │
├─────────────────────────────────────────────┤
│ L2. Provider 选择策略 — 走哪个供应商？       │
│     (成本/延迟/可用性)                       │
├─────────────────────────────────────────────┤
│ L3. Account 选择策略 — 用哪个账号？          │
│     (亲和性/配额/负载均衡)                   │
├─────────────────────────────────────────────┤
│ L4. Variant 选择策略 — 选哪个实例？          │
│     (实时性能/权重)                          │
├─────────────────────────────────────────────┤
│ L5. 容错策略 — 失败了怎么办？               │
│     (重试/降级/fallback/熔断/对冲)           │
└─────────────────────────────────────────────┘
```

##### L1. 模型选择策略

**核心问题：这个请求应该用什么级别的模型？**

```rust
pub enum ModelSelectionStrategy {
    /// 指定模型：用户明确指定 model="gpt-4o"，跳过选择
    Pinned { model_id: String },

    /// 智能选择：基于请求内容分析自动选模型
    Auto {
        /// 业务目标
        objective: RoutingObjective,
        /// 候选池（不指定则全局可用）
        pool_ids: Option<Vec<String>>,
        /// 硬约束：必须满足的能力要求
        required_capabilities: Option<RequiredCapabilities>,
    },

    /// 级联：先试小模型，质量不达标再升级
    Cascade {
        tiers: Vec<CascadeTier>,
    },
}

pub enum RoutingObjective {
    /// SLO 优先：延迟必须达标
    SloFirst {
        max_latency_ms: u32,
        max_ttft_ms: u32,
        min_quality_score: f32,
    },
    /// 成本优先
    CostFirst {
        min_quality_score: f32,
        budget_per_request: Option<f64>,  // 单次请求预算上限
    },
    /// 质量优先
    QualityFirst {
        max_latency_ms: Option<u32>,
    },
    /// 均衡
    Balanced {
        cost_weight: f32,
        quality_weight: f32,
        latency_weight: f32,
    },
    /// 自定义评分函数
    Custom {
        scorer_id: String,
    },
}

/// 能力硬约束（不满足直接过滤）
pub struct RequiredCapabilities {
    pub min_context_window: Option<u32>,
    pub function_calling: Option<bool>,
    pub vision: Option<bool>,
    pub json_mode: Option<bool>,
    pub reasoning: Option<bool>,
}

/// 级联层级：先用便宜模型生成，质量不达标再升级
pub struct CascadeTier {
    pub pool_id: String,                // "tier-3" → "tier-2" → "tier-1"
    pub quality_threshold: f32,         // 质量不达标则升级到下一层
    pub evaluator: QualityEvaluator,    // 怎么判断质量
}
```

**L1 评分算法：**

```rust
impl StrategyEngine {
    /// 对每个候选模型打分，选择最优
    pub fn score_model(
        &self,
        objective: &RoutingObjective,
        profile: &RequestProfile,
        candidate: &ModelCandidate,
    ) -> f32 {
        match objective {
            RoutingObjective::Balanced { cost_weight, quality_weight, latency_weight } => {
         cost_score = 1.0 - normalize(
                    candidate.effective_pricing.cost_per_1m,
                    0.0, self.max_cost
                );
                let quality_score = self.estimate_quality(profile, candidate);
                let latency_score = 1.0 - normalize(
                    candidate.variant.performance.p50_latency_ms as f32,
                    0.0, self.max_latency
                );

                cost_score * cost_weight
                    + quality_score * quality_weight
                    + latency_score * latency_weight
            }
            RoutingObjective::SloFirst { max_latency_ms, max_ttft_ms, min_quality_score } => {
                // 硬约束：延迟不达标直接 0 分
                if candidate.variant.performance.p99_latency_ms > *max_latency_ms {
                    return 0.0;
                }
                if candidate.variant.performance.ttft_p50_ms > *max_ttft_ms {
                    return 0.0;
                }
                // 软约束：在满足 SLO 的前提下选质量最好的
                self.estimate_quality(profile, candidate)
            }
            RoutingObjective::CostFirst { min_quality_score, budget_per_request } => {
                let quality = self.estimate_quality(profile, candidate);
                if quality < *min_quality_score {
                    return 0.0;
                }
                // 质量达标后，成本越低分越高
                1.0 - normalize(
                    candidate.effective_pricing.cost_per_1m,
                    0.0, self.max_cost
                )
            }
            // ...
        }
    }
}
```

##### L2. Provider 选择策略

**核心问题：同一个模型多个供应商都有，选哪个？**

典型场景：Claude 既可以走 Anthropic 直连，也可以走 GCP Vertex、AWS Bedrock，
价格、延迟、配额可能都不同。

```rust
pub enum ProviderSelectionStrategy {
    /// 优先级：按配置的优先级顺序（主备模式）
    Priority,

    /// 成本最优：选当前最便宜的
    LowestCost,

    /// 延迟最优：基于实时 P50 延迟选择
    LowestLatency,

    /// 加权轮询
    WeightedRoundRobin,

    /// 地理就近：按请求来源 region 选最近的
    GeoProximity {
        region_preferences: Vec<String>,
    },

    /// 配额感知：优先选配额剩余最多的
    QuotaAware,

    /// 综合评分
    Scored {
        cost_weight: f32,
        latency_weight: f32,
        availability_weight: f32,
        quota_weight: f32,
    },
}
```

##### L3. Account 选择策略

**核心问题：同一个 Provider/Endpoint 下多个账号，用哪个？**

```rust
pub enum AccountSelectionStrategy {
    /// 亲和性优先（提高缓存命中率）
    AffinityFirst {
        affinity: AffinityStrategy,
        /// 亲和失败时的降级策略
        fallback: Box<AccountSelectionStrategy>,
    },

    /// 配额均衡：优先选配额剩余最多的账号
    QuotaBalanced,

    /// 负载均衡：基于实时 QPS / 并发数
    LoadBalanced {
        algorithm: LoadBalanceAlgorithm,
    },

    /// 成本隔离：按业务线/租户路由到对应的计费账号
    CostIsolation {
        /// 从请求中提取租户标识 → 映射到账号
        tenant_key: AffinityKeySource,
        tenant_account_map: HashMap<String, String>,
    },

    /// 加权随机
    WeightedRandom,
}

pub enum LoadBalanceAlgorithm {
    RoundRobin,
    LeastConnections,
    LeastLoad,               // 基于实时负载
    PowerOfTwoChoices,       // P2C 随机选两个取更优的
}
```

**亲和性 + 负载均衡的联动流程：**

```
请求 (session_id: abc)
  │
  ├─ 查亲和绑定 → 命中 account-A
  │   ├─ account-A 健康且负载 < 阈值 → 直接使用 ✅
  │   └─ account-A 过载 (QPS > 配额 80%)
  │       ├─ Soft 亲和 → 迁移到负载最低的账号，更新绑定
  │       └─ Hard 亲和 → 排队等待，或返回 429
  │
  └─ 未命中 → LoadBalanced 选账号 → 写入绑定
```

##### L4. Variant 选择策略

**核心问题：最终路由到哪个具体部署实例？**

经过 L1-L3 已经收敛到很小的候选集，这层做最终的实例选择。

```rust
pub enum VariantSelectionStrategy {
    /// 实时性能最优
    BestPerformance {
        metric: PerformanceMetric,
    },

    /// 加权选择（人工调权重）
    Weighted,

    /// 灰度发布：新版本 variant 按比例切流量
    Canary {
        canary_variant_id: String,
        canary_pct: f32,                 // 0.0 - 1.0
    },

    /// 默认：如果只剩一个 variant，直接用
    Default,
}

pub enum PerformanceMetric {
    LatencyP50,
    LatencyP99,
    TTFT,
    Throughput,
    ErrorRate,
    CompositeScore,    // 综合评分
}
```

##### L5. 容错策略

**核心问题：调度到的目标失败了怎么办？**

```rust
pub struct ResiliencePolicy {
    /// 重试策略
    pub retry: RetryPolicy,
    /// 降级策略
    pub fallback: FallbackPolicy,
    /// 熔断策略
    pub circuit_breaker: CircuitBreakerPolicy,
    /// 超时策略
    pub timeout: TimeoutPolicy,
    /// 对冲请求（同时发多个，取最快返回的）
    pub hedging: Option<HedgingPolicy>,
}

pub struct RetryPolicy {
    pub max_retries: u32,
    pub retry_on: Vec<RetryCondition>,
    pub backoff: BackoffStrategy,
    /// 在哪一层重试
    pub retry_scope: RetryScope,
}

pub enum RetryScope {
    /// 同 Variant 重试（可能是临时错误）
    SameVariant,
    /// 同 Account 换 Variant
    SameAccount,
    /// 同 Provider 换 Account
    SameProvider,
    /// 换 Provider（最大范围重试）
    CrossProvider,
}

pub enum RetryCondition {
    HttpStatus(Vec<u16>),        // 429, 500, 502, 503
    Timeout,
    ConnectionError,
    RateLimit,
    ModelOverloaded,
}

pub struct FallbackPolicy {
    /// 静态降级链：按顺序尝试
    pub chain: Vec<FallbackTarget>,
    /// 是否允许跨模型降级（如 gpt-4o 降级到 gpt-4o-mini）
    pub allow_model_downgrade: bool,
    /// 降级时是否通知客户端（通过 response header）
    pub notify_client: bool,
}

pub enum FallbackTarget {
    /// 指定 variant
    Variant(String),
    /// 指定模型（重新走 L2-L4 选择）
    Model(String),
    /// 指定 Pool
    Pool(String),
    /// 返回缓存的响应（如果有语义相似的历史请求）
    CachedResponse,
}

pub struct CircuitBreakerPolicy {
    pub error_threshold_pct: f32,        // 错误率阈值
    pub window_seconds: u32,             // 统计窗口
    pub min_requests: u32,               // 最少请求数才触发
    pub open_duration_seconds: u32,      // 熔断持续时间
    pub half_open_requests: u32,         // 半开状态探测请求数
}

/// 对冲请求：同时发多个，取最快返回的（适合延迟敏感场景）
pub struct HedgingPolicy {
    pub max_parallel: u32,               // 最多同时发几个
    pub delay_ms: u32,                   // 延迟多久发第二个
    pub cancel_remaining: bool,          // 拿到结果后取消其余
}

pub struct TimeoutPolicy {
    pub connect_ms: u32,                 // 连接超时
    pub request_ms: u32,                 // 整体请求超时
    pub ttft_ms: Option<u32>,            // 首 token 超时
}
```

##### 策略覆盖优先级

请求级 > 租户级 > Pool 级 > 全局默认：

```
X-Prism-Strategy: cost_first        ← 请求级覆盖
       ↓ (如果没指定)
tenant config (API Key 关联)         ← 租户级
       ↓ (如果没指定)
pool config                          ← Pool 级
       ↓ (如果没指定)
global default in prism.yaml         ← 全局
```

##### 路由决策结果

```rust
/// 路由决策（L1-L4 的最终输出）
pub struct RoutingDecision {
    pub selected_variant: String,        // 最终选中的 ModelVariant ID
    pub selected_model: String,          // 模型 ID
    pub selected_account: String,        // 账号 ID
    pub selected_provider: String,       // Provider ID
    pub fallback_chain: Vec<String>,     // 降级链（variant IDs）
    pub reason: RoutingReason,           // 每层的决策原因（可观测）
    pub score: f32,                      // 综合评分
    pub decision_time_us: u64,           // 决策耗时（微秒）
}

/// 决策原因（用于可观测性和调试）
pub struct RoutingReason {
    pub l1_model: String,                // "balanced: quality=0.92, cost=0.65"
    pub l2_provider: String,             // "scored: latency=best, quota=80%"
    pub l3_account: String,              // "affinity: session-abc → account-A"
    pub l4_variant: String,              // "best_performance: p50=320ms"
    pub constraints_applied: Vec<String>, // 应用了哪些硬约束
    pub candidates_filtered: u32,        // 过滤掉了多少候选
}
```

#### 3.1.3 调度对象模型 & Model Registry

路由引擎的调度对象分为五层，从粗到细：

```
Provider (供应商)
  └── Endpoint (接入点)
        └── Account (供应账号)
              └── Model (模型)
                    └── ModelVariant (模型变体/部署实例) ← 最终调度单元
                          └── ModelPool (模型池，逻辑分组)
```

关系图：

```
                    ┌──────────────────┐
                    │    ModelPool     │
                    │  (tier-1, etc)   │
                    └────────┬─────────┘
                             │ contains N
                             ▼
┌──────────┐  1:N  ┌─────────┐  1:N  ┌─────────┐
│ Provider │◄──────│Endpoint │◄──────│ Account │
│ (vertex) │       │(region) │       │(project)│
└──────────┘       └─────────┘       └────┬────┘
                                          │
                     ┌─────────┐    ┌─────┴────────┐
                     │  Model  │───▶│ ModelVariant  │
                     │ (claude)│    │ (调度最小单元)  │
                     └─────────┘    └──────────────┘
                                      ▲
                                      │
                                路由引擎最终
                                选择的对象
```

##### Provider — 供应商

LLM 服务提供方，管理认证和全局限流。

```rust
pub struct Provider {
    pub id: String,                      // "openai"
    pub name: String,                    // "OpenAI"
    pub auth: AuthConfig,                // API Key / OAuth / Custom
    pub base_url: String,                // "https://api.openai.com/v1"
    pub rate_limit: Option<RateLimit>,   // 全局限流 (RPM / TPM)
    pub status: HealthStatus,            // healthy | degraded | down
    pub metadata: HashMap<String, String>, // region, billing_account 等
}

pub enum AuthConfig {
    ApiKey { key: String, header: String },  // header 默认 "Authorization"
    OAuth { client_id: String, client_secret: String, token_url: String },
    Custom { headers: HashMap<String, String> },
}

pub struct RateLimit {
    pub rpm: Option<u32>,                // Requests per minute
    pub tpm: Option<u32>,                // Tokens per minute
    pub concurrent: Option<u32>,         // 最大并发
}
```

##### Endpoint — 接入点

同一供应商的不同接入地址（如 Azure OpenAI 不同 region、私有部署 vs 公有 API）。

```rust
pub struct Endpoint {
    pub id: String,                      // "openai-us-east"
    pub provider_id: String,             // "openai"
    pub base_url: Option<String>,        // 可覆盖 Provider 的 base_url
    pub region: Option<String>,          // "us-east-1"
    pub auth_override: Option<AuthConfig>, // 可覆盖 Provider 的认证
    pub rate_limit: Option<RateLimit>,   // 可覆盖
    pub priority: u32,                   // 同 Provider 内优先级
    pub status: HealthStatus,
}
```

对于只有单一入口的供应商（如 DeepSeek），Endpoint 可省略，系统自动生成一个与 Provider 同 ID 的默认 Endpoint。

##### Account — 供应账号

同一个 Endpoint 下的不同账号/项目。典型场景：GCP Vertex AI 下多个 project 都能访问 Claude，
不同账号有独立的配额、计费，且 Provider 侧的 Context Cache / KV Cache 按账号隔离。

```rust
pub struct Account {
    pub id: String,                      // "gcp-project-alpha"
    pub endpoint_id: String,             // "vertex-us-central1"
    pub auth: AuthConfig,                // 该账号的认证信息
    pub project_id: Option<String>,      // GCP project / Azure subscription
    pub rate_limit: Option<RateLimit>,   // 账号级配额
    pub billing_tag: Option<String>,     // 计费标签/成本中心
    pub weight: f32,                     // 流量权重，默认 1.0
    pub status: HealthStatus,
    pub metadata: HashMap<String, String>,
}
```

对于不区分账号的场景（如 DeepSeek 只有一个 API Key），Account 可省略，
系统自动生成一个与 Endpoint 同 ID 的默认 Account。

**更新后的完整层级：**

```
Provider (供应商: GCP Vertex AI)
  └── Endpoint (接入点: us-central1)
        └── Account (账号: project-a / project-b)
              └── ModelVariant (模型变体: claude-sonnet@project-a)
```

##### Affinity — 亲和性路由

账号亲和的核心目的是**提高 Provider 侧缓存命中率**。同一用户/session 的请求
路由到同一 Account，可以复用 Provider 端的 KV Cache / Context Cache / Prompt Cache，
在多轮对话场景下显著降低延迟和成本。

```rust
/// 亲和性策略
pub enum AffinityStrategy {
    /// 无亲和，每次独立选择
    None,
    /// 按 affinity key 做一致性哈希，绑定到 Account
    ConsistentHash {
        key_source: AffinityKeySource,
    },
    /// 软亲和：优先选之前的 Account，但在负载不均时可迁移
    Soft {
        key_source: AffinityKeySource,
        max_load_ratio: f32,            // 超过此负载比时允许迁移
    },
    /// 硬亲和：严格绑定，除非 Account 不可用
    Hard {
        key_source: AffinityKeySource,
    },
}

/// 亲和 key 的来源
pub enum AffinityKeySource {
    /// 从请求 header 中提取（如 X-Prism-User-Id / X-Prism-Session-Id）
    Header(String),
    /// 从 API Key 提取（不同租户自动隔离）
    ApiKey,
    /// 从 messages 的 conversation_id 或首条 system prompt hash
    ConversationHash,
    /// 自定义提取函数
    Custom(String),
}

/// 亲和状态追踪（存 Redis）
pub struct AffinityBinding {
    pub affinity_key: String,            // 如 "user:12345"
    pub account_id: String,              // 绑定的 Account
    pub model_id: String,                // 绑定的模型
    pub created_at: Instant,
    pub last_used: Instant,
    pub ttl_seconds: u64,                // 绑定过期时间
}
```

**路由引擎中的亲和性决策流程：**

```
1. 提取 affinity_key（从 header / API key / conversation hash）
2. 查询 AffinityBinding（Redis）
   ├── 命中 → 检查绑定的 Account 是否健康且未过载
   │          ├── 健康 → 直接路由到该 Account 的 Variant ✅
   │          └── 不健康/过载 → 按策略决定：
   │              ├── Hard: 等待或报错
   │              └── Soft: 重新选择，更新绑定
   └── 未命中 → 正常路由选择，选定后写入绑定
```

**配置示例：**

```yaml
routing:
  affinity:
    strategy: soft
    key_source:
      type: header
      header: "X-Prism-Session-Id"
    max_load_ratio: 1.5     # Account 负载超过平均值 1.5 倍时允许迁移
    ttl_seconds: 3600        # 绑定 1 小时后过期

  # 也支持在 API 请求中指定
  # X-Prism-Affinity-Key: session-abc-123
```

##### Model — 模型

模型的静态元信息，与具体部署无关。

```rust
pub struct Model {
    pub id: String,                      // "gpt-4o"
    pub display_name: String,            // "GPT-4o"
    pub quality_tier: QualityTier,       // S | A | B | C
    pub capabilities: ModelCapabilities,
    pub pricing: ModelPricing,           // 官方定价（可被 Variant 覆盖）
    pub specialties: Vec<TaskType>,      // 擅长的任务类型
    pub limits: Option<RateLimit>,
    pub status: ModelStatus,             // available | deprecated | disabled
}

pub enum QualityTier { S, A, B, C }

pub struct ModelCapabilities {
    pub max_context_window: u32,
    pub max_output_tokens: u32,
    pub supports_streaming: bool,
    pub supports_function_calling: bool,
    pub supports_vision: bool,
    pub supports_json_mode: bool,
    pub supports_reasoning: bool,        // o1/o3 类思考模型
}

pub struct ModelPricing {
    pub input_per_1m_tokens: f64,        // 输入 $/1M tokens
    pub output_per_1m_tokens: f64,       // 输出 $/1M tokens
    pub currency: String,                // "USD"
}
```

##### ModelVariant — 模型变体 / 部署实例（最终调度单元）

**路由引擎最终调度的最小单元。** 绑定具体模型 + 具体接入点，附带实时性能数据。

同一个模型可能有多个 Variant，例如：
- `gpt-4o` 通过 OpenAI 直连 vs Azure 不同 region
- `deepseek-v3` 官方 API vs 万擎自部署
- 同模型不同量化版本

```rust
pub struct ModelVariant {
    pub id: String,                      // "gpt-4o@openai-us-east"
    pub model_id: String,                // "gpt-4o"
    pub endpoint_id: String,             // "openai-us-east"
    pub account_id: Option<String>,      // 绑定的 Account（多账号场景）
    pub deployment_name: Option<String>, // 实际部署版本 "gpt-4o-2024-08-06"
    pub upstream_model_name: Option<String>, // 发给上游 Provider 的模型名（覆盖 model_id）
    pub weight: f32,                     // 手动权重调整，默认 1.0
    pub performance: ModelPerformance,   // 实时性能数据
    pub pricing_override: Option<ModelPricing>, // 覆盖 Model 层定价（如私有部署）
    pub status: VariantStatus,           // active | draining | disabled
}

pub enum VariantStatus {
    Active,     // 正常接收流量
    Draining,   // 逐步排空，不接新请求
    Disabled,   // 已禁用
}
```

**`upstream_model_name` — 上游模型名映射**

某些 Provider 的模型名与 Prism 的 `model_id` 不同。例如：
- DeepSeek 官方 API 接受 `deepseek-chat`，但内部 model_id 为 `deepseek-v3`
- 自部署 vLLM 的模型名可能是 `/data/models/qwen2.5-72b-instruct`

`upstream_model_name` 让 Variant 能覆盖请求体中的 `model` 字段，
反向代理层会自动把请求 JSON body 里的 `"model"` 改写为该值再转发给上游 Provider。

```yaml
# 配置示例
variants:
  - id: "deepseek-v3@deepseek"
    model_id: deepseek-v3
    provider_id: deepseek
    upstream_model_name: "deepseek-chat"  # 发给上游的实际模型名
  - id: "qwen2.5-72b@local"
    model_id: qwen2.5-72b
    provider_id: local-vllm
    upstream_model_name: "/data/models/qwen2.5-72b-instruct"
```

pub struct ModelPerformance {
    pub p50_latency_ms: u32,
    pub p95_latency_ms: u32,
    pub p99_latency_ms: u32,
    pub ttft_p50_ms: u32,                // Time to First Token
    pub throughput_tps: f32,             // Tokens per second
    pub error_rate: f32,                 // 最近 N 分钟错误率
    pub current_qps: f32,               // 当前 QPS
    pub queue_depth: u32,                // 排队深度
    pub last_updated: Instant,
}
```

##### ModelPool — 模型池（逻辑分组）

将多个 Variant 组织成逻辑池，用于策略路由的候选范围。

```rust
pub struct ModelPool {
    pub id: String,                      // "tier-1"
    pub name: String,                    // "高质量模型池"
    pub variants: Vec<String>,           // variant IDs
    pub selection_mode: SelectionMode,   // scored | round_robin | random
    pub description: Option<String>,
}

pub enum SelectionMode {
    Scored,      // 根据策略引擎打分选择（默认）
    RoundRobin,  // 轮询
    Random,      // 随机
    WeightedRandom, // 按权重随机
}
```

##### Model Registry — 注册表

管理所有调度对象的生命周期和实时状态。

```rust
pub struct ModelRegistry {
    providers: DashMap<String, Provider>,
    endpoints: DashMap<String, Endpoint>,
    accounts: DashMap<String, Account>,
    models: DashMap<String, Model>,
    variants: DashMap<String, ModelVariant>,
    pools: DashMap<String, ModelPool>,

    /// 实时性能数据追踪（滑动窗口）
    performance_tracker: PerformanceTracker,
    /// 健康检查
    health_checker: HealthChecker,
    /// 亲和性绑定追踪
    affinity_tracker: AffinityTracker,
}

impl ModelRegistry {
    /// 获取某个 Pool 下所有可用的 Variant（过滤掉不健康的）
    pub fn get_available_variants(&self, pool_id: &str) -> Vec<ModelVariant>;

    /// 获取满足 capabilities 要求的所有 Variant
    pub fn filter_by_capabilities(&self, required: &ModelCapabilities) -> Vec<ModelVariant>;

    /// 更新某个 Variant 的实时性能数据
    pub fn update_performance(&self, variant_id: &str, perf: ModelPerformance);

    /// 热更新配置（增删改 Provider/Model/Variant/Pool）
    pub fn reload(&self, config: &RegistryConfig);
}
```

**策略引擎中的 `ModelCandidate` 由 Registry 组装：**

```rust
/// 路由决策的候选对象（由 Registry 组装，打平层级关系）
pub struct ModelCandidate {
    pub variant: ModelVariant,
    pub model: Model,
    pub account: Account,
    pub endpoint: Endpoint,
    pub provider: Provider,
    /// 有效定价（Variant override > Model default）
    pub effective_pricing: ModelPricing,
}
```

#### 3.1.4 FFI 接口

```rust
// lib.rs - C ABI 导出给 Go 调用

#[repr(C)]
pub struct PrismRouteRequest {
    pub messages_json: *const c_char,
    pub strategy_json: *const c_char,
    pub metadata_json: *const c_char,
}

#[repr(C)]
pub struct PrismRouteResult {
    pub model_id: *const c_char,
    pub provider: *const c_char,
    pub fallbacks_json: *const c_char,
    pub reason_json: *const c_char,
    pub decision_time_us: u64,
    pub error: *const c_char,
}

#[no_mangle]
pub extern "C" fn prism_route(request: *const PrismRouteRequest) -> *mut PrismRouteResult {
    // ...
}

#[no_mangle]
pub extern "C" fn prism_init(config_json: *const c_char) -> i32 {
    // 初始化路由引擎
}

#[no_mangle]
pub extern "C" fn prism_update_model_stats(stats_json: *const c_char) -> i32 {
    // 更新模型实时性能数据
}

#[no_mangle]
pub extern "C" fn prism_free_result(result: *mut PrismRouteResult) {
    // 释放内存
}
```

#### 3.1.5 路由服务协议（gRPC / HTTP）

独立部署模式下，prism-core 以 `prism-router` 二进制运行，通过 gRPC 和 HTTP API 对外提供路由决策服务。
这是与 Higress、Envoy、APISIX 等现有网关集成的核心接口。

**协议定义 (`proto/prism/router/v1/router.proto`)：**

```protobuf
syntax = "proto3";
package prism.router.v1;

// ──────────────────────────────────────────────
// 核心服务
// ──────────────────────────────────────────────

service RouterService {
  // 路由决策：根据请求内容和策略选择最优模型/Provider/Account
  rpc Route(RouteRequest) returns (RouteResponse);

  // 批量路由（减少 RPC 开销）
  rpc RouteBatch(RouteBatchRequest) returns (RouteBatchResponse);

  // 上报请求结果（用于实时性能追踪和反馈闭环）
  rpc ReportOutcome(OutcomeReport) returns (ReportResponse);

  // 健康检查
  rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);
}

// ──────────────────────────────────────────────
// 路由请求
// ──────────────────────────────────────────────

message RouteRequest {
  // 请求内容（用于分析复杂度和任务类型）
  repeated ChatMessage messages = 1;

  // 路由策略（可选覆盖默认配置）
  optional string strategy = 2;           // "balanced", "cost_first", "slo_first", "quality_first"
  optional string model = 3;              // "auto" 触发智能路由，具体模型名则 Pinned 模式

  // 候选范围
  repeated string pool_ids = 4;           // 限定候选模型池
  repeated string exclude_models = 5;     // 排除的模型
  repeated string exclude_providers = 6;  // 排除的 Provider

  // 亲和性
  optional string affinity_key = 7;       // session id / user id
  optional string tenant_id = 8;          // 租户标识（成本隔离）

  // 能力硬约束
  optional CapabilityRequirements capabilities = 9;

  // 策略参数覆盖
  optional StrategyParams strategy_params = 10;

  // 透传元数据
  map<string, string> metadata = 15;
}

message ChatMessage {
  string role = 1;
  string content = 2;
}

message CapabilityRequirements {
  optional uint32 min_context_window = 1;
  optional bool vision = 2;
  optional bool function_calling = 3;
  optional bool json_mode = 4;
  optional bool reasoning = 5;
}

message StrategyParams {
  // Balanced 模式权重
  optional float cost_weight = 1;
  optional float quality_weight = 2;
  optional float latency_weight = 3;

  // SLO 约束
  optional uint32 max_latency_ms = 4;
  optional uint32 max_ttft_ms = 5;
  optional float min_quality_score = 6;

  // Cost 约束
  optional double budget_per_request = 7;
}

// ──────────────────────────────────────────────
// 路由响应
// ──────────────────────────────────────────────

message RouteResponse {
  // 路由决策结果
  string model = 1;                       // 选中的模型 ID
  string provider = 2;                    // 选中的 Provider ID
  string variant = 3;                     // 选中的 Variant ID
  string account = 4;                     // 选中的 Account ID

  // 上游连接信息（网关用这些来转发请求）
  Upstream upstream = 5;

  // 降级链（按优先级排序）
  repeated Upstream fallback_chain = 6;

  // 可观测性
  RoutingReason reason = 7;
  float score = 8;
  int64 decision_time_us = 9;

  // 透传给客户端 response header 的信息
  map<string, string> response_metadata = 10;
}

// 网关转发请求所需的全部信息
message Upstream {
  string base_url = 1;                    // "https://api.openai.com/v1"
  string model_name = 2;                  // 实际发给 Provider 的模型名
  map<string, string> headers = 3;        // Authorization, x-api-key 等
  string path = 4;                        // API 路径（默认 "/chat/completions"）
  int32 timeout_ms = 5;                   // 超时
  string variant_id = 6;                  // 用于上报 outcome
}

message RoutingReason {
  string l1_model = 1;
  string l2_provider = 2;
  string l3_account = 3;
  string l4_variant = 4;
  repeated string constraints_applied = 5;
  uint32 candidates_filtered = 6;
}

// ──────────────────────────────────────────────
// 结果上报（反馈闭环）
// ──────────────────────────────────────────────

message OutcomeReport {
  string request_id = 1;                  // 请求追踪 ID
  string variant_id = 2;                  // 实际使用的 variant
  int32 status_code = 3;                  // HTTP 状态码
  int64 latency_ms = 4;                   // 总延迟
  int64 ttft_ms = 5;                      // 首 token 延迟
  int32 input_tokens = 6;
  int32 output_tokens = 7;
  optional string error = 8;              // 错误信息
  bool used_fallback = 9;                 // 是否走了降级链
  optional string fallback_variant_id = 10; // 实际降级到的 variant
}

message ReportResponse {}

// ──────────────────────────────────────────────
// 批量 & 健康检查
// ──────────────────────────────────────────────

message RouteBatchRequest {
  repeated RouteRequest requests = 1;
}

message RouteBatchResponse {
  repeated RouteResponse responses = 1;
}

message HealthCheckRequest {}

message HealthCheckResponse {
  string status = 1;                      // "healthy", "degraded", "down"
  int64 uptime_seconds = 2;
  uint32 registered_models = 3;
  uint32 active_variants = 4;
}
```

**HTTP REST API（兼容不使用 gRPC 的场景）：**

```
POST /v1/route              → 等价于 RouterService.Route
POST /v1/route/batch        → 等价于 RouterService.RouteBatch
POST /v1/report             → 等价于 RouterService.ReportOutcome
GET  /health                → 等价于 RouterService.HealthCheck
GET  /metrics               → Prometheus metrics
```

**Upstream 字段的设计意图：**

路由决策不只返回"用哪个模型"，还返回**怎么连上去**。这样任意网关拿到 `RouteResponse` 后
可以直接用 `upstream.base_url` + `upstream.model_name` + `upstream.headers` 构造并转发请求，
不需要自己维护 Provider 的认证和连接信息。

```
网关收到 RouteResponse:
  upstream:
    base_url: "https://us-central1-aiplatform.googleapis.com"
    model_name: "claude-3-5-sonnet@20240620"
    headers:
      Authorization: "Bearer ya29.xxx..."
    timeout_ms: 30000

网关直接构造请求:
  POST {base_url}/v1/chat/completions
  Authorization: Bearer ya29.xxx...
  {"model": "claude-3-5-sonnet@20240620", "messages": [...]}
```

**与 Envoy ext_proc 的适配：**

在模式 C 下，prism-router 实现 Envoy `envoy.service.ext_proc.v3.ExternalProcessor` 接口，
在 `request_headers` 阶段完成路由决策，通过修改请求头来指定上游：

```
1. Envoy 发送 request_headers → prism-router
2. prism-router 解析请求，调用 Route()
3. 返回 header_mutation:
   - 设置 :authority / Host → upstream host
   - 设置 Authorization → upstream auth
   - 设置 x-prism-model / x-prism-variant → 路由元信息
4. Envoy 根据修改后的 headers 路由到对应 upstream cluster
```

---

### 3.2 prism-router (Rust)

prism-router 是 Prism 的服务层，以单一 Rust 二进制部署，包含 HTTP API、gRPC 和反向代理三种接口。

#### 3.2.1 项目结构

```
prism-router/
├── Cargo.toml
├── build.rs                     # tonic-build (proto 编译)
└── src/
    ├── main.rs                  # 入口：HTTP + gRPC + Proxy 启动
    ├── api.rs                   # Axum HTTP 处理器
    │                            #   POST /v1/route — 路由决策
    │                            #   POST /v1/outcomes — 结果上报
    │                            #   GET  /health — 健康检查
    │                            #   GET  /version — 版本信息
    │                            #   GET  /metrics — Prometheus 指标
    ├── grpc.rs                  # tonic gRPC RouterService 实现
    ├── proxy.rs                 # Pingora 反向代理 (OpenAI 兼容)
    ├── protocol.rs              # 协议检测 (completions / embeddings / ...)
    ├── metrics.rs               # MetricsCollector — 6 维度 Prometheus 指标
    ├── config/                  # 配置源插件化
    │   ├── mod.rs               #   ConfigSource trait, CompositeSource
    │   ├── types.rs             #   PrismConfig 完整类型定义
    │   ├── file_source.rs       #   FileSource — YAML/JSON/TOML 文件加载
    │   ├── env_source.rs        #   EnvSource — 环境变量加载
    │   ├── wasm_source.rs       #   WasmConfigSource (feature-gated)
    │   └── registry.rs          #   build_registry() — PrismConfig → ModelRegistry
    ├── state.rs                 # AppState (共享 Router + Metrics)
    ├── ratelimit.rs             # 限流系统 (滑动窗口 + header-based key)
    ├── unified_plugin.rs        # 统一插件注册框架
    │                            #   PluginRegistry, PluginDescriptor, PluginPhase,
    │                            #   PluginSource, 自动能力检测, load_all()
    ├── plugin_loader.rs         # 插件加载器 (File/URL/OCI/Builtin + 缓存 + SHA-256)
    ├── plugin_manager.rs        # 插件管理器 (load/unload/reload + 事件回调)
    └── codec/                   # Proxy 插件链 (协议翻译)
        ├── mod.rs               #   ProxyPlugin trait + ProxyContext
        ├── chain.rs             #   ProxyPluginChain
        ├── registry.rs          #   CodecRegistry
        └── wasm_host.rs         #   wasmtime 宿主层 (feature-gated)
```

#### 3.2.2 HTTP API

```
POST /v1/route              → 路由决策（分析请求内容，返回最优模型 + 上游连接信息）
POST /v1/outcomes           → 结果上报（用于实时性能追踪和反馈闭环）
GET  /health                → 健康检查
GET  /version               → 版本信息
GET  /metrics               → Prometheus 6 维度指标（完整 exposition format）
```

#### 3.2.3 6 维度监控指标

MetricsCollector 提供完整的 Prometheus 指标输出：

| 维度 | 指标 | 说明 |
|---|---|---|
| **路由决策** | `routing_decision_duration_us` | 直方图：1μs/5μs/10μs/50μs/100μs/500μs/1ms/5ms |
| | `strategy_calls_total` | 按策略名分类的调用计数 |
| | `fallback_triggered_total` | 降级触发次数 |
| | `plugin_filter/score_duration_us` | 插件各阶段耗时直方图 |
| **Provider** | `provider_requests/errors/latency_sum` | 每 Provider 的请求/错误/延迟 |
| | `rate_limit_utilization` | 限流利用率 (0-1.0) |
| | `circuit_breaker_state` | 熔断器状态 (0/1/2) |
| **流量分布** | `routes_by_model/provider/account_total` | 按维度的路由分布 |
| | `experiment_assignments_total` | A/B 实验分组计数 |
| **Token 用量** | `input/output_tokens_total` | 按 variant 的 token 消耗 |
| | `tokens_per_second` | 按 variant 的吞吐 |
| **资源** | `active_requests` | 每 variant 活跃请求数 |
| | `queue_depth` | 全局队列深度 |
| **DataLayer** | `data_layer_queries/hits/latency_sum_us` | 数据层查询统计 |

每个 variant 的延迟百分位 (P50/P95/P99)、错误率、QPS、TTFT 也会导出。

#### 3.2.4 反向代理 (Pingora)

通过 `proxy` feature flag 启用。基于 Pingora 实现的 OpenAI 兼容反向代理，客户端无需感知路由逻辑，只需将 `base_url` 指向 prism-router 的 proxy 端口。

代理流程：
1. 接收 OpenAI 格式请求
2. 检测协议类型 (completions / embeddings / etc.)
3. 调用 prism-core 进行路由决策
4. 修改请求目标为选定的上游 Provider
5. 转发请求并流式返回响应
6. 在响应 header 中注入路由元信息 (X-Prism-*)
7. 异步上报结果到 MetricsCollector

---

### 3.3 prism-admin (Go)

管理面，提供配置管理和可观测性。

#### 功能模块

| 模块 | 功能 |
|------|------|
| **配置管理** | 模型池配置、策略配置、Provider 配置，支持热更新 |
| **Dashboard API** | 路由决策分布、成本统计、延迟分布、模型使用率 |
| **告警** | 模型异常、成本超限、延迟飙升 |
| **A/B 测试** | 策略对比实验，自动流量分配 |
| **审计日志** | 路由决策全记录，可回溯 |

---

## 4. 配置设计

### 4.1 PrismConfig 完整结构

```rust
pub struct PrismConfig {
    pub server: ServerConfig,                       // HTTP/gRPC/Proxy 端口
    pub providers: HashMap<String, ProviderConfig>, // 供应商定义
    pub models: Vec<ModelConfig>,                   // 模型元信息
    pub variants: Vec<VariantConfig>,               // 模型变体
    pub pools: Vec<PoolConfig>,                     // 模型池
    pub routing: RoutingConfig,                     // 路由策略
    pub rate_limits: Vec<RateLimitConfig>,           // 限流规则
    pub plugins: Vec<PluginConfig>,                 // 插件描述符 (WASM/Builtin)
    pub discoveries: Vec<DiscoveryConfig>,          // 外部发现源
    pub codec: CodecConfig,                         // Codec/协议转换配置
}
```

### 4.2 VariantConfig 字段

```rust
pub struct VariantConfig {
    pub id: String,
    pub model_id: String,
    pub provider_id: String,
    pub weight: Option<f32>,
    pub account_id: Option<String>,
    pub base_url: Option<String>,               // 自部署 URL 覆盖
    pub deployment_name: Option<String>,        // Azure/GCP 部署名
    pub upstream_model_name: Option<String>,    // 上游模型名映射
    pub pricing_override: Option<ModelPricing>, // 覆盖定价
}
```

### 4.3 完整 YAML 配置示例

```yaml
# prism.yaml

server:
  http_port: 8080
  grpc_port: 9090
  proxy_port: 8081

# 模型池定义
model_pools:
  tier-1:  # 高质量模型
    - model_id: gpt-4o
      provider: openai
      quality_tier: S
      pricing:
        input: 2.5    # $/1M tokens
        output: 10.0
      capabilities:
        max_context: 128000
        vision: true
        function_calling: true
      specialties: [reasoning, code_generation, creative]

    - model_id: claude-sonnet-4-5-20250929
      provider: anthropic
      quality_tier: S
      pricing:
        input: 3.0
        output: 15.0
      capabilities:
        max_context: 200000
        vision: true
        function_calling: true
      specialties: [reasoning, code_review, creative]

  tier-2:  # 性价比模型
    - model_id: gpt-4o-mini
      provider: openai
      quality_tier: A
      pricing:
        input: 0.15
        output: 0.6
      capabilities:
        max_context: 128000
        vision: true
        function_calling: true
      specialties: [simple_qa, translation, summarization]

    - model_id: deepseek-v3
      provider: deepseek
      quality_tier: A
      pricing:
        input: 0.27
        output: 1.1
      capabilities:
        max_context: 64000
        vision: false
        function_calling: true
      specialties: [code_generation, reasoning, math]

  tier-3:  # 低成本模型
    - model_id: deepseek-v3-lite
      provider: deepseek
      quality_tier: B
      pricing:
        input: 0.07
        output: 0.3

# 限流规则
rate_limits:
  - name: per-model-limit
    key_headers: ["x-model"]
    window_secs: 60
    max_requests: 100
    overrides:
      - match_headers: { "x-model": "gpt-4o" }
        max_requests: 30
      - match_headers: { "x-model": "gpt-4o", "x-team": "vip" }
        max_requests: 80

# 插件配置
plugins:
  - name: openai-compat
    source:
      wasm: ./plugins/openai-compat.wasm
    phases: [proxy]
    enabled: true
    priority: 100
  - name: custom-scorer
    source:
      url: https://cdn.example.com/plugins/scorer-v1.0.wasm
      sha256: abc123...
    phases:
      - route:
          weight: 1.5
          stages: [filter, score]
    priority: 200

# 外部发现源
discoveries:
  - name: k8s-discovery
    type:
      wasm:
        path: ./plugins/k8s-discovery.wasm
    enabled: true

# Codec 配置
codec:
  default_chain: []

# 默认路由策略
routing:
  # L1: 模型选择
  model_selection:
    strategy: auto
    objective: balanced
    balanced:
      cost_weight: 0.3
      quality_weight: 0.5
      latency_weight: 0.2
    default_pools: [tier-1, tier-2]

  # L2: Provider 选择
  provider_selection:
    strategy: scored
    scored:
      cost_weight: 0.3
      latency_weight: 0.4
      availability_weight: 0.2
      quota_weight: 0.1

  # L3: Account 选择
  account_selection:
    strategy: affinity_first
    affinity:
      strategy: soft
      key_source:
        type: header
        header: "X-Prism-Session-Id"
      max_load_ratio: 1.5
      ttl_seconds: 3600
    fallback:
      strategy: load_balanced
      algorithm: power_of_two_choices

  # L4: Variant 选择
  variant_selection:
    strategy: best_performance
    metric: latency_p50

  # L5: 容错
  resilience:
    retry:
      max_retries: 2
      retry_on: [rate_limit, timeout, server_error]
      backoff: exponential
      retry_scope: cross_provider
    fallback:
      allow_model_downgrade: true
      notify_client: true
      chain:
        - model: gpt-4o-mini
        - pool: tier-3
    circuit_breaker:
      error_threshold_pct: 50
      window_seconds: 60
      open_duration_seconds: 30
    hedging:
      max_parallel: 2
      delay_ms: 500
      cancel_remaining: true
    timeout:
      connect_ms: 3000
      request_ms: 30000
      ttft_ms: 10000

  # 请求分析器配置
  analyzer:
    tier: feature      # rule | feature | model
    # model_path: ./models/classifier.onnx   # tier=model 时需要

# Provider 配置
providers:
  openai:
    api_key: ${OPENAI_API_KEY}
    base_url: https://api.openai.com/v1
    timeout_ms: 30000
    max_concurrent: 100

  anthropic:
    api_key: ${ANTHROPIC_API_KEY}
    base_url: https://api.anthropic.com
    timeout_ms: 30000

  # 同一 Provider 可以有多个 Endpoint 和 Account
  vertex:
    base_url: https://us-central1-aiplatform.googleapis.com
    endpoints:
      - id: vertex-us-central1
        region: us-central1
      - id: vertex-europe-west1
        region: europe-west1
        base_url: https://europe-west1-aiplatform.googleapis.com
    accounts:
      - id: gcp-project-alpha
        endpoint_id: vertex-us-central1
        project_id: my-project-alpha
        auth:
          type: service_account
          key_file: ${GCP_SA_KEY_ALPHA}
        billing_tag: team-a
      - id: gcp-project-beta
        endpoint_id: vertex-us-central1
        project_id: my-project-beta
        auth:
          type: service_account
          key_file: ${GCP_SA_KEY_BETA}
        billing_tag: team-b

  deepseek:
    api_key: ${DEEPSEEK_API_KEY}
    base_url: https://api.deepseek.com
    timeout_ms: 30000

  local:
    base_url: http://localhost:8000/v1  # vLLM / Ollama
    timeout_ms: 60000

# 可观测性
observability:
  metrics:
    enabled: true
    endpoint: /metrics   # Prometheus
  tracing:
    enabled: true
    exporter: otlp
    endpoint: localhost:4317
  logging:
    level: info
    format: json

# 存储
storage:
  redis:
    addr: localhost:6379
    db: 0
  postgres:
    dsn: postgres://prism:prism@localhost:5432/prism?sslmode=disable
```

---

## 5. 请求流程

### 反向代理模式（Pingora，最简部署）

```
1. 客户端发送 OpenAI 兼容请求 (model="auto") → prism-router :8081
                    │
2. Pingora 反向代理接收请求
   ├── 协议检测 (completions / embeddings / ...)
   └── 提取请求信息
                    │
3. 进程内调用 prism-core（零网络开销）
   ├── Request Analyzer: 分析请求 → RequestProfile
   │   ├── 规则层快速分类 (< 0.1ms)
   │   └── 特征层精细分类 (< 0.5ms, 如需要)
   │
   ├── Plugin Pipeline: 过滤 + 评分
   │   ├── Filter: Health → Capability → CircuitBreaker → Protocol
   │   └── Scorer: Cost → Latency → Quality → Specialty → Strategy → ...
   │
   ├── Strategy Engine (L1-L4): 对候选模型打分
   │   ├── L1: 从 Registry 获取候选，过滤 capabilities，按策略评分
   │   ├── L2: 同模型多 Provider 选择（延迟/成本/可用性）
   │   ├── L3: 同 Provider 多 Account 选择（亲和性/负载均衡）
   │   └── L4: 同 Account 多 Variant 选择（实时性能/灰度）
   │
   └── 返回 RoutingDecision（选中的 variant + 上游连接信息）
                    │
4. Pingora 修改请求目标为选定的 Provider
   ├── 设置上游 base_url + model_name + auth headers
   ├── 转发请求（支持 streaming SSE）
   ├── 如果失败，按 fallback chain 重试 (L5)
   └── 在响应 header 中注入 X-Prism-* 路由元信息
                    │
5. 返回响应给客户端
                    │
6. 异步上报到 MetricsCollector
   ├── 更新模型性能数据（滑动窗口）
   ├── 更新 6 维度 Prometheus 指标
   └── 周期性刷新到 DataLayer（每 5 秒）
```

### HTTP API 模式（与现有网关集成）

```
1. 客户端请求 → 现有网关 (Higress / Envoy / APISIX / 自建)
                    │
2. 网关提取请求信息，调用 prism-router HTTP API
   POST /v1/route
   {
     "messages": [...],
     "strategy": "balanced",
     "model": "auto"
   }
                    │
3. prism-router (Rust) 执行路由决策
   ├── 请求分析 + Plugin Pipeline + L1-L4 策略引擎
   └── 返回 RouteResponse，包含上游连接信息：
       {
         "model": "gpt-4o",
         "provider": "openai",
         "variant": "gpt-4o@openai",
         "upstream": {
           "base_url": "https://api.openai.com/v1",
           "model_name": "gpt-4o",
           "headers": {"Authorization": "Bearer sk-xxx"},
           "timeout_ms": 30000
         },
         "fallback_chain": [...]
       }
                    │
4. 网关根据 upstream 信息转发请求
   ├── 设置目标地址 = upstream.base_url
   ├── 设置请求 model = upstream.model_name
   ├── 附加 upstream.headers
   └── 按 fallback_chain 处理失败重试
                    │
5. 返回响应给客户端
                    │
6. 网关异步调用 POST /v1/outcomes 上报延迟、token 数、状态码等
```

### gRPC 模式（高性能集成）

```
1. 客户端请求 → 现有网关
                    │
2. 网关调用 prism-router gRPC 服务
   RouterService.Route(RouteRequest)
                    │
3. prism-router 返回 RouteResponse（同 HTTP API）
                    │
4-6. 同 HTTP API 模式
```

---

## 6. 质量评估与反馈闭环

### 6.1 离线质量评估

- 收集同一请求在不同模型上的输出
- 使用 LLM-as-Judge（GPT-4 评分）建立质量基线
- 按 TaskType 维度统计每个模型的质量分布
- 定期更新 ModelCapabilities.quality_tier

### 6.2 在线反馈

```yaml
# 反馈 API
POST /v1/feedback
{
  "request_id": "req-xxx",
  "rating": 5,            # 1-5 用户评分
  "preferred_model": null, # 用户觉得应该用哪个模型
  "issue": null            # "too_slow" | "low_quality" | "too_expensive"
}
```

- 反馈数据用于在线调整模型的 quality_score
- 使用 EMA（指数移动平均）平滑更新，避免单次反馈波动过大
- 当某模型质量持续下降时自动降级

### 6.3 A/B 测试

支持对路由策略做 A/B 实验：

```yaml
experiments:
  - name: "new_classifier_v2"
    traffic_pct: 10          # 10% 流量
    strategy_override:
      analyzer:
        tier: model
        model_path: ./models/classifier_v2.onnx
```

---

## 6.4 Discovery 抽象层

Discovery 抽象层将候选对象（Provider、Endpoint、Account、Model、Variant、Pool）的来源从路由引擎中解耦。通过实现 `Discovery` trait，可以从不同来源加载调度对象。

```rust
pub trait Discovery: Send + Sync {
    /// Discover all scheduling objects from this source.
    fn discover(&self) -> Result<DiscoveryResult, DiscoveryError>;

    /// Human-readable name for logging / metrics.
    fn name(&self) -> &str;
}

pub struct DiscoveryResult {
    pub providers: Vec<Provider>,
    pub endpoints: Vec<Endpoint>,
    pub accounts:  Vec<Account>,
    pub models:    Vec<Model>,
    pub variants:  Vec<ModelVariant>,
    pub pools:     Vec<ModelPool>,
}
```

**内置实现：**

| 实现 | 说明 |
|------|------|
| `StaticDiscovery` | 从内存中的配置结构体加载，默认实现 |
| `WasmDiscovery` | 通过 WASM 模块动态发现候选（`wasm` feature-gated，**已实现**） |

### WasmDiscovery 实现

`WasmDiscovery` 基于 wasmtime 运行时加载 WASM 模块，在沙箱中执行发现逻辑。WASM 模块通过导出函数和宿主回调完成双向通信。

**WASM 模块 ABI 契约：**

| 导出函数 | 签名 | 说明 |
|---|---|---|
| `memory` | — | WASM 线性内存 |
| `proxy_on_memory_allocate` | `(size: i32) -> i32` | 内存分配（bump allocator） |
| `prism_discovery_init` | `(config_ptr: i32, config_len: i32) -> i32` | 初始化，0 = 成功 |
| `prism_discovery_discover` | `() -> i32` | 执行发现，0 = 成功 |

**宿主函数（WASM 模块导入）：**

| 宿主函数 | 签名 | 说明 |
|---|---|---|
| `env.proxy_log` | `(level: i32, msg_ptr: i32, msg_len: i32) -> i32` | 日志输出 |
| `env.prism_discovery_set_result` | `(json_ptr: i32, json_len: i32) -> i32` | 将 JSON 编码的 `DiscoveryResult` 回传给宿主 |

WASM 模块在 `prism_discovery_discover` 执行过程中调用 `prism_discovery_set_result`，将发现到的调度对象（JSON 格式）传回宿主，宿主反序列化后构建 `DiscoveryResult`。

自定义 Discovery 实现（如从 Consul/etcd/Kubernetes API 发现）只需实现 `Discovery` trait，或编写 WASM 模块实现上述 ABI。

## 6.5 统一插件注册框架

Prism 的插件系统是**五维**的：一个 WASM 模块可以同时参与多个阶段的处理。统一插件注册框架（`unified_plugin.rs`）提供单一入口管理所有插件的注册、加载和分发。

### PluginDescriptor — 插件描述符

```rust
pub struct PluginDescriptor {
    pub name: String,           // 唯一标识
    pub source: PluginSource,   // 代码来源
    pub phases: Vec<PluginPhase>, // 参与的阶段
    pub config: serde_json::Value, // 插件配置
    pub enabled: bool,          // 是否启用
    pub priority: u32,          // 执行优先级（越小越先）
}
```

### PluginPhase — 五维插件阶段

```rust
pub enum PluginPhase {
    /// Proxy 层 — 拦截/变换 HTTP 请求响应，使用 proxy-wasm ABI
    Proxy,
    /// 路由层 — 过滤/评分模型候选，使用 Prism routing ABI
    Route {
        weight: f32,                // Score 权重
        stages: Vec<RouteStage>,    // 参与的子阶段 (PreFilter/Filter/PostFilter/Score/Reserve)
    },
    /// 分析层 — 提取请求特征，使用 Prism analyzer ABI
    Analyze,
}
```

### PluginSource — 代码来源

```rust
pub enum PluginSource {
    Wasm(String),       // WASM 文件路径
    Builtin(String),    // 内置 Rust 插件（按名称引用）
}
```

### 五维 WASM 插件体系

| 维度 | 阶段 | ABI | 核心导出函数 | 说明 |
|:---:|---|---|---|---|
| **1** | Config | Prism Config ABI | `prism_config_init`, `prism_config_load` | 配置源插件 |
| **2** | Discovery | Prism Discovery ABI | `prism_discovery_init`, `prism_discovery_discover` | 候选发现 |
| **3** | Route | Prism Routing ABI | `prism_route_filter`, `prism_route_score` | 路由过滤/评分 |
| **4** | Proxy | proxy-wasm ABI | `proxy_on_request_headers/body`, `proxy_on_response_headers/body` | 协议翻译 |
| **5** | Analyze | Prism Analyzer ABI | `analyze` | 请求特征提取 |

### 自动能力检测

框架会检查 WASM 模块的导出函数，自动检测支持哪些阶段：

```rust
pub fn detect_capabilities(module_exports: &[String]) -> DetectedCapabilities {
    DetectedCapabilities {
        has_proxy: has("proxy_on_request_headers"),
        has_route: has("prism_route_filter") || has("prism_route_score"),
        has_analyze: has("analyze"),
    }
}
```

如果声明了某阶段但模块未导出对应函数，框架会报告不匹配而非静默失败。

### PluginRegistry — 统一注册表

`PluginRegistry` 管理所有插件的描述符和加载状态：

```rust
pub struct PluginRegistry {
    descriptors: Vec<PluginDescriptor>,
    loaded: Vec<LoadedPlugin>,
}
```

- `register()` — 注册插件描述符（不立即加载）
- `load_all()` — 加载所有已启用的插件，返回按阶段分组的 `LoadResult`
- `set_enabled()` — 运行时启用/禁用插件
- `enabled_descriptors()` — 按 priority 排序返回已启用的描述符

## 6.6 配置源插件化

配置系统从单文件 `config.rs` 拆分为 `config/` 模块目录，实现了可插拔的配置源架构。

### ConfigSource Trait

```rust
pub trait ConfigSource: Send + Sync {
    fn name(&self) -> &str;
    fn load(&self) -> Result<PrismConfig, ConfigError>;
    fn supports_watch(&self) -> bool { false }
    fn watch(&self, callback: Box<dyn Fn(ConfigEvent) + Send + Sync>) -> Result<(), ConfigError>;
}
```

### 内置实现

| 实现 | 说明 |
|------|------|
| `FileSource` | 从本地 YAML/JSON/TOML 文件加载，支持格式自动检测 |
| `EnvSource` | 从环境变量加载（`PRISM_` 前缀），覆盖文件配置 |
| `WasmConfigSource` | 通过 WASM 模块从任意来源加载配置（`wasm` feature-gated） |
| `CompositeSource` | 组合多个配置源，按优先级合并 |

### 配置合并优先级

```rust
pub enum ConfigPriority {
    Base = 0,           // 基础配置（被所有其他源覆盖）
    File = 10,          // 文件配置
    Remote = 20,        // 远程配置中心
    Environment = 30,   // 环境变量
    Override = 40,      // 显式覆盖
}
```

`CompositeSource` 按优先级排序所有配置源，后加载的高优先级源覆盖低优先级源。

### ConfigEvent

```rust
pub enum ConfigEvent {
    FullReload(PrismConfig),   // 完整配置重载
    DiscoveryChanged,          // 发现源变更
    RoutingChanged(RoutingConfig), // 路由策略变更
    RateLimitChanged,          // 限流规则变更
    PluginChanged,             // 插件配置变更
}
```

### 模块目录结构

```
prism-router/src/config/
├── mod.rs            # ConfigSource trait, ConfigError, ConfigEvent, CompositeSource
├── types.rs          # PrismConfig 完整类型定义
├── file_source.rs    # FileSource — 文件加载
├── env_source.rs     # EnvSource — 环境变量加载
├── wasm_source.rs    # WasmConfigSource — WASM 配置源
└── registry.rs       # build_registry() — 从 PrismConfig 构建 ModelRegistry
```

## 6.7 插件加载器 (PluginLoader)

`PluginLoader`（`plugin_loader.rs`）负责将插件源描述解析为可执行的 WASM 字节码，支持四种来源：

### 四种来源

| 来源 | 配置格式 | 说明 |
|------|----------|------|
| **File** | `{ wasm: "./path.wasm" }` | 本地文件路径 |
| **URL** | `{ url: "https://...", sha256: "abc..." }` | HTTP/HTTPS 远程下载 |
| **OCI** | `{ oci: "registry/name:tag", digest: "..." }` | OCI 容器镜像仓库 |
| **Builtin** | `{ builtin: "plugin-name" }` | 内置 Rust 插件（无 WASM 字节码） |

### 缓存策略

```rust
pub struct PluginLoader {
    cache_dir: PathBuf,                       // 磁盘缓存目录 (~/.prism/plugins/cache/)
    memory_cache: RwLock<HashMap<String, Vec<u8>>>,  // 内存缓存 (sha256 → bytes)
}
```

- **磁盘缓存**：下载的 WASM 模块按 SHA-256 哈希命名存储，避免重复下载
- **内存缓存**：已加载的字节码缓存在内存中，同一模块多次引用不重复读取

### SHA-256 校验

URL 和 OCI 来源支持可选的 SHA-256 校验：

```rust
pub struct ResolvedPlugin {
    pub name: String,
    pub wasm_bytes: Vec<u8>,
    pub source_info: String,
    pub sha256: String,      // 自动计算并与声明值比对
}
```

如果配置了 `sha256` 或 `digest`，加载器会在下载后验证，不匹配则返回 `ChecksumMismatch` 错误。

## 6.8 插件管理器 (PluginManager)

`PluginManager`（`plugin_manager.rs`）管理插件的运行时生命周期，提供 load / unload / reload 操作。

### 运行时操作

```rust
pub struct PluginManager {
    loader: PluginLoader,
    plugins: RwLock<HashMap<String, PluginInfo>>,
    listeners: RwLock<Vec<Box<dyn Fn(PluginEvent) + Send + Sync>>>,
}
```

| 操作 | 说明 |
|------|------|
| `load(config)` | 从配置解析 → PluginLoader 加载 → 注册到对应阶段 |
| `unload(name)` | 从所有阶段移除 → 释放资源 |
| `reload(config)` | 原子替换：加载新版本 → 交换 → 卸载旧版本 |
| `list()` | 查询所有已加载插件的状态 |
| `load_all(configs)` | 批量加载，返回成功/失败统计 |

### 事件回调

```rust
pub enum PluginEvent {
    Loaded { name: String, sha256: String },
    Unloaded { name: String },
    Reloaded { name: String, old_sha256: String, new_sha256: String },
    LoadFailed { name: String, error: String },
}
```

通过 `on_event()` 注册监听器，可用于日志记录、指标上报等。

## 6.5 Proxy 插件链 (Codec)

反向代理层的协议翻译通过 **Proxy 插件链** 实现，位于 `prism-router/src/codec/`。设计对齐 proxy-wasm ABI，支持原生 Rust 插件和 WASM 插件。

### ProxyPlugin Trait

```rust
pub trait ProxyPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn on_request_headers(&self, ctx: &mut ProxyContext) -> Action;
    fn on_request_body(&self, ctx: &mut ProxyContext, body: &[u8]) -> (Action, Option<Vec<u8>>);
    fn on_response_headers(&self, ctx: &mut ProxyContext) -> Action;
    fn on_response_body(&self, ctx: &mut ProxyContext, body: &[u8]) -> (Action, Option<Vec<u8>>);
    fn on_response_stream_chunk(&self, ctx: &mut ProxyContext, chunk: &[u8]) -> Option<Vec<u8>>;
}
```

### 4 个 Hook 点

| Hook | 阶段 | 用途 |
|------|------|------|
| `on_request_headers` | 请求头到达 | 路径改写、协议标记、认证注入 |
| `on_request_body` | 请求体到达 | 请求格式转换 (e.g., prompt → messages) |
| `on_response_headers` | 响应头到达 | 添加/修改响应头 |
| `on_response_body` / `on_response_stream_chunk` | 响应体到达 | 响应格式转换、流式 chunk 过滤 |

### ProxyPluginChain

`ProxyPluginChain` 管理多个插件的有序执行：
- **请求阶段**：插件按添加顺序（正序）执行
- **响应阶段**：插件按反向顺序（逆序）执行
- 任何插件返回 `Action::Stop` 则终止链路

### CodecRegistry

`CodecRegistry` 维护 provider/协议对到插件链的映射，根据路由决策的 provider 自动选择正确的协议翻译插件链。

### WASM 宿主层 (wasmtime)

通过 `wasm` feature flag 启用（`prism-router` 的 feature），基于 wasmtime 42 实现 proxy-wasm host ABI 的核心子集：

| proxy-wasm ABI | 支持状态 |
|---|---|
| `proxy_on_request_headers` | ✅ |
| `proxy_on_request_body` | ✅ |
| `proxy_on_response_headers` | ✅ |
| `proxy_on_response_body` | ✅ |
| `proxy_get_header_map_value` | ✅ |
| `proxy_add_header_map_value` | ✅ |
| `proxy_get_buffer_bytes` | ✅ |
| `proxy_set_buffer_bytes` | ✅ |
| `proxy_log` | ✅ |

WASM 插件编译为 `wasm32-unknown-unknown` 或 `wasm32-wasip1` 目标，运行在沙箱examples/wasm-plugins/openai-compat/`](examples/wasm-plugins/openai-compat/) 获取完整示例。

### Pingora Hook 集成

插件链在 Pingora 反向代理的 5 个阶段中被调用：

```
upstream_request_filter  → on_request_headers
  request_body_filter    → on_request_body (+ upstream_model_name 改写)
upstream_response_filter → on_response_headers
  response_body_filter   → on_response_body / on_response_stream_chunk
logging                  → 异步 metrics 上报
```

## 6.6 限流系统

prism-router 内置可配置的限流系统（`ratelimit.rs`），支持灵活的 header-based key 提取和差异化 override。

### 核心特性

- **Header-based key 提取**：从请求 header 中提取任意组合的 key（如 `x-model`、`x-app-id`），实现 per-model、per-app 或全局维度的限流
- **滑动窗口**：基于时间窗口的请求计数和 token 计数限流
- **差异化 override**：同一规则下不同匹配条件可设置不同限额
- **最具体匹配优先**：当多个 override 同时匹配时，匹配条件数最多的 override 生效（避免宽泛规则覆盖精确规则）

### 配置示例

```yaml
rate_limits:
  - name: per-model-limit
    key_headers: ["x-model"]
    window_secs: 60
    max_requests: 100
    overrides:
      # GPT-4o 限制更严格
      - match_headers: { "x-model": "gpt-4o" }
        max_requests: 30
      # GPT-4o + 高优先级团队放宽
      - match_headers: { "x-model": "gpt-4o", "x-team": "vip" }
        max_requests: 80

  - name: global-limit
    key_headers: []        # 空 = 全局限流
    window_secs: 60
    max_requests: 1000
```

当请求的 `x-model: gpt-4o` 且 `x-team: vip` 时，两个 override 都匹配，但第二个有 2 个匹配条件，更具体，因此 `max_requests=80` 生效。

## 7. 项目结构

```
prism/
├── DESIGN.md                    # 本文档
├── README.md                    # 英文 README
├── README.zh-CN.md              # 中文 README
├── LICENSE
├── Makefile                     # 统一构建入口
├── Cargo.toml                   # Workspace 根配置
├── docker-compose.yml
├── Dockerfile
│
├── proto/                       # gRPC 协议定义
│   └── prism/router/v1/
│       └── router.proto
│
├── prism-core/                  # Rust 路由引擎（核心库）
│   ├── Cargo.toml               # crate-type: cdylib + rlib
│   └── src/
│       ├── lib.rs               # 模块声明
│       ├── types.rs             # 调度对象类型 (Model, Variant, Provider, Pool, etc.)
│       ├── analyzer.rs          # 请求分析器 (规则/特征/Webhook)
│       ├── strategy.rs          # L1-L5 策略定义
│       ├── router.rs            # PrismRouter 主路由逻辑
│       ├── selector.rs          # 候选评分与选择
│       ├── registry.rs          # ModelRegistry — 模型/变体/池管理
│       ├── plugin.rs            # RoutingPlugin trait & 管线
│       ├── plugins/             # 18 个内置路由插件
│       │   ├── health_filter.rs
│       │   ├── capability_filter.rs
│       │   ├── circuit_breaker.rs
│       │   ├── protocol_filter.rs
│       │   ├── pinned_model.rs
│       │   ├── cost_scorer.rs
│       │   ├── latency_scorer.rs
│       │   ├── quality_scorer.rs
│       │   ├── specialty_scorer.rs
│       │   ├── feedback_scorer.rs
│       │   ├── geo_proximity.rs
│       │   ├── strategy_scorer.rs
│       │   ├── affinity.rs
│       │   ├── canary.rs
│       │   ├── weighted.rs
│       │   ├── quota_aware.rs           # 配额感知评分
│       │   ├── prefix_affinity.rs       # 前缀亲和（推理引擎缓存复用）
│       │   └── inference_load.rs        # 推理引擎负载感知调度
│       ├── discovery/           # 插件化候选发现层
│       │   ├── mod.rs               # Discovery trait 定义
│       │   ├── static_discovery.rs  # 静态配置发现（默认）
│       │   └── wasm_discovery.rs    # WASM 发现插件（feature-gated）
│       ├── data.rs              # DataLayer — 指标存储抽象
│       ├── feedback.rs          # 反馈收集器 (结果 → 权重调整)
│       └── ffi.rs               # C FFI 接口
│
├── prism-router/                # Rust 独立路由服务
│   ├── Cargo.toml               # features: grpc, proxy, wasm
│   ├── build.rs                 # tonic-build (proto 编译)
│   └── src/
│       ├── main.rs              # 入口: HTTP + gRPC + Proxy 服务
│       ├── api.rs               # Axum HTTP 处理器
│       ├── grpc.rs              # tonic gRPC RouterService
│       ├── proxy.rs             # Pingora 反向代理 + upstream_model_name 改写
│       ├── protocol.rs          # 协议检测
│       ├── metrics.rs           # MetricsCollector — 6 维度 Prometheus 指标
│       ├── config/              # 配置源插件化
│       │   ├── mod.rs               # ConfigSource trait, CompositeSource
│       │   ├── types.rs             # PrismConfig 完整类型定义
│       │   ├── file_source.rs       # FileSource — YAML/JSON/TOML 文件加载
│       │   ├── env_source.rs        # EnvSource — 环境变量加载
│       │   ├── wasm_source.rs       # WasmConfigSource — WASM 配置源 (feature-gated)
│       │   └── registry.rs          # build_registry() — PrismConfig → ModelRegistry
│       ├── state.rs             # AppState (共享 Router + Metrics)
│       ├── ratelimit.rs         # 限流系统 (滑动窗口 + header-based key + override)
│       ├── unified_plugin.rs    # 统一插件注册框架 (PluginRegistry + PluginDescriptor)
│       ├── plugin_loader.rs     # 插件加载器 (File/URL/OCI/Builtin + 缓存 + SHA-256)
│       ├── plugin_manager.rs    # 插件管理器 (load/unload/reload + 事件回调)
│       ├── codec/               # Proxy 插件链 (协议翻译)
│       │   ├── mod.rs               # ProxyPlugin trait + ProxyContext
│       │   ├── chain.rs             # ProxyPluginChain (请求正序, 响应逆序)
│       │   ├── registry.rs          # CodecRegistry (provider/协议对映射)
│       │   └── wasm_host.rs         # wasmtime 宿主层 (feature-gated `wasm`)
│       └── gen/                 # 生成的 protobuf 代码
│
├── examples/
│   └── wasm-plugins/
│       └── openai-compat/       # 示例 WASM 插件 — OpenAI 兼容协议转换
│           ├── Cargo.toml
│           ├── src/lib.rs
│           └── README.md
│
├── configs/
│   ├── prism.example.yaml       # 配置示例
│   ├── prism.yaml               # 实际配置
│   └── prometheus.yml           # Prometheus 采集配置
│
└── docs/                        # 文档 (待补充)
```

---

## 8. 开发路线图

### Phase 1 — MVP ✅ (已完成)
- [x] prism-core: 规则分析器 + 多策略引擎 (Balanced/SLO/CostFirst/QualityFirst)
- [x] prism-core: ModelRegistry + DataLayer + 反馈闭环
- [x] prism-core: 插件系统 (15 个内置 Filter/Scorer 插件)
- [x] prism-core: C FFI 接口
- [x] prism-router: Axum HTTP API (route/outcomes/health/version/metrics)
- [x] prism-router: tonic gRPC RouterService
- [x] prism-router: Pingora 反向代理 (OpenAI 兼容)
- [x] prism-router: YAML 配置加载
- [x] prism-router: 6 维度 Prometheus 指标 + format_prometheus()
- [x] proto 文件定义
- [x] Dockerfile + docker-compose.yml
- [x] Makefile

### Phase 2 — 生产就绪 ✅ (已完成)
- [x] 307 个单元测试通过（含 wasm feature 365 个）
- [x] Discovery 抽象层 — 插件化候选发现 (StaticDiscovery + WasmDiscovery)
- [x] WASM Discovery 实现 — wasmtime 宿主层完整实现
- [x] 推理引擎调度 — PrefixAffinityPlugin + InferenceLoadPlugin (18 个内置插件)
- [x] 多维标签监控指标 — 6 维标签下钻
- [x] Proxy 插件链框架 — ProxyPlugin trait + chain + registry，hook 对齐 proxy-wasm ABI
- [x] WASM 宿主层 — wasmtime 42，proxy-wasm host ABI 核心子集
- [x] Proxy 插件链集成 — Pingora 5 个 hook 接入 codec 插件链
- [x] 示例 WASM 插件 — OpenAI compat E2E
- [x] 限流系统 — header-based key + 滑动窗口 + 差异化 override（最具体匹配优先）
- [x] upstream_model_name — variant 级模型名映射，proxy body 自动改写
- [x] 统一插件注册框架 — PluginRegistry + PluginDescriptor + 五维 WASM 插件体系
- [x] WASM Route + Analyze 适配器 — 完整四阶段 WASM 插件 (Config/Discovery/Route/Proxy/Analyze)
- [x] 配置源插件化 — ConfigSource trait + FileSource + EnvSource + WasmConfigSource + CompositeSource
- [x] 配置类型补全 — rate_limits / plugins / discoveries / codec
- [x] 插件加载器 — File/URL/OCI/Builtin 来源 + 磁盘/内存缓存 + SHA-256 校验
- [x] 插件管理器 — load/unload/reload + 事件回调
- [x] Legacy 清理 — 移除旧 Go 代码
- [x] 配置热更新 (arc-swap 原子替换 + notify 文件监听 + /admin/reload API)
- [ ] Redis 实时状态存储
- [ ] 实际 Provider 请求转发验证 (OpenAI / Anthropic / DeepSeek)
- [ ] 流式转发 (SSE) 完整验证
- [x] Docker 部署配置更新 (纯 Rust Dockerfile + CI)
- [ ] 文档完善 (docs/)

### Phase 3 — 智能化 & 集成
- [ ] 特征分析器 + ONNX 模型分析器
- [ ] A/B 测试框架
- [ ] prism-admin Dashboard
- [ ] Envoy ext_proc 适配
- [ ] Higress / APISIX 集成文档和示例
- [ ] 与万擎平台集成（实时负载感知）

### Phase 4 — 规模化
- [ ] 多集群部署
- [ ] 自定义评分函数 (Plugin)
- [ ] SDK (Python / TypeScript / Go)
- [ ] 开源发布

---

## 9. 性能目标

| 指标 | 目标 |
|------|------|
| 路由决策延迟 (P50) | < 0.5ms |
| 路由决策延迟 (P99) | < 5ms |
| 反向代理额外延迟 (P99) | < 10ms |
| 吞吐量 | > 10K QPS (单实例) |
| 内存占用 | < 100MB |
| 可用性 | 99.99% |

**实测数据：**

| 指标 | 实测值 |
|------|--------|
| 路由决策 | **8μs** |
| P99 延迟 | < 5ms |
| 内存占用 | ~30MB |
| 测试覆盖 | 374 个测试通过（含 wasm）+ 24 个 E2E |

---

## 10. 与竞品差异

| 特性 | Prism | RouteLLM | LiteLLM | Portkey |
|------|-------|----------|---------|---------|
| 智能路由（基于内容） | ✅ 分层分析 + 插件管线 | ✅ 二分类 | ❌ | ❌ |
| 多策略切换 | ✅ 请求级 | ❌ | ❌ | 部分 |
| 反向代理 | ✅ Pingora (高性能) | ❌ | ✅ | ✅ |
| 现有网关集成 | ✅ gRPC/HTTP API | ❌ | ❌ | ❌ |
| 路由插件系统 | ✅ 18 个内置插件 | ❌ | ❌ | ❌ |
| WASM Proxy 插件 | ✅ wasmtime + proxy-wasm ABI | ❌ | ❌ | ❌ |
| 推理引擎调度 | ✅ 前缀亲和 + 负载感知 | ❌ | ❌ | ❌ |
| 限流系统 | ✅ header-based key + 差异化 override | ❌ | 部分 | 部分 |
| upstream_model_name 映射 | ✅ variant 级自动改写 | ❌ | ✅ | ✅ |
| 实时负载感知 | ✅ DataLayer + 反馈闭环 | ❌ | ❌ | ❌ |
| 账号亲和 & 缓存优化 | ✅ | ❌ | ❌ | ❌ |
| 路由决策延迟 | **8μs** | ~10ms | N/A | N/A |
| 6 维度监控 | ✅ Prometheus | ❌ | 部分 | 部分 |
| 反馈闭环 | ✅ Outcome API | ❌ | ❌ | ❌ |
| A/B 测试 | ✅ | ❌ | ❌ | ❌ |
| OpenAI 兼容 | ✅ | ✅ | ✅ | ✅ |

---

*Last updated: 2026-03-01*
*Author: Zoe & 小Z & 营造*
