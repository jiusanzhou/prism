# Prism 棱镜

> 一束请求进来，根据策略折射到最优模型。

<p align="center">
  <strong>高性能 LLM 智能路由引擎</strong><br>
  Rust 核心 · Go 网关 · OpenAI 兼容 · 亚毫秒级决策
</p>

---

## 为什么需要 Prism？

当你同时使用 GPT-4o、Claude、DeepSeek 等多个模型时，面临的问题：

- 简单问题用 GPT-4o 太贵，复杂问题用 mini 效果差
- 某个 Provider 限流了，手动切换麻烦
- 不同团队的请求需要走不同的计费账号
- 想做 A/B 测试对比模型效果，但没有基础设施

**Prism 解决这些问题**：根据请求内容自动分析复杂度，结合你的策略偏好（省钱？低延迟？最高质量？），在亚毫秒内选出最优的模型和 Provider。

## 特性

- 🧠 **智能路由** — 分析请求内容（代码/推理/翻译/闲聊），自动匹配最合适的模型
- ⚡ **极致性能** — Rust 核心引擎，路由决策 8μs，P99 < 5ms
- 🎯 **多策略** — Balanced / SLO / CostFirst / QualityFirst，请求级可切换
- 🔌 **即插即用** — OpenAI 兼容 API，一行改 base_url 就能接入
- 📊 **可观测** — Prometheus metrics、请求追踪、成本分析
- 🔄 **反馈闭环** — 根据实际效果动态调整路由权重
- 🧪 **A/B 测试** — 配置化实验，对比模型效果
- 🏗️ **多部署模式** — FFI 内嵌（< 0.5ms）/ 独立 HTTP 服务 / Envoy ext_proc
- 🔧 **可扩展分析器** — 内置规则 / YAML 配置 / Webhook / WASM 插件

## 架构

```
                                    ┌──────────────────────────┐
                                    │   prism-core (Rust)      │
Client ──→ prism-gateway (Go) ──FFI──→  Request Analyzer       │
              │                     │  Strategy Engine (L1-L5) │
              │                     │  Feedback Collector      │
              │                     └──────────────────────────┘
              │
              ├──→ OpenAI
              ├──→ Anthropic
              ├──→ DeepSeek
              └──→ Any OpenAI-compatible
```

**五层调度引擎：**

| 层 | 职责 | 示例 |
|---|---|---|
| L1 | 模型选择 | "这是代码问题 → 选 GPT-4o" |
| L2 | Provider 选择 | "用户在 us-east → 选最近的 endpoint" |
| L3 | Account 选择 | "team-a 的请求 → 走 team-a 的计费账号" |
| L4 | Variant 选择 | "10% 流量给 canary 版本" |
| L5 | 容错 | "Provider 超时 → 自动重试/降级" |

## 快速开始

### 1. 构建

```bash
# 需要 Rust 1.82+ 和 Go 1.22+
make build
```

### 2. 配置

```bash
cp configs/prism.example.yaml configs/prism.yaml
# 编辑 prism.yaml，填入 API key
```

```yaml
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

### 3. 运行

```bash
# 直接运行
./bin/prism --config configs/prism.yaml

# 或 Docker
docker compose up -d
```

### 4. 使用

```bash
# 和 OpenAI API 完全一样，只需改 base_url
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "auto",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## 路由策略

通过 header 或 body 指定策略，每个请求可以不同：

```bash
# 最低成本 — 简单问题自动选便宜模型
curl -H "X-Prism-Strategy: cost_first" ...

# SLO 优先 — 延迟必须在 500ms 内
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

### Session 亲和性

同一会话路由到同一 Provider，提高 KV Cache 命中率：

```bash
curl -H "X-Prism-Session-Id: session-abc123" ...
```

### 租户成本隔离

不同团队的请求走不同计费账号：

```bash
curl -H "X-Prism-Tenant-Id: team-backend" ...
```

### 来源平台差异化规则

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
curl -H "X-Prism-Source: mobile" \
     -H "X-Prism-Caller-Type: user" ...
```

### A/B 测试

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

### 结果反馈

```bash
# 上报请求结果，用于动态调整路由权重
curl -X POST http://localhost:8080/v1/outcomes \
  -d '{
    "variant_id": "gpt-4o@openai",
    "success": true,
    "total_latency_ms": 1200,
    "prompt_tokens": 50,
    "completion_tokens": 200
  }'
```

## 部署模式

| 模式 | 路由延迟 | 适用场景 |
|---|---|---|
| **A: FFI 内嵌** | < 0.5ms | 性能最优，完整自建方案 |
| **B: 独立服务** | 1-3ms | 集成现有网关（Higress/Envoy/APISIX） |
| **C: Envoy ext_proc** | 1-3ms | 深度 Envoy 生态集成 |

```bash
# 模式 A（默认）— FFI 内嵌
./bin/prism --config configs/prism.yaml

# 模式 B — 独立路由服务
./bin/prism-router --config configs/prism.yaml --listen :8081
```

## 可观测性

```bash
# Prometheus metrics
curl http://localhost:8080/metrics

# 关键指标
# prism_routing_decision_duration_seconds  — 路由决策延迟
# prism_provider_request_duration_seconds  — Provider 请求延迟
# prism_provider_requests_total            — 请求计数
# prism_tokens_total                       — Token 用量
# prism_active_requests                    — 活跃请求数
# prism_ttft_seconds                       — 首 Token 延迟
```

## 管理后台

```bash
# 启动 admin 服务（独立端口）
./bin/prism-admin --config configs/prism.yaml --listen :8081

# API
GET /api/v1/status              # 系统状态
GET /api/v1/providers           # Provider 列表
GET /api/v1/models              # 模型列表
GET /api/v1/variants            # Variant 列表 + 实时性能
GET /api/v1/dashboard/overview  # 仪表盘概览
GET /api/v1/experiments         # 实验列表
```

## 项目结构

```
prism/
├── prism-core/          # Rust 路由引擎核心
│   ├── src/
│   │   ├── analyzer.rs      # 请求分析器（规则/配置/Webhook/WASM）
│   │   ├── strategy.rs      # L1-L5 调度策略
│   │   ├── router.rs        # PrismRouter 主逻辑
│   │   ├── registry.rs      # 模型注册表
│   │   ├── feedback.rs      # 反馈收集器
│   │   ├── ffi.rs           # C FFI 接口
│   │   └── types.rs         # 核心类型定义
│   └── tests/               # 76 个测试
├── prism-gateway/       # Go API 网关
│   ├── cmd/prism/           # 入口
│   ├── internal/
│   │   ├── gateway/         # HTTP handler
│   │   ├── provider/        # OpenAI/Anthropic/DeepSeek
│   │   ├── router/          # FFI 桥接 + 策略映射
│   │   ├── metrics/         # Prometheus 指标
│   │   ├── state/           # Redis/内存状态存储
│   │   ├── abtesting/       # A/B 测试框架
│   │   └── config/          # 配置加载
│   ├── pkg/api/             # OpenAI 兼容类型
│   └── tests/e2e/           # E2E 测试
├── prism-router/        # 独立路由服务（模式 B）
├── prism-admin/         # 管理后台
├── proto/               # Protobuf 定义
├── configs/             # 配置文件 + Prometheus
├── Dockerfile           # 多阶段构建（Rust + Go）
├── Dockerfile.simple    # 轻量构建（无 FFI）
├── docker-compose.yml   # Prism + Redis + Prometheus
├── Makefile
└── DESIGN.md            # 完整技术设计文档
```

## 开发

```bash
# 全量测试
make test-all

# 各模块测试
make test-core       # Rust 76 tests
make test-e2e        # Go E2E tests
make test-ffi        # Go FFI tests

# 格式化 & 检查
make fmt
make lint

# Docker
make docker-up
make docker-down
```

## 性能

| 指标 | 实测 | 目标 |
|---|---|---|
| 路由决策延迟 | **8μs** | < 500μs |
| FFI 调用开销 | < 0.01ms | < 0.5ms |
| 网关额外延迟 | < 5ms | < 10ms |
| 1000 次路由 | < 50ms | - |

## 技术栈

| 组件 | 语言 | 说明 |
|---|---|---|
| prism-core | Rust | 路由引擎、请求分析、策略评分 |
| prism-gateway | Go | API 网关、Provider 对接、流式转发 |
| prism-router | Go + Rust FFI | 独立路由服务 |
| prism-admin | Go | 管理后台 API |
| 状态存储 | Redis（可选） | 实时 QPS/延迟/错误率 |
| 监控 | Prometheus | 指标采集 |

## 文档

- [完整技术设计文档](./DESIGN.md) — 架构、策略引擎、对象模型、部署模式
- [配置示例](./configs/prism.example.yaml)
- [Protobuf 定义](./proto/prism/router/v1/router.proto)

## License

MIT
