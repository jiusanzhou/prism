<div align="center">

# 🔺 Prism 棱镜

**高性能大模型智能路由引擎**

*一束请求进来，根据策略折射到最优模型。*

[![Rust](https://img.shields.io/badge/Rust-核心引擎-orange?logo=rust)](prism-core/)
[![Tests](https://img.shields.io/badge/测试-97_通过-brightgreen)]()
[![License](https://img.shields.io/badge/License-MIT-blue)]()

[English](./README.md) · [中文](./README.zh-CN.md)

</div>

---

## 为什么需要 Prism？

当你同时使用 GPT-4o、Claude、DeepSeek 等多个模型时：

- 💸 简单问题用 GPT-4o 太贵，复杂问题用 mini 效果差
- 🔄 Provider 限流了，手动切换很痛苦
- 💰 不同团队需要走不同的计费账号
- 🧪 想 A/B 测试对比模型效果，但缺基础设施

**Prism 自动解决这些问题**：分析请求内容判断复杂度，结合你的策略偏好，在 **8 微秒**内选出最优的模型和 Provider。

### 核心特性

| | 特性 | 说明 |
|---|---|---|
| 🧠 | **智能路由** | 分析请求内容（代码/推理/翻译/闲聊），自动匹配模型 |
| ⚡ | **极致性能** | Rust 核心引擎，路由决策 8μs，P99 < 5ms |
| 🎯 | **多策略** | Balanced / SLO / CostFirst / QualityFirst，请求级切换 |
| 🔌 | **即插即用** | OpenAI 兼容 API，改一行 base_url 就能接入 |
| 📊 | **可观测** | Prometheus 6 维度监控指标、请求追踪、成本分析 |
| 🔄 | **反馈闭环** | 根据实际效果动态调整路由权重 |
| 🧪 | **A/B 测试** | 配置化实验，流量分流对比 |
| 🏗️ | **灵活部署** | HTTP + gRPC + 反向代理 (Pingora) 三种接口 |
| 🔧 | **插件系统** | 可扩展的 Filter/Scorer 插件，自定义路由逻辑 |

## 架构

Prism 是一个 Rust workspace，包含两个 crate：

```
                          ┌──────────────────────────────────────┐
                          │         prism-router (Rust)          │
                          │                                      │
                          │  HTTP API ──┐                        │
  客户端 ─────────────────│  gRPC API ──┼──▶ prism-core (Rust)  │
  (OpenAI 兼容)           │  Proxy   ───┘   │                   │
                          │                 ├─ 请求分析器        │
                          │                 ├─ 插件系统          │
                          │                 ├─ 策略引擎          │
                          │                 ├─ 模型注册表        │
                          │                 ├─ DataLayer         │
                          │                 └─ 反馈闭环          │
                          └──────────────────────────────────────┘
                                        │
                          ┌─────────────┼──────────────┐
                          │             │              │
                          ▼             ▼              ▼
                       OpenAI      Anthropic       DeepSeek
                                                  (任何 OpenAI 兼容)
```

### 五层调度引擎

| 层 | 职责 | 示例 |
|:---:|---|---|
| **L1** | 模型选择 | *"这是代码问题 → 选 GPT-4o"* |
| **L2** | Provider 选择 | *"用户在 us-east → 选最近的节点"* |
| **L3** | Account 选择 | *"team-a 的请求 → 走 team-a 的计费账号"* |
| **L4** | Variant 选择 | *"10% 流量给 canary 版本"* |
| **L5** | 容错 | *"Provider 超时 → 自动重试/降级"* |

### 服务接口

prism-router 同时暴露三种接口，通过 feature flag 控制：

| 接口 | Feature Flag | 默认端口 | 用途 |
|---|---|---|---|
| **HTTP API** | *(始终开启)* | 8080 | REST 路由决策、健康检查、Prometheus 指标 |
| **gRPC** | `grpc` | 9090 | 高性能路由服务，与网关集成 |
| **反向代理** | `proxy` | 8081 | OpenAI 兼容反向代理 (Pingora)，路由 + 转发一体 |

## 快速开始

### 构建

```bash
# 需要 Rust 1.82+
cargo build --release

# 全部功能（默认）
cargo build --release -p prism-router

# 仅 HTTP（不含 gRPC 和 Proxy）
cargo build --release -p prism-router --no-default-features
```

### 配置

```bash
cp configs/prism.example.yaml configs/prism.yaml
# 编辑填入 API key 和模型定义
```

<details>
<summary>📄 配置示例</summary>

```yaml
server:
  http_port: 8080
  grpc_port: 9090
  proxy_port: 8081

providers:
  openai:
    type: openai
    api_key: ${OPENAI_API_KEY}
    base_url: https://api.openai.com/v1
  anthropic:
    type: anthropic
    api_key: ${ANTHROPIC_API_KEY}
    base_url: https://api.anthropic.com
  deepseek:
    type: deepseek
    api_key: ${DEEPSEEK_API_KEY}
    base_url: https://api.deepseek.com

models:
  - id: gpt-4o
    quality_tier: S
    pricing: { input_per_1m_tokens: 2.5, output_per_1m_tokens: 10.0 }
    specialties: [Reasoning, CodeGeneration]
  - id: gpt-4o-mini
    quality_tier: A
    pricing: { input_per_1m_tokens: 0.15, output_per_1m_tokens: 0.6 }
    specialties: [SimpleQA]
  - id: deepseek-chat
    quality_tier: A
    pricing: { input_per_1m_tokens: 0.27, output_per_1m_tokens: 1.1 }
    specialties: [Reasoning, CodeGeneration]

variants:
  - { id: "gpt-4o@openai", model_id: gpt-4o, provider_id: openai }
  - { id: "gpt-4o-mini@openai", model_id: gpt-4o-mini, provider_id: openai }
  - { id: "deepseek-chat@deepseek", model_id: deepseek-chat, provider_id: deepseek }

pools:
  - id: default
    variants: [gpt-4o@openai, gpt-4o-mini@openai, deepseek-chat@deepseek]

routing:
  default_strategy: balanced
  balanced: { cost_weight: 0.3, quality_weight: 0.5, latency_weight: 0.2 }
```

</details>

### 运行

```bash
# 直接运行
./target/release/prism-router --config configs/prism.yaml

# 或 Docker
docker compose up -d
```

### 使用

**HTTP API — 路由决策：**

```bash
curl -X POST http://localhost:8080/v1/route \
  -H "Content-Type: application/json" \
  -d '{
    "messages": [{"role": "user", "content": "用 Rust 写一个快排"}],
    "strategy": "balanced"
  }'
```

**反向代理 — OpenAI 直接替换（端口 8081）：**

```bash
# 和 OpenAI API 完全一样，改一行 base_url 就行
curl http://localhost:8081/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "auto",
    "messages": [{"role": "user", "content": "你好！"}]
  }'
```

代理响应会在 header 中返回路由信息：

```
X-Prism-Model: gpt-4o-mini
X-Prism-Provider: openai
X-Prism-Variant: gpt-4o-mini@openai
X-Prism-Decision-Time-Us: 8
```

## 路由策略

支持通过 header 或 body 指定请求级策略：

```bash
# 成本优先 — 简单问题自动选最便宜的
curl -H "X-Prism-Strategy: cost_first" ...

# SLO 优先 — 延迟必须低于 500ms
curl -H "X-Prism-Strategy: slo_first" \
     -H "X-Prism-Max-Latency-Ms: 500" ...

# 质量优先 — 选最强模型
curl -H "X-Prism-Strategy: quality_first" ...

# 均衡 — 自定义权重
curl -H "X-Prism-Strategy: balanced" \
     -H "X-Prism-Cost-Weight: 0.5" \
     -H "X-Prism-Quality-Weight: 0.3" \
     -H "X-Prism-Latency-Weight: 0.2" ...

# 指定模型 — 跳过路由
curl -d '{"model": "gpt-4o", ...}'
```

## 高级功能

<details>
<summary><b>会话亲和</b> — 提高 KV Cache 命中率</summary>

```bash
curl -H "X-Prism-Session-Id: session-abc123" ...
```

同一会话路由到同一 Provider/Account，提高缓存复用。过载时自动软迁移。

</details>

<details>
<summary><b>租户成本隔离</b> — 按团队计费</summary>

```bash
curl -H "X-Prism-Tenant-Id: team-backend" ...
```

路由到匹配 `billing_tag` 的账号。

</details>

<details>
<summary><b>A/B 测试</b> — 配置化实验</summary>

```yaml
experiments:
  - id: gpt4-vs-claude
    enabled: true
    traffic_split:
      "gpt-4o@openai": 50
      "claude@anthropic": 50
    target:
      pool_ids: [default]
      sample_rate: 100
```

一致性哈希保证同一用户始终看到同一变体。

</details>

<details>
<summary><b>结果反馈</b> — 动态调权</summary>

```bash
curl -X POST http://localhost:8080/v1/outcomes \
  -d '{
    "variant_id": "gpt-4o@openai",
    "success": true,
    "total_latency_ms": 1200,
    "prompt_tokens": 50,
    "completion_tokens": 200,
    "user_rating": 5
  }'
```

Prism 根据实际效果通过反馈闭环动态调整路由评分。

</details>

<details>
<summary><b>插件系统</b> — 可扩展路由逻辑</summary>

prism-core 使用插件管线进行路由决策：

- **过滤插件** — 移除不满足要求的候选（能力、健康度、熔断、协议）
- **评分插件** — 对候选打分（成本、延迟、质量、专长、反馈、地理亲和、策略权重）

内置插件：`HealthFilter`、`CapabilityFilter`、`CircuitBreaker`、`ProtocolFilter`、`PinnedModel`、`CostScorer`、`LatencyScorer`、`QualityScorer`、`SpecialtyScorer`、`FeedbackScorer`、`GeoProximity`、`StrategyScorer`、`AffinityScorer`、`Canary`、`Weighted`

自定义插件实现 `RoutingPlugin` trait 即可。

</details>

## 可观测性

```bash
# Prometheus 指标
curl http://localhost:8080/metrics
```

### 6 维度监控指标

| 维度 | 指标 |
|---|---|
| **路由决策** | `prism_routing_decision_duration_us`（直方图）、`prism_strategy_calls_total`、`prism_fallback_triggered_total`、插件 filter/score 耗时 |
| **Provider** | `prism_provider_requests_total`、`prism_provider_errors_total`、`prism_provider_latency_sum_ms`、`prism_rate_limit_utilization`、`prism_circuit_breaker_state` |
| **流量分布** | `prism_routes_by_model_total`、`prism_routes_by_provider_total`、`prism_routes_by_account_total`、`prism_experiment_assignments_total` |
| **Token 用量** | `prism_input_tokens_total`、`prism_output_tokens_total`、`prism_tokens_per_second` |
| **资源** | `prism_active_requests`、`prism_queue_depth` |
| **DataLayer** | `prism_data_layer_queries_total`、`prism_data_layer_hits_total`、`prism_data_layer_latency_sum_us` |

每个 variant 的延迟百分位（P50/P95/P99）、错误率、QPS、TTFT 也会导出。

## 项目结构

```
prism/
├── prism-core/              # 🦀 Rust 路由引擎（库）
│   ├── src/
│   │   ├── lib.rs               # 模块声明
│   │   ├── types.rs             # 核心类型：Model、Variant、Provider、Pool 等
│   │   ├── analyzer.rs          # 请求分析器（规则/特征/Webhook）
│   │   ├── strategy.rs          # L1-L5 策略定义
│   │   ├── router.rs            # PrismRouter — 主路由逻辑
│   │   ├── selector.rs          # 候选评分与选择
│   │   ├── registry.rs          # ModelRegistry — 模型/变体/池管理
│   │   ├── plugin.rs            # RoutingPlugin trait 与管线
│   │   ├── plugins/             # 15 个内置插件（过滤 + 评分）
│   │   ├── data.rs              # DataLayer — 指标存储抽象
│   │   ├── feedback.rs          # 反馈收集器（结果 → 权重调整）
│   │   └── ffi.rs               # C FFI 接口（用于嵌入）
│   └── Cargo.toml
├── prism-router/            # 🚀 独立路由服务
│   ├── src/
│   │   ├── main.rs              # 入口：HTTP + gRPC + Proxy 服务
│   │   ├── api.rs               # Axum HTTP 处理器（route、outcomes、health、metrics）
│   │   ├── grpc.rs              # tonic gRPC RouterService 实现
│   │   ├── proxy.rs             # Pingora 反向代理（OpenAI 兼容）
│   │   ├── protocol.rs          # 协议检测（completions、embeddings 等）
│   │   ├── metrics.rs           # MetricsCollector — 6 维度 Prometheus 指标
│   │   ├── config.rs            # YAML 配置加载 → Registry 构建
│   │   └── state.rs             # AppState（共享 Router + Metrics）
│   └── Cargo.toml
├── proto/                   # 📋 Protobuf 定义
│   └── prism/router/v1/
│       └── router.proto
├── configs/                 # ⚙️ 配置文件
│   ├── prism.example.yaml
│   ├── prism.yaml
│   └── prometheus.yml
├── Makefile                 # 构建与测试命令
├── Dockerfile               # 多阶段构建（Rust + Go FFI 网关）
├── docker-compose.yml       # Prism + Redis + PostgreSQL + Prometheus
└── DESIGN.md                # 完整技术设计文档
```

## 开发

```bash
cargo build --release        # 构建
cargo test                   # 运行测试（97 个通过）
cargo bench                  # 性能基准测试（prism-core）
cargo fmt --all              # 格式化代码
cargo clippy -- -D warnings  # 代码检查
```

### Feature Flags

| Flag | 默认 | 说明 |
|---|---|---|
| `grpc` | ✅ | 启用 gRPC RouterService (tonic) |
| `proxy` | ✅ | 启用 Pingora 反向代理 |
| `wasm` | ❌ | 启用 WASM 插件支持 (wasmtime)，prism-core |

## 性能

| 指标 | 实测 | 目标 |
|---|---|---|
| 路由决策 | **8μs** | < 500μs |
| P99 延迟 | < 5ms | < 5ms |
| 内存占用 | ~30MB | < 100MB |

## 技术栈

| 组件 | 技术 | 用途 |
|---|---|---|
| prism-core | Rust | 路由引擎、请求分析、插件评分 |
| prism-router | Rust (Axum + tonic + Pingora) | HTTP API、gRPC 服务、反向代理 |
| 监控 | Prometheus | 6 维度指标采集 |
| 状态存储（可选） | Redis | 实时 QPS / 延迟 / 错误率 |
| 日志存储（可选） | PostgreSQL | 审计日志和成本分析 |

## 文档

- [完整技术设计文档](./DESIGN.md) — 架构、5 层策略引擎、对象模型、部署模式
- [配置示例](./configs/prism.example.yaml)
- [Protobuf 定义](./proto/prism/router/v1/router.proto)

## License

MIT
