<div align="center">

# 🔺 Prism

**High-Performance LLM Intelligent Routing Engine**

*Route every request to the optimal model — automatically.*

[![Rust](https://img.shields.io/badge/Rust-Core_Engine-orange?logo=rust)](prism-core/)
[![Go](https://img.shields.io/badge/Go-API_Gateway-00ADD8?logo=go)](prism-gateway/)
[![Tests](https://img.shields.io/badge/Tests-205_passing-brightgreen)]()
[![License](https://img.shields.io/badge/License-MIT-blue)]()

[English](#-why-prism) · [中文](#-为什么需要-prism)

</div>

---

## 🤔 Why Prism?

When you use multiple LLMs (GPT-4o, Claude, DeepSeek, etc.) simultaneously, you face these problems:

- 💸 Simple questions on GPT-4o are expensive; complex questions on mini models give poor results
- 🔄 Provider rate-limited? Manual switching is painful
- 💰 Different teams need separate billing accounts
- 🧪 Want to A/B test models but lack the infrastructure

**Prism solves this.** It analyzes request content, determines complexity, and picks the optimal model + provider in **under 8 microseconds** — based on your strategy preferences (save money? low latency? best quality?).

### Key Features

| | Feature | Description |
|---|---|---|
| 🧠 | **Smart Routing** | Analyzes request content (code / reasoning / translation / chat) to match the best model |
| ⚡ | **Blazing Fast** | Rust core engine, **8μs** routing decisions, P99 < 5ms |
| 🎯 | **Multi-Strategy** | Balanced / SLO / Cost-First / Quality-First — switchable per request |
| 🔌 | **Drop-in** | OpenAI-compatible API — just change `base_url` |
| 📊 | **Observable** | Prometheus metrics, request tracing, cost analysis |
| 🔄 | **Feedback Loop** | Dynamically adjusts routing weights based on actual outcomes |
| 🧪 | **A/B Testing** | Config-driven experiments with traffic splitting |
| 🏗️ | **Flexible Deploy** | FFI embedded (< 0.5ms) / Standalone HTTP / Envoy ext_proc |
| 🔧 | **Extensible Analyzer** | Built-in rules / YAML config / Webhook / WASM plugins |

## 🏛️ Architecture

```
                                    ┌──────────────────────────────┐
                                    │      prism-core (Rust)       │
                                    │                              │
Client ───▶ prism-gateway (Go) ─FFI─▶  L1 Model Selection        │
                 │                  │  L2 Provider Selection       │
                 │                  │  L3 Account Selection        │
                 │                  │  L4 Variant Selection        │
                 │                  │  L5 Resilience (retry/cb)    │
                 │                  │  Feedback Collector           │
                 │                  └──────────────────────────────┘
                 │
                 ├───▶ OpenAI
                 ├───▶ Anthropic
                 ├───▶ DeepSeek
                 └───▶ Any OpenAI-compatible provider
```

### 5-Layer Scheduling Engine

| Layer | Responsibility | Example |
|:---:|---|---|
| **L1** | Model Selection | *"This is a code question → pick GPT-4o"* |
| **L2** | Provider Selection | *"User is in us-east → pick nearest endpoint"* |
| **L3** | Account Selection | *"team-a's request → route to team-a's billing account"* |
| **L4** | Variant Selection | *"Send 10% traffic to canary version"* |
| **L5** | Resilience | *"Provider timeout → auto retry / fallback"* |

## 🚀 Quick Start

### Build

```bash
# Requires Rust 1.82+ and Go 1.22+
make build
```

### Configure

```bash
cp configs/prism.example.yaml configs/prism.yaml
# Edit prism.yaml with your API keys
```

<details>
<summary>📄 Example configuration</summary>

```yaml
server:
  http_port: 8080

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

### Run

```bash
# Direct
./bin/prism --config configs/prism.yaml

# Or Docker
docker compose up -d
```

### Use

```bash
# Same as OpenAI API — just change base_url
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "auto",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

The response includes routing metadata in headers:

```
X-Prism-Model: gpt-4o-mini
X-Prism-Provider: openai
X-Prism-Variant: gpt-4o-mini@openai
X-Prism-Decision-Time-Us: 8
```

## 🎯 Routing Strategies

Specify strategy per request via header or body:

```bash
# Cost First — auto-select cheapest model for simple questions
curl -H "X-Prism-Strategy: cost_first" ...

# SLO First — latency must be under 500ms
curl -H "X-Prism-Strategy: slo_first" \
     -H "X-Prism-Max-Latency-Ms: 500" ...

# Quality First — pick the strongest model
curl -H "X-Prism-Strategy: quality_first" ...

# Balanced — custom weights
curl -H "X-Prism-Strategy: balanced" \
     -H "X-Prism-Cost-Weight: 0.5" \
     -H "X-Prism-Quality-Weight: 0.3" \
     -H "X-Prism-Latency-Weight: 0.2" ...

# Pinned — bypass routing, use specific model
curl -d '{"model": "gpt-4o", ...}'
```

## 🔧 Advanced Features

<details>
<summary><b>Session Affinity</b> — improve KV cache hit rate</summary>

```bash
curl -H "X-Prism-Session-Id: session-abc123" ...
```

Same session routes to the same provider/account. Soft migration on overload.

</details>

<details>
<summary><b>Tenant Cost Isolation</b> — per-team billing</summary>

```bash
curl -H "X-Prism-Tenant-Id: team-backend" ...
```

Routes to the account with matching `billing_tag`.

</details>

<details>
<summary><b>Platform-Specific Rules</b> — different rules for different sources</summary>

```yaml
analyzer:
  scopes:
    - name: mobile_users
      match_on:
        sources: [mobile, ios, android]
      rules:
        - keywords: [help]
          task_type: simple_qa
          complexity: 0.1
```

```bash
curl -H "X-Prism-Source: mobile" -H "X-Prism-Caller-Type: user" ...
```

</details>

<details>
<summary><b>A/B Testing</b> — config-driven experiments</summary>

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

Consistent hashing ensures the same user always sees the same variant.

</details>

<details>
<summary><b>Outcome Feedback</b> — dynamic weight adjustment</summary>

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

Prism adjusts routing scores based on real-world outcomes.

</details>

## 🏗️ Deployment Modes

| Mode | Routing Latency | Use Case |
|---|---|---|
| **A: FFI Embedded** | < 0.5ms | Best performance, self-contained |
| **B: Standalone Service** | 1-3ms | Integrate with existing gateways (Higress/Envoy/APISIX) |
| **C: Envoy ext_proc** | 1-3ms | Deep Envoy ecosystem integration |

```bash
# Mode A (default) — FFI embedded
./bin/prism --config configs/prism.yaml

# Mode B — standalone routing service
./bin/prism-router --config configs/prism.yaml --listen :8081
```

## 📊 Observability

```bash
# Prometheus metrics
curl http://localhost:8080/metrics
```

| Metric | Description |
|---|---|
| `prism_routing_decision_duration_seconds` | Routing decision latency |
| `prism_provider_request_duration_seconds` | Provider request latency |
| `prism_provider_requests_total` | Request count by provider/model/status |
| `prism_tokens_total` | Token usage (prompt/completion) |
| `prism_active_requests` | Active concurrent requests |
| `prism_ttft_seconds` | Time to first token (streaming) |

## 🛠️ Admin Dashboard

```bash
./bin/prism-admin --config configs/prism.yaml --listen :8081
```

| Endpoint | Description |
|---|---|
| `GET /api/v1/status` | System health |
| `GET /api/v1/providers` | Provider list + status |
| `GET /api/v1/models` | Model list + capabilities |
| `GET /api/v1/variants` | Variants + real-time performance |
| `GET /api/v1/dashboard/overview` | Aggregated dashboard |
| `GET /api/v1/experiments` | A/B test results |

## 📁 Project Structure

```
prism/
├── prism-core/          # 🦀 Rust routing engine
│   ├── src/
│   │   ├── analyzer.rs      # Request analyzer (rules/YAML/webhook/WASM)
│   │   ├── strategy.rs      # L1-L5 scheduling strategies
│   │   ├── router.rs        # PrismRouter main logic
│   │   ├── registry.rs      # Model registry
│   │   ├── feedback.rs      # Feedback collector
│   │   ├── ffi.rs           # C FFI interface
│   │   └── types.rs         # Core type definitions
│   └── tests/               # 76 tests
├── prism-gateway/       # 🐹 Go API gateway
│   ├── internal/
│   │   ├── gateway/         # HTTP handler + streaming
│   │   ├── provider/        # OpenAI / Anthropic / DeepSeek
│   │   ├── router/          # FFI bridge + strategy mapping
│   │   ├── metrics/         # Prometheus collectors
│   │   ├── state/           # Redis / in-memory state store
│   │   ├── abtesting/       # A/B testing framework
│   │   └── config/          # YAML config loading
│   └── tests/e2e/           # E2E tests
├── prism-router/        # 📡 Standalone routing service (Mode B)
├── prism-admin/         # 🖥️ Admin dashboard backend
├── proto/               # 📋 Protobuf definitions
├── configs/             # ⚙️ Configuration + Prometheus
├── Dockerfile           # 🐳 Multi-stage build (Rust + Go)
└── docker-compose.yml   # Prism + Redis + Prometheus
```

## 🧑‍💻 Development

```bash
make build           # Build everything
make test-all        # Run all tests (Rust + Go)
make test-core       # Rust tests (76)
make test-e2e        # Go E2E tests
make test-ffi        # Go FFI tests
make fmt             # Format code
make lint            # Lint check
make docker-up       # Start with Docker
```

## ⚡ Performance

| Metric | Measured | Target |
|---|---|---|
| Routing decision | **8μs** | < 500μs |
| FFI call overhead | < 0.01ms | < 0.5ms |
| Gateway overhead | < 5ms | < 10ms |
| 1000 routing decisions | < 50ms | — |

## 🧱 Tech Stack

| Component | Language | Purpose |
|---|---|---|
| prism-core | Rust | Routing engine, request analysis, scoring |
| prism-gateway | Go | API gateway, provider integration, streaming |
| prism-router | Go + Rust FFI | Standalone routing service |
| prism-admin | Go | Admin dashboard API |
| State store | Redis (optional) | Real-time QPS / latency / error rate |
| Monitoring | Prometheus | Metrics collection |

## 📖 Documentation

- [Full Technical Design Document](./DESIGN.md) — Architecture, strategy engine, object model, deployment modes
- [Configuration Example](./configs/prism.example.yaml)
- [Protobuf Definitions](./proto/prism/router/v1/router.proto)

---

<div align="center">

# 🔺 Prism 棱镜

**高性能大模型智能路由引擎**

*一束请求进来，根据策略折射到最优模型。*

</div>

---

## 🤔 为什么需要 Prism？

当你同时使用 GPT-4o、Claude、DeepSeek 等多个模型时：

- 💸 简单问题用 GPT-4o 太贵，复杂问题用 mini 效果差
- 🔄 Provider 限流了，手动切换很痛苦
- 💰 不同团队需要走不同的计费账号
- 🧪 想 A/B 测试对比模型效果，但缺基础设施

**Prism 自动解决这些问题**：分析请求内容判断复杂度，结合你的策略偏好，在 **8 微秒**内选出最优的模型和 Provider。

### 核心特性

- 🧠 **智能路由** — 分析请求内容（代码/推理/翻译/闲聊），自动匹配模型
- ⚡ **极致性能** — Rust 核心引擎，路由决策 8μs，P99 < 5ms
- 🎯 **多策略** — Balanced / SLO / CostFirst / QualityFirst，请求级切换
- 🔌 **即插即用** — OpenAI 兼容 API，改一行 base_url 就能接入
- 📊 **可观测** — Prometheus 指标、请求追踪、成本分析
- 🔄 **反馈闭环** — 根据实际效果动态调整路由权重
- 🧪 **A/B 测试** — 配置化实验，流量分流对比
- 🏗️ **灵活部署** — FFI 内嵌 / 独立 HTTP 服务 / Envoy ext_proc
- 🔧 **可扩展** — 规则引擎 / YAML 配置 / Webhook / WASM 插件

### 五层调度引擎

| 层 | 职责 | 示例 |
|:---:|---|---|
| **L1** | 模型选择 | *"这是代码问题 → 选 GPT-4o"* |
| **L2** | Provider 选择 | *"用户在 us-east → 选最近的节点"* |
| **L3** | Account 选择 | *"team-a 的请求 → 走 team-a 的计费账号"* |
| **L4** | Variant 选择 | *"10% 流量给 canary 版本"* |
| **L5** | 容错 | *"Provider 超时 → 自动重试/降级"* |

### 快速开始

```bash
# 构建（需要 Rust 1.82+ 和 Go 1.22+）
make build

# 配置
cp configs/prism.example.yaml configs/prism.yaml
# 编辑填入 API key

# 运行
./bin/prism --config configs/prism.yaml

# 使用（和 OpenAI API 完全一样）
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"auto","messages":[{"role":"user","content":"你好！"}]}'
```

详细文档请参考上方英文部分和 [DESIGN.md](./DESIGN.md)。

---

## License

MIT
