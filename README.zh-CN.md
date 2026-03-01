<div align="center">

# 🔺 Prism 棱镜

**高性能大模型智能路由引擎**

*一束请求进来，根据策略折射到最优模型。*

[![Rust](https://img.shields.io/badge/Rust-核心引擎-orange?logo=rust)](prism-core/)
[![Tests](https://img.shields.io/badge/测试-803_通过-brightgreen)]()
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

**Prism 自动解决这些问题**：分析请求内容判断复杂度，结合你的策略偏好，在 **90 微秒**内选出最优的模型和 Provider。

### 核心特性

| | 特性 | 说明 |
|---|---|---|
| 🧠 | **智能路由** | 分析请求内容（代码/推理/翻译/闲聊），自动匹配模型 |
| ⚡ | **极致性能** | Rust 核心引擎，路由决策 90μs（100 variants），线性扩展 |
| 🎯 | **多策略** | Balanced / SLO / CostFirst / QualityFirst，请求级切换 |
| 🔌 | **即插即用** | OpenAI 兼容 API，改一行 base_url 就能接入 |
| 📊 | **可观测** | Prometheus 6 维度监控指标、请求追踪、成本分析 |
| 🔄 | **反馈闭环** | 根据实际效果动态调整路由权重 |
| 🧪 | **A/B 测试** | 配置化实验，流量分流对比 |
| 🏗️ | **灵活部署** | HTTP + gRPC + 反向代理 (Pingora) 三种接口 |
| 🔧 | **插件系统** | 可扩展的 Filter/Scorer 插件，自定义路由逻辑 |
| 🧩 | **五维 WASM 插件** | Config / Discovery / Route / Proxy / Analyze — 全部支持 WASM 扩展 |
| ⚙️ | **配置源插件化** | ConfigSource trait，支持文件、环境变量、WASM、组合源 |

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
                          │                 ├─ Discovery 层      │
                          │                 ├─ DataLayer         │
                          │                 └─ 反馈闭环          │
                          │                                      │
                          │  ConfigSource (文件/环境变量/WASM)    │
                          │  PluginLoader (文件/URL/OCI)         │
                          │  PluginManager (加载/卸载/重载)       │
                          │                                      │
                          │  Proxy 层：                          │
                          │  ├─ Codec 插件链 (WASM/Native)       │
                          │  ├─ 限流系统                         │
                          │  └─ 模型名改写                       │
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
X-Prism-Decision-Time-Us: 92
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

- **过滤插件** — 移除不满足要求的候选（能力、健康度、熔断、协议、推理引擎负载）
- **评分插件** — 对候选打分（成本、延迟、质量、专长、反馈、地理亲和、策略权重、前缀亲和、推理负载）

内置插件（18 个）：`HealthFilter`、`CapabilityFilter`、`CircuitBreaker`、`ProtocolFilter`、`PinnedModel`、`CostScorer`、`LatencyScorer`、`QualityScorer`、`SpecialtyScorer`、`FeedbackScorer`、`GeoProximity`、`StrategyScorer`、`AffinityScorer`、`Canary`、`Weighted`、`QuotaAware`、`PrefixAffinity`、`InferenceLoad`

**推理引擎调度插件**（适用于 vLLM / SGLang 等自部署推理引擎）：
- `PrefixAffinityPlugin` — 基于 prompt 前缀的一致性哈希路由，最大化前缀缓存复用
- `InferenceLoadPlugin` — 过滤 KV Cache 使用率 > 95% 的节点，按 GPU 利用率和队列深度评分

自定义插件实现 `RoutingPlugin` trait 即可。

</details>

<details>
<summary><b>WASM Proxy 插件</b> — 使用 WASM 扩展代理层</summary>

prism-router 支持通过完整的 codec 插件链框架将代理层插件编译为 WebAssembly。WASM 插件可在反向代理层拦截 HTTP 请求/响应，用于协议翻译、Header 操作和 Body 变换。

插件链对齐 **proxy-wasm ABI**，支持：
- **4 个 Hook 点**：`on_request_headers`、`on_request_body`、`on_response_headers`、`on_response_body`/`on_response_stream_chunk`
- **ProxyPluginChain**：请求阶段正序执行，响应阶段逆序执行
- **CodecRegistry**：按 provider/协议对映射到对应的插件链
- **wasmtime 42 宿主**：实现 proxy-wasm ABI 核心子集（get/set headers、get/set buffer、log）

**示例：OpenAI 兼容插件** ([`examples/wasm-plugins/openai-compat/`](examples/wasm-plugins/openai-compat/))

将非标请求标准化为 OpenAI 格式：
- 改写 `/api/chat` → `/v1/chat/completions`
- 添加 `X-Prism-Converted: true` Header
- 转换 `{"prompt": "..."}` → `{"messages": [...]}`
- 在响应中添加 `"prism_converted": true`

```bash
# 构建示例插件
cd examples/wasm-plugins/openai-compat
cargo build --target wasm32-unknown-unknown --release

# 运行 WASM 集成测试
make test-wasm
```

启用方式：`cargo build -p prism-router --features wasm`

</details>

<details>
<summary><b>五维 WASM 插件系统</b> — 单模块多阶段</summary>

一个 WASM 模块可以同时参与多个阶段的处理。统一插件框架会自动检测模块导出函数来确定支持的能力：

| 维度 | 阶段 | ABI | 核心导出函数 | 说明 |
|:---:|---|---|---|---|
| **1** | Config | Prism Config ABI | `prism_config_init`, `prism_config_load` | 配置源插件 |
| **2** | Discovery | Prism Discovery ABI | `prism_discovery_init`, `prism_discovery_discover` | 候选发现 |
| **3** | Route | Prism Routing ABI | `prism_route_filter`, `prism_route_score` | 路由过滤/评分 |
| **4** | Proxy | proxy-wasm ABI | `proxy_on_request_headers/body`, `proxy_on_response_headers/body` | 协议翻译 |
| **5** | Analyze | Prism Analyzer ABI | `analyze` | 请求特征提取 |

```yaml
plugins:
  - name: my-plugin
    source:
      wasm: ./plugins/my-plugin.wasm
    phases: [proxy, route, analyze]  # 同时参与多个阶段
    enabled: true
    priority: 100
```

</details>

<details>
<summary><b>配置源插件化</b> — ConfigSource trait + 多源合并</summary>

prism-router 通过 `ConfigSource` trait 支持可插拔的配置源：

| 配置源 | 说明 |
|---|---|
| `FileSource` | 从本地 YAML/JSON/TOML 文件加载 |
| `EnvSource` | 从环境变量覆盖（`PRISM_` 前缀） |
| `WasmConfigSource` | 通过 WASM 模块从任意来源加载配置（feature-gated） |
| `CompositeSource` | 按优先级合并多个配置源 |

**优先级顺序**（高覆盖低）：`Base(0) < File(10) < Remote(20) < Environment(30) < Override(40)`

</details>

<details>
<summary><b>插件加载器</b> — 多来源插件解析</summary>

`PluginLoader` 将插件源描述解析为 WASM 字节码，支持缓存和完整性校验：

| 来源 | 配置格式 | 说明 |
|---|---|---|
| **File** | `{ wasm: "./path.wasm" }` | 本地文件系统 |
| **URL** | `{ url: "https://...", sha256: "..." }` | HTTP/HTTPS 远程下载 + 可选 SHA-256 校验 |
| **OCI** | `{ oci: "registry/name:tag" }` | OCI 容器镜像仓库 |
| **Builtin** | `{ builtin: "name" }` | 内置 Rust 插件（无 WASM 字节码） |

下载的插件缓存在本地（`~/.prism/plugins/cache/`），基于 SHA-256 哈希去重。

</details>

<details>
<summary><b>插件管理器</b> — 运行时生命周期管理</summary>

`PluginManager` 提供运行时插件生命周期操作：

- **Load**：解析来源 → 编译 WASM → 注册到对应阶段
- **Unload**：从所有阶段移除 → 释放资源
- **Reload**：原子替换（加载新版本 → 交换 → 卸载旧版本）
- **List**：查询已加载插件的状态和元信息

事件回调（`Loaded`、`Unloaded`、`Reloaded`、`LoadFailed`）用于监控和日志。

</details>

<details>
<summary><b>限流系统</b> — 可配置的 Header 维度限流</summary>

prism-router 内置灵活的限流系统，支持基于 Header 的 key 提取：

- **Header-based key**：组合任意请求 Header（如 `x-model`、`x-app-id`）作为限流 key
- **滑动窗口**：按时间窗口的请求计数和 token 计数限流
- **差异化 override**：同一规则下不同 Header 值组合可设置不同限额
- **最具体匹配优先**：匹配条件最多的 override 生效

```yaml
rate_limits:
  - name: per-model-limit
    key_headers: ["x-model"]
    window_secs: 60
    max_requests: 100
    overrides:
      - match_headers: { "x-model": "gpt-4o" }
        max_requests: 30
      - match_headers: { "x-model": "gpt-4o", "x-team": "vip" }
        max_requests: 80  # 更具体 → 优先
```

</details>

<details>
<summary><b>模型名映射</b> — upstream_model_name 改写</summary>

每个 `ModelVariant` 可指定 `upstream_model_name`，在转发请求前自动改写请求体中的 `model` 字段。适用于：

- Provider 的 API 期望不同的模型名（如 `deepseek-chat` vs 内部的 `deepseek-v3`）
- 自部署推理引擎使用路径式模型名（如 `/data/models/qwen2.5-72b-instruct`）

```yaml
variants:
  - id: "deepseek-v3@deepseek"
    model_id: deepseek-v3
    provider_id: deepseek
    upstream_model_name: "deepseek-chat"
```

代理会在转发前自动将 JSON body 中的 `"model"` 字段改写为该值。

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
│   │   ├── plugins/             # 18 个内置插件（过滤 + 评分）
│   │   ├── discovery/           # 插件化候选发现层
│   │   │   ├── mod.rs               # Discovery trait 定义
│   │   │   ├── static_discovery.rs  # 静态配置发现（默认）
│   │   │   └── wasm_discovery.rs    # WASM 发现插件（feature-gated）
│   │   ├── data.rs              # DataLayer — 指标存储抽象
│   │   ├── feedback.rs          # 反馈收集器（结果 → 权重调整）
│   │   └── ffi.rs               # C FFI 接口（用于嵌入）
│   └── Cargo.toml
├── prism-router/            # 🚀 独立路由服务
│   ├── src/
│   │   ├── main.rs              # 入口：HTTP + gRPC + Proxy 服务
│   │   ├── api.rs               # Axum HTTP 处理器（route、outcomes、health、metrics）
│   │   ├── grpc.rs              # tonic gRPC RouterService 实现
│   │   ├── proxy.rs             # Pingora 反向代理 + upstream_model_name 改写
│   │   ├── protocol.rs          # 协议检测（completions、embeddings 等）
│   │   ├── metrics.rs           # MetricsCollector — 6 维度 Prometheus 指标
│   │   ├── config/              # 配置源插件化
│   │   │   ├── mod.rs               # ConfigSource trait、CompositeSource
│   │   │   ├── types.rs             # PrismConfig 完整类型定义
│   │   │   ├── file_source.rs       # FileSource — YAML/JSON/TOML 文件加载
│   │   │   ├── env_source.rs        # EnvSource — 环境变量加载
│   │   │   ├── wasm_source.rs       # WasmConfigSource（feature-gated）
│   │   │   └── registry.rs          # build_registry() — PrismConfig → ModelRegistry
│   │   ├── state.rs             # AppState（共享 Router + Metrics）
│   │   ├── ratelimit.rs         # 限流系统（滑动窗口 + header-based key）
│   │   ├── unified_plugin.rs    # 统一插件注册框架（PluginRegistry + PluginDescriptor）
│   │   ├── plugin_loader.rs     # 插件加载器（File/URL/OCI/Builtin + 缓存 + SHA-256）
│   │   ├── plugin_manager.rs    # 插件管理器（load/unload/reload + 事件回调）
│   │   └── codec/               # Proxy 插件链（协议翻译）
│   │       ├── mod.rs               # ProxyPlugin trait + ProxyContext
│   │       ├── chain.rs             # ProxyPluginChain
│   │       ├── registry.rs          # CodecRegistry
│   │       └── wasm_host.rs         # wasmtime 宿主层（feature-gated）
│   └── Cargo.toml
├── examples/
│   └── wasm-plugins/
│       └── openai-compat/       # 示例 WASM 插件 — OpenAI 兼容协议转换
├── proto/                   # 📋 Protobuf 定义
│   └── prism/router/v1/
│       └── router.proto
├── configs/                 # ⚙️ 配置文件
│   ├── prism.example.yaml
│   ├── prism.yaml
│   └── prometheus.yml
├── Makefile                 # 构建与测试命令
├── Dockerfile               # 多阶段构建
├── docker-compose.yml       # Prism + Redis + PostgreSQL + Prometheus
└── DESIGN.md                # 完整技术设计文档
```

## 开发

```bash
cargo build --release        # 构建
cargo test                   # 运行测试（803 个通过）
cargo bench                  # 性能基准测试（prism-core + prism-router）
cargo fmt --all              # 格式化代码
cargo clippy -- -D warnings  # 代码检查
```

### Feature Flags

| Flag | 默认 | 说明 |
|---|---|---|
| `grpc` | ✅ | 启用 gRPC RouterService (tonic) |
| `proxy` | ✅ | 启用 Pingora 反向代理 |
| `wasm` | ❌ | 启用 WASM proxy 插件支持 (wasmtime)，prism-router |

## 性能

### 路由决策延迟

| Variants 数量 | 路由延迟 | 完整 Proxy Pipeline | 说明 |
|------------|---------|-------------------:|------|
| **100** | 90 µs | 100 µs | 典型小规模部署 |
| **1,000** | 900 µs | 980 µs | 中等规模（32p × 32m） |
| **10,000** | 11 ms | 11 ms | 大型多 Provider 集群 |
| **100,000** | 146 ms | 132 ms | 极端规模（317p × 317m） |

> 完整 Proxy Pipeline = 限流检查 + 路由 + Codec Body 变换 + 指标记录。
> Pipeline 额外开销仅 **~10µs**，所有规模下路由本身是主要耗时。

### 组件性能

| 组件 | 指标 | 延迟 |
|------|------|-----:|
| 限流 | 全局规则检查 | 100 ns |
| 限流 | Per-model 维度 | 150 ns |
| 限流 | 5 条规则串行 | 500 ns |
| 限流 | Override 匹配（5 个） | 300 ns |
| Codec 链 | 请求体变换（2KB） | 2 µs |
| Codec 链 | SSE 流式 chunk（1 插件） | 100 ns |
| Codec 注册表 | 协议对查找 | 78 ns |
| 配置热更新 | **arc-swap load（每请求）** | **9.8 ns** |
| 配置热更新 | arc-swap store（重载） | 22 µs |
| 配置热更新 | Registry 重建（400 variants） | 295 µs |
| 指标 | 单 variant 记录 | 80 ns |
| 指标 | 6 维全标签记录 | 1.7 µs |
| 指标 | Prometheus 格式输出 | 28 µs |

### 运行 Benchmark

```bash
# 核心路由 benchmark
cargo bench --bench routing

# Proxy 层 benchmark（限流、codec、热更新、指标、端到端）
cargo bench --bench proxy

# 仅端到端
cargo bench --bench proxy -- "e2e/"
```

### 测试覆盖

| 测试套件 | 数量 |
|---------|-----:|
| 默认 (`cargo test`) | 803 |
| 含 WASM (`--features wasm`) | 945 |
| E2E 集成测试 | 24 |
| **总计** | **~970** |

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
