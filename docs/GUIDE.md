# Prism 使用指南

> 从安装部署到生产运维的完整文档。

---

## 目录

1. [概述](#概述)
2. [安装部署](#安装部署)
   - [源码编译](#源码编译)
   - [Docker 部署](#docker-部署)
   - [Kubernetes (Helm)](#kubernetes-helm)
3. [快速开始](#快速开始)
4. [配置详解](#配置详解)
   - [Server 配置](#server-配置)
   - [Provider (供应商)](#provider-供应商)
   - [Account (账户)](#account-账户)
   - [Model (模型)](#model-模型)
   - [Variant (部署变体)](#variant-部署变体)
   - [Pool (资源池)](#pool-资源池)
   - [Routing (路由策略)](#routing-路由策略)
   - [Model Alias (模型别名)](#model-alias-模型别名)
   - [Client Keys (多租户配额)](#client-keys-多租户配额)
   - [Rate Limiting (限流)](#rate-limiting-限流)
   - [Auth (入口鉴权)](#auth-入口鉴权)
   - [Access Log (访问日志)](#access-log-访问日志)
   - [Codec (协议转换)](#codec-协议转换)
   - [Plugin (插件)](#plugin-插件)
   - [Admin (管理后台)](#admin-管理后台)
   - [Usage Tracking (用量追踪)](#usage-tracking-用量追踪)
   - [TLS 终止](#tls-终止)
5. [新增供应商和模型](#新增供应商和模型)
   - [标准 LLM 供应商](#标准-llm-供应商)
   - [非标准供应商 (自定义端点)](#非标准供应商-自定义端点)
   - [自建推理集群 (K8s)](#自建推理集群-k8s)
   - [多账户管理](#多账户管理)
6. [API 参考](#api-参考)
   - [Proxy (OpenAI 兼容)](#proxy-openai-兼容)
   - [Routing API](#routing-api)
   - [Admin API](#admin-api)
7. [路由策略详解](#路由策略详解)
8. [可观测性](#可观测性)
9. [高级功能](#高级功能)
   - [P/D 分离调度](#pd-分离调度)
   - [WebSocket 代理](#websocket-代理)
   - [异步任务管理](#异步任务管理)
   - [WASM 插件](#wasm-插件)
   - [配置热更新](#配置热更新)
10. [环境变量](#环境变量)
11. [生产部署建议](#生产部署建议)
12. [故障排查](#故障排查)

---

## 概述

Prism 是一个高性能 AI 模型路由器，核心职责：

- **智能路由**：根据请求内容自动选择最优模型 + 供应商
- **反向代理**：OpenAI 兼容 API，客户端只需改 `base_url`
- **多模态**：支持 Chat / Embedding / ASR / TTS / 图像 / 视频等 14 种 API 协议
- **多租户**：per-key 配额、模型 ACL、用量追踪
- **可观测**：Prometheus 指标、Access Log、Request Tracing

**三个服务端口：**

| 端口 | 用途 | 说明 |
|------|------|------|
| `8080` | HTTP API + Admin | 路由决策、健康检查、管理后台、Prometheus metrics |
| `8000` | Proxy | OpenAI 兼容反向代理（客户端连这个） |
| `9090` | gRPC | 高性能路由服务（网关集成） |

---

## 安装部署

### 源码编译

**要求：** Rust 1.82+, protoc (gRPC 编译需要)

```bash
# 克隆
git clone https://github.com/jiusanzhou/prism.git
cd prism

# 完整构建（HTTP + gRPC + Proxy）
cargo build --release -p prism-router

# 仅 HTTP（不含 gRPC 和 Proxy）
cargo build --release -p prism-router --no-default-features

# 含 WASM 插件支持
cargo build --release -p prism-router --features wasm

# 含 SQLite 用量追踪
cargo build --release -p prism-router --features usage-db

# 含 PostgreSQL 用量追踪
cargo build --release -p prism-router --features usage-pg
```

**Feature Flags：**

| Flag | 默认 | 说明 |
|------|------|------|
| `grpc` | ✅ | gRPC 路由服务 (tonic) |
| `proxy` | ✅ | Pingora 反向代理 |
| `wasm` | ❌ | WASM 插件支持 (wasmtime) |
| `redis` | ❌ | 分布式限流 (Redis 后端) |
| `usage-db` | ❌ | SQLite 用量持久化 |
| `usage-pg` | ❌ | PostgreSQL 用量持久化 |

### Docker 部署

```bash
# 构建镜像
docker compose build

# 启动（Prism + Prometheus）
docker compose up -d

# 或使用预构建镜像
docker pull ghcr.io/jiusanzhou/prism:0.1.0
docker run -d \
  -p 8080:8080 -p 8000:8000 -p 9090:9090 \
  -v ./configs:/etc/prism \
  -e OPENAI_API_KEY=sk-xxx \
  ghcr.io/jiusanzhou/prism:0.1.0
```

**docker-compose.yml:**

```yaml
services:
  prism:
    build: .
    ports:
      - "8080:8080"   # HTTP API + Admin
      - "8000:8000"   # Proxy
      - "9090:9090"   # gRPC
    volumes:
      - ./configs:/etc/prism
    environment:
      - OPENAI_API_KEY=${OPENAI_API_KEY}
      - ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY}
    restart: unless-stopped

  prometheus:
    image: prom/prometheus:latest
    ports:
      - "9091:9090"
    volumes:
      - ./configs/prometheus.yml:/etc/prometheus/prometheus.yml
```

### Kubernetes (Helm)

```bash
# 安装
helm install prism deploy/helm/prism \
  --set config.providers.openai.base_url=https://api.openai.com/v1 \
  --set secrets.OPENAI_API_KEY=sk-xxx

# 或使用自定义 values
helm install prism deploy/helm/prism -f my-values.yaml

# 升级
helm upgrade prism deploy/helm/prism -f my-values.yaml

# 查看状态
helm status prism
```

**values.yaml 要点：**

```yaml
replicaCount: 2

image:
  repository: ghcr.io/jiusanzhou/prism
  tag: "0.1.0"

service:
  type: ClusterIP
  proxyPort: 8000
  httpPort: 8080
  grpcPort: 9090

autoscaling:
  enabled: true
  minReplicas: 2
  maxReplicas: 10
  targetCPUUtilizationPercentage: 70

resources:
  requests:
    cpu: 250m
    memory: 256Mi
  limits:
    cpu: "1"
    memory: 512Mi

# Prism 配置直接嵌入 values
config:
  server:
    proxy_port: 8000
  providers:
    openai:
      base_url: "https://api.openai.com/v1"
      protocols: ["openai"]
  # ... (完整配置见下文)
```

---

## 快速开始

**1. 准备配置文件**

```bash
cp examples/prism.example.yaml configs/prism.yaml
```

**2. 最小配置（一个供应商 + 一个模型）：**

```yaml
server:
  proxy_port: 8000
  http_port: 8080

providers:
  openai:
    base_url: "https://api.openai.com/v1"
    protocols: ["openai"]

accounts:
  - id: openai-default
    provider_id: openai
    api_key: "sk-your-key-here"

models:
  - id: gpt-4o
    display_name: "GPT-4o"
    quality_tier: S

variants:
  - id: gpt-4o@openai
    model_id: gpt-4o
    provider_id: openai

pools:
  - id: default
    name: "Default Pool"
    variants:
      - gpt-4o@openai
```

**3. 启动**

```bash
./target/release/prism-router --config configs/prism.yaml

# 或带调试日志
RUST_LOG=prism_router=debug,prism_core=debug \
  ./target/release/prism-router --config configs/prism.yaml
```

**4. 测试**

```bash
# 通过 Proxy 端口直接请求（OpenAI 兼容）
curl http://localhost:8000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'

# 查看健康状态
curl http://localhost:8080/health

# 查看 Prometheus 指标
curl http://localhost:8080/metrics
```

---

## 配置详解

配置文件使用 YAML 格式，所有字符串字段支持 `${ENV_VAR}` 环境变量替换。

### Server 配置

```yaml
server:
  listen: "0.0.0.0"              # 监听地址
  http_port: 8080                # HTTP API + Admin 端口
  grpc_port: 9090                # gRPC 端口
  proxy_port: 8000               # Proxy 端口（客户端连这个）
  request_timeout_secs: 300      # 上游请求超时（默认 300s，适合 LLM 长推理）
  idle_timeout_secs: 90          # SSE 流式空闲超时
  keepalive_secs: 60             # TCP keepalive
  shutdown_timeout_secs: 30      # 优雅关闭超时
  max_request_body_bytes: 10485760  # 请求体最大大小（默认 10MB）
  cors_origins: ["*"]            # CORS 允许的源
  retry_on_5xx: true             # 上游 5xx 是否自动重试
  retry_5xx_codes: [502, 503, 504]  # 哪些 5xx 触发重试
  pd_prefill_timeout_secs: 30    # P/D 分离 prefill 超时
  tls:                           # TLS 终止（可选）
    cert_path: /path/to/cert.pem
    key_path: /path/to/key.pem
```

### Provider (供应商)

Provider 是上游 API 服务的抽象。每个 provider 有一个唯一 ID（YAML 的 key）。

```yaml
providers:
  # ── 标准 OpenAI ──
  openai:
    base_url: "https://api.openai.com/v1"
    protocols: ["openai"]
    request_timeout_secs: 120        # 覆盖全局超时
    idle_timeout_secs: 30            # SSE 空闲超时
    circuit_breaker_threshold: 5     # 连续失败 N 次后熔断
    circuit_breaker_recovery_secs: 30  # 熔断恢复等待

  # ── Anthropic ──
  anthropic:
    base_url: "https://api.anthropic.com/v1"
    protocols: ["anthropic"]
    auth_type: x_api_key             # Anthropic 用 x-api-key header
    request_timeout_secs: 300

  # ── Azure OpenAI ──
  azure-openai:
    base_url: "https://my-resource.openai.azure.com/openai/deployments"
    protocols: ["openai"]
    auth_type: api_key               # Azure 用 api-key header
    headers:
      api-version: "2024-02-01"      # 自定义请求头

  # ── 自定义 header 鉴权 ──
  custom-provider:
    base_url: "https://api.example.com"
    protocols: ["openai"]
    auth_type:
      custom: "X-My-Auth-Token"      # 自定义鉴权 header 名

  # ── AWS Bedrock ──
  bedrock:
    base_url: "https://bedrock-runtime.us-east-1.amazonaws.com"
    auth_type: aws_sig_v4
    aws_region: "us-east-1"
    aws_access_key: "${AWS_ACCESS_KEY}"
    aws_secret_key: "${AWS_SECRET_KEY}"
```

**auth_type 选项：**

| 值 | Header | 适用 |
|---|---|---|
| `bearer` (默认) | `Authorization: Bearer <key>` | OpenAI, DeepSeek 等 |
| `header` | `Authorization: <key>` | 无 Bearer 前缀 |
| `x_api_key` | `x-api-key: <key>` | Anthropic, ElevenLabs 等 |
| `api_key` | `api-key: <key>` | Azure OpenAI |
| `custom("X-Name")` | `X-Name: <key>` | 自定义 header |
| `aws_sig_v4` | AWS Signature V4 | Bedrock |

**Endpoint 声明（多模态供应商）：**

当供应商支持非标准 API 端点时，显式声明 endpoints：

```yaml
providers:
  # ── ElevenLabs TTS ──
  elevenlabs:
    base_url: "https://api.elevenlabs.io/v1"
    auth_type: x_api_key
    endpoints:
      - type: audio_speech
        path: "/text-to-speech/{voice_id}"     # 自定义路径（支持模板变量）
        response_format: binary                 # 响应是二进制音频
      - type: audio_speech
        path: "/text-to-speech/{voice_id}/stream"
        response_format: binary
        streaming: true                         # 流式变体

  # ── Stability AI ──
  stability:
    base_url: "https://api.stability.ai"
    auth_type: bearer
    endpoints:
      - type: image_generation
        path: "/v2beta/stable-image/generate/core"
        content_type: multipart                 # 请求是 multipart
        response_format: binary                 # 响应是二进制图片

  # ── Google Gemini ──
  google:
    base_url: "https://generativelanguage.googleapis.com"
    endpoints:
      - type: chat_completion
        path: "/v1beta/models/{model}:generateContent"
      - type: embedding
        path: "/v1beta/models/{model}:embedContent"
```

**EndpointConfig 字段：**

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `type` | string | 必填 | 端点类型：`chat_completion`, `completion`, `embedding`, `rerank`, `audio_transcription`, `audio_translation`, `audio_speech`, `image_generation`, `image_edit`, `image_variation`, `video_generation`, `moderation` |
| `path` | string | 无 | 自定义上游路径，支持 `{model}` 和 `{voice_id}` 模板变量 |
| `content_type` | string | `"json"` | 请求格式：`json` / `multipart` / `binary` |
| `response_format` | string | `"json"` | 响应格式：`json` / `binary` / `sse` |
| `streaming` | bool | 无 | 是否支持流式（默认从 type 推断） |
| `headers` | map | 空 | 该端点的额外请求头 |

> **向后兼容**：如果不配 `endpoints`，Prism 从 `protocols` 字段自动推断。`protocols: ["openai"]` 自动支持所有 OpenAI 标准端点。

### Account (账户)

Account 是一个 API Key + Provider 的绑定。同一个 provider 可以有多个 account（多账户轮转、团队隔离）。

```yaml
accounts:
  - id: openai-main
    provider_id: openai
    api_key: "${OPENAI_API_KEY}"
    enabled: true
    weight: 1.0                      # 账户选择权重
    billing_tag: "team-backend"      # 计费标签（租户路由用）
    rate_limit:                      # 账户级别限流
      rpm: 500
      tpm: 200000
      concurrent: 50
    metadata:
      team: "infra"

  - id: openai-backup
    provider_id: openai
    api_key: "${OPENAI_BACKUP_KEY}"
    weight: 0.5                      # 低权重，backup 用途

  - id: anthropic-main
    provider_id: anthropic
    api_key: "${ANTHROPIC_API_KEY}"
```

### Model (模型)

Model 是逻辑模型定义，描述模型能力和成本，不绑定任何供应商。

```yaml
models:
  - id: gpt-4o
    display_name: "GPT-4o"
    quality_tier: S                  # S(最高) > A > B > C > D
    capabilities:
      max_context_window: 128000
      supports_streaming: true
      supports_function_calling: true
      supports_vision: true
    pricing:
      input_per_1m_tokens: 2.5      # 每百万 input token 美元
      output_per_1m_tokens: 10.0
    specialties:                     # 模型擅长的任务类型
      - Reasoning
      - CodeGeneration

  - id: gpt-4o-mini
    display_name: "GPT-4o Mini"
    quality_tier: A
    capabilities:
      max_context_window: 128000
      supports_streaming: true
    pricing:
      input_per_1m_tokens: 0.15
      output_per_1m_tokens: 0.60
    specialties:
      - SimpleQA
      - Translation

  - id: whisper-1
    display_name: "Whisper"
    quality_tier: A
    capabilities:
      supports_streaming: false
```

**quality_tier：** `S` / `A` / `B` / `C` / `D`，用于路由策略中 quality_weight 的排序。

**specialties 可选值：** `Reasoning`, `CodeGeneration`, `SimpleQA`, `Translation`, `Summarization`, `CreativeWriting`, `DataAnalysis`, `MathProof`

### Variant (部署变体)

Variant 是 Model + Provider 的具体部署实例。同一个模型可以有多个 variant（不同供应商、不同区域、不同版本）。

```yaml
variants:
  # ── 基本格式：model@provider ──
  - id: gpt-4o@openai
    model_id: gpt-4o
    provider_id: openai

  # ── 绑定账户 ──
  - id: gpt-4o@openai-backup
    model_id: gpt-4o
    provider_id: openai
    account_id: openai-backup        # 使用 backup 账户

  # ── 自建模型（覆盖 base_url）──
  - id: qwen-72b@selfhost
    model_id: qwen3-72b
    provider_id: selfhost
    base_url: "http://gpu-node-1:8080"  # 覆盖 provider 的 base_url
    upstream_model_name: "/data/models/qwen2.5-72b-instruct"  # 上游实际模型名

  # ── Azure 部署 ──
  - id: gpt-4o@azure
    model_id: gpt-4o
    provider_id: azure-openai
    deployment_name: "gpt-4o-2024-05-13"

  # ── 带权重、限流、元数据 ──
  - id: deepseek-v3@deepseek
    model_id: deepseek-v3
    provider_id: deepseek
    weight: 1.5                       # 路由权重（越高越优先）
    upstream_model_name: "deepseek-chat"
    status: active                    # active / inactive
    rate_limit:
      rpm: 100
      tpm: 50000
      concurrent: 20
    pricing_override:                 # 覆盖 model 级别定价
      input_per_1m_tokens: 0.27
      output_per_1m_tokens: 1.1
    metadata:
      region: "cn-east"
      pd_role: "decode"              # P/D 分离角色

  # ── 多模态 variant（声明 capabilities）──
  - id: whisper@openai
    model_id: whisper-1
    provider_id: openai
    capabilities: ["audio_transcription"]

  - id: tts-1@openai
    model_id: tts-1
    provider_id: openai
    capabilities: ["audio_speech"]

  - id: dall-e-3@openai
    model_id: dall-e-3
    provider_id: openai
    capabilities: ["image_generation"]
```

**upstream_model_name：** 当上游 API 期望的模型名与 Prism 内部名不同时，Proxy 自动改写请求 body 中的 `"model"` 字段。

### Pool (资源池)

Pool 是 variant 的分组，路由策略在 pool 范围内选择。

```yaml
pools:
  - id: default
    name: "Default Pool"
    variants:
      - gpt-4o@openai
      - gpt-4o-mini@openai
      - deepseek-v3@deepseek

  - id: premium
    name: "Premium Pool"
    variants:
      - gpt-4o@openai
      - claude-opus@anthropic

  - id: selfhosted
    name: "Self-hosted Pool"
    variants:
      - qwen-72b@selfhost
```

### Routing (路由策略)

```yaml
routing:
  default_strategy: balanced         # 默认策略
  balanced:                          # balanced 策略的权重
    cost_weight: 0.3
    quality_weight: 0.5
    latency_weight: 0.2
  l2_selector: default               # L2 Provider 选择策略
  l3_selector: affinity              # L3 Account 选择策略
  l4_selector: best                  # L4 Variant 选择策略

  # 按模型覆盖策略
  model_overrides:
    - model_id: gpt-4o
      strategy: quality_first
      pool: premium                  # 特定模型走特定 pool

  # 容错配置
  resilience:
    retry:
      max_retries: 2
      retry_on: [RateLimit, Timeout]
      backoff: Exponential           # Exponential / Fixed
      retry_scope: CrossProvider     # CrossProvider / SameProvider
    circuit_breaker:
      error_threshold_pct: 50.0
      window_seconds: 60
      open_duration_seconds: 30
    timeout:
      connect_ms: 3000
      request_ms: 30000
      ttft_ms: 10000                 # Time to First Token
```

**可用策略：**

| 策略 | 说明 |
|------|------|
| `balanced` | 综合权衡成本、质量、延迟 |
| `cost_first` | 成本优先，选最便宜的 |
| `quality_first` | 质量优先，选 quality_tier 最高的 |
| `slo_first` | SLO 优先，延迟必须满足要求 |

客户端可通过请求头覆盖策略：
```bash
curl -H "X-Prism-Strategy: cost_first" http://localhost:8000/v1/chat/completions ...
```

### Model Alias (模型别名)

客户端发送的模型名 → Prism 内部模型 ID 映射。

```yaml
model_aliases:
  gpt-4: qwen3-72b                # gpt-4 映射到 qwen3-72b
  gpt-4o: qwen3-72b
  gpt-4o-mini: qwen3-0.6b
  claude-sonnet: deepseek-r1
  "*": qwen3-0.6b                 # 通配符兜底
```

客户端请求 `"model": "gpt-4"` 时，Prism 内部路由到 `qwen3-72b`。

### Client Keys (多租户配额)

```yaml
client_keys:
  - key: "${TEAM_ALPHA_KEY}"
    name: "Team Alpha"
    enabled: true
    allowed_models: []               # 空 = 所有模型
    quota:
      max_requests_per_minute: 200   # RPM
      max_tokens_per_minute: 500000  # TPM
      max_tokens_per_day: 5000000    # TPD
      max_concurrent: 20             # 最大并发
    model_quotas:                    # 按模型覆盖
      gpt-4o:
        max_requests_per_minute: 30
        max_tokens_per_minute: 100000
        max_tokens_per_day: 1000000
        max_concurrent: 5
    metadata:
      team: "alpha"
      tier: "enterprise"

  - key: "${TEAM_BETA_KEY}"
    name: "Team Beta"
    allowed_models: ["gpt-4o-mini", "qwen3-0.6b"]  # 模型 ACL
    quota:
      max_requests_per_minute: 60
```

客户端通过 `Authorization: Bearer <key>` 传递 API Key。

### Rate Limiting (限流)

基于 Header 值的灵活限流，支持差异化 override。

```yaml
rate_limits:
  # ── 全局限流 ──
  - name: global
    key_headers: []                  # 空 = 所有请求共享一个计数器
    window_secs: 60
    max_requests: 1000
    enabled: true

  # ── 按模型限流 ──
  - name: per-model
    key_headers: ["x-model"]         # 按 x-model header 分组
    window_secs: 60
    max_requests: 100
    max_tokens: 200000
    overrides:                       # 差异化覆盖
      - match_headers: { "x-model": "gpt-4o" }
        max_requests: 30             # gpt-4o 单独限制
      - match_headers: { "x-model": "gpt-4o", "x-team": "vip" }
        max_requests: 80             # VIP 团队的 gpt-4o 额度更高（最具体匹配优先）
```

### Auth (入口鉴权)

```yaml
auth:
  enabled: true
  header: "authorization"            # 检查的 header（默认）
  strip_prefix: "Bearer "           # 去掉前缀后匹配
  keys:                              # 静态 key 列表（简单模式）
    - "${PRISM_API_KEY}"
  skip_paths:                        # 跳过鉴权的路径
    - /health
    - /readyz
    - /version
```

> **推荐**：生产环境使用 `client_keys` 而非 `auth.keys`，前者支持配额和 ACL。

### Access Log (访问日志)

Envoy 风格的 `%COMMAND_OPERATOR%` 格式。

```yaml
access_log:
  enabled: true
  output: both                       # stdout / file / both
  file_path: /var/log/prism/access.log
  format: json                       # json / text
  max_file_size_mb: 100              # 日志轮转大小
  max_files: 5                       # 保留的轮转文件数
  log_request_body: false            # 记录请求体
  log_response_body: false           # 记录响应体
  body_log_max_bytes: 4096           # body 最大记录字节数

  # text 格式的自定义模板
  text_format: '[%START_TIME%] "%METHOD% %PATH%" %RESPONSE_CODE% %DURATION_MS%ms variant=%VARIANT% provider=%PROVIDER%'

  # json 格式的字段选择（空 = 所有默认字段）
  fields:
    - "%START_TIME%"
    - "%METHOD%"
    - "%PATH%"
    - "%RESPONSE_CODE%"
    - "%DURATION_MS%"
    - "%VARIANT%"
    - "%PROVIDER%"
    - "%MODEL%"
    - "%REQ_BODY%"
    - "%RESP_BODY%"
```

**常用 Operator：**

| Operator | 说明 |
|----------|------|
| `%START_TIME%` | 请求开始时间 |
| `%METHOD%` | HTTP 方法 |
| `%PATH%` | 请求路径 |
| `%RESPONSE_CODE%` | 响应状态码 |
| `%DURATION_MS%` | 总耗时 (ms) |
| `%VARIANT%` | 选中的 variant |
| `%PROVIDER%` | 选中的 provider |
| `%MODEL%` | 选中的 model |
| `%ROUTE_STRATEGY%` | 使用的路由策略 |
| `%DECISION_TIME_US%` | 路由决策耗时 (µs) |
| `%UPSTREAM_HOST%` | 上游地址 |
| `%INPUT_TOKENS%` | 输入 token 数 |
| `%OUTPUT_TOKENS%` | 输出 token 数 |
| `%REQ_BODY%` | 请求体（需启用） |
| `%RESP_BODY%` | 响应体（需启用） |
| `%ERROR%` | 错误信息 |

### Codec (协议转换)

当客户端使用 OpenAI 协议但上游是 Anthropic 协议时，codec 插件链自动转换。

```yaml
codec:
  default_inbound_protocol: openai   # 默认入站协议

  # 按协议对配置转换链
  protocol_chains:
    "openai:anthropic":              # OpenAI → Anthropic
      - openai-to-anthropic-plugin

  # 按 provider 配置转换链
  provider_chains:
    azure-openai:
      - azure-compat-plugin

  # 全局链 — 所有请求都经过
  global_chain:
    - request-logger
    - token-counter
```

### Plugin (插件)

```yaml
plugins:
  - name: openai-compat
    source:
      wasm: ./plugins/openai-compat.wasm    # 本地 WASM 文件
    phases: [proxy]                          # 参与的阶段
    config:
      normalize_paths: true
    enabled: true
    priority: 100                            # 执行优先级（越小越先）

  - name: k8s-discovery
    source:
      url: https://cdn.example.com/plugins/k8s-disco.wasm  # 远程下载
      sha256: "abc123..."                    # 完整性校验
    phases: [discovery]
    config:
      namespace: "inference"

  - name: custom-scorer
    source:
      builtin: custom-scorer                 # 内置 Rust 插件
    phases: [route]
```

**插件源类型：**

| 类型 | 配置 | 说明 |
|------|------|------|
| `wasm` | `{ wasm: "./path.wasm" }` | 本地 WASM 文件 |
| `url` | `{ url: "https://...", sha256: "..." }` | HTTP 下载（带校验） |
| `oci` | `{ oci: "registry/name:tag" }` | OCI 容器镜像 |
| `builtin` | `{ builtin: "name" }` | 内置 Rust 插件 |

**插件阶段：**

| 阶段 | ABI | 说明 |
|------|-----|------|
| `config` | Prism Config ABI | 配置源插件 |
| `discovery` | Prism Discovery ABI | 候选发现 |
| `route` | Prism Routing ABI | 路由过滤/打分 |
| `proxy` | proxy-wasm ABI | 协议转换/Header 改写 |
| `analyze` | Prism Analyzer ABI | 请求特征提取 |

### Admin (管理后台)

```yaml
admin:
  enabled: true
  auth_token: "${PRISM_ADMIN_TOKEN}"  # Bearer token 保护
```

### Usage Tracking (用量追踪)

```yaml
# SQLite（本地单机）
usage_db_path: "./prism-usage.db"    # 需 --features usage-db

# PostgreSQL（分布式）
usage_pg_url: "postgresql://user:pass@localhost:5432/prism"  # 需 --features usage-pg

# 数据保留
usage_retention_days: 90             # 超过 N 天的记录自动清理
```

### TLS 终止

```yaml
server:
  tls:
    cert_path: /etc/prism/tls/cert.pem
    key_path: /etc/prism/tls/key.pem
```

---

## 新增供应商和模型

### 标准 LLM 供应商

**场景：** 添加 DeepSeek，使用 OpenAI 兼容协议。

```yaml
# 1. 添加 provider
providers:
  deepseek:
    base_url: "https://api.deepseek.com"
    protocols: ["openai"]           # OpenAI 兼容

# 2. 添加 account
accounts:
  - id: deepseek-main
    provider_id: deepseek
    api_key: "${DEEPSEEK_API_KEY}"

# 3. 添加 model
models:
  - id: deepseek-v3
    display_name: "DeepSeek V3"
    quality_tier: A
    pricing:
      input_per_1m_tokens: 0.27
      output_per_1m_tokens: 1.1
    specialties: [Reasoning, CodeGeneration]

  - id: deepseek-r1
    display_name: "DeepSeek R1"
    quality_tier: S
    pricing:
      input_per_1m_tokens: 0.55
      output_per_1m_tokens: 2.19
    specialties: [Reasoning, MathProof]

# 4. 添加 variant
variants:
  - id: deepseek-v3@deepseek
    model_id: deepseek-v3
    provider_id: deepseek
    upstream_model_name: "deepseek-chat"  # API 实际名字

  - id: deepseek-r1@deepseek
    model_id: deepseek-r1
    provider_id: deepseek
    upstream_model_name: "deepseek-reasoner"

# 5. 加入 pool
pools:
  - id: default
    name: "Default Pool"
    variants:
      - gpt-4o@openai
      - deepseek-v3@deepseek
      - deepseek-r1@deepseek
```

### 非标准供应商 (自定义端点)

**场景：** 添加 ElevenLabs TTS + Stability AI 图像生成。

```yaml
providers:
  elevenlabs:
    base_url: "https://api.elevenlabs.io/v1"
    auth_type: x_api_key
    endpoints:
      - type: audio_speech
        path: "/text-to-speech/{voice_id}"
        response_format: binary

  stability:
    base_url: "https://api.stability.ai"
    auth_type: bearer
    endpoints:
      - type: image_generation
        path: "/v2beta/stable-image/generate/core"
        content_type: multipart
        response_format: binary

accounts:
  - id: elevenlabs-main
    provider_id: elevenlabs
    api_key: "${ELEVENLABS_API_KEY}"
  - id: stability-main
    provider_id: stability
    api_key: "${STABILITY_API_KEY}"

models:
  - id: eleven-multilingual-v2
    display_name: "ElevenLabs Multilingual V2"
    quality_tier: A
  - id: stable-diffusion-core
    display_name: "Stable Diffusion Core"
    quality_tier: A

variants:
  - id: tts@elevenlabs
    model_id: eleven-multilingual-v2
    provider_id: elevenlabs
    capabilities: ["audio_speech"]
  - id: sdxl@stability
    model_id: stable-diffusion-core
    provider_id: stability
    capabilities: ["image_generation"]

pools:
  - id: tts-pool
    name: "TTS Pool"
    variants: [tts@elevenlabs]
  - id: image-pool
    name: "Image Pool"
    variants: [sdxl@stability]
```

### 自建推理集群 (K8s)

**场景：** K8s 集群上部署了 vLLM 推理服务，通过 sidecar 自动发现。

```yaml
providers:
  ai-infra:
    type: sidecar
    discovery:
      sidecar_url: "http://k8s-discovery-sidecar:9091"
      cluster_name: ai-infra-prod
      enable_pd_filter: true           # P/D 分离
      poll_interval_secs: 30

# variant 由 sidecar 自动注册，无需手动配置
# 也可手动配置覆盖：
variants:
  - id: qwen-72b@gpu-01
    model_id: qwen3-72b
    provider_id: ai-infra
    base_url: "http://10.0.1.100:8080"  # Pod 直连
    upstream_model_name: "qwen2.5-72b-instruct"
    metadata:
      pd_role: "decode"                 # P/D 角色
      gpu_type: "A100"
```

### 多账户管理

**场景：** 同一个供应商的多个 API Key 轮转。

```yaml
providers:
  openai:
    base_url: "https://api.openai.com/v1"
    protocols: ["openai"]

accounts:
  - id: openai-team-a
    provider_id: openai
    api_key: "${OPENAI_TEAM_A_KEY}"
    billing_tag: "team-a"
    weight: 1.0
    rate_limit:
      rpm: 500

  - id: openai-team-b
    provider_id: openai
    api_key: "${OPENAI_TEAM_B_KEY}"
    billing_tag: "team-b"
    weight: 1.0
    rate_limit:
      rpm: 300

  - id: openai-fallback
    provider_id: openai
    api_key: "${OPENAI_FALLBACK_KEY}"
    weight: 0.1                        # 低权重，仅兜底
    rate_limit:
      rpm: 100

# 租户路由：请求头带 X-Prism-Tenant-Id: team-a → 路由到 openai-team-a
```

---

## API 参考

### Proxy (OpenAI 兼容)

**端口：** 8000（默认）

**完全兼容 OpenAI API，客户端只需改 `base_url`：**

```python
import openai

client = openai.OpenAI(
    base_url="http://prism:8000/v1",   # 指向 Prism
    api_key="your-prism-key"            # client_keys 中配置的 key
)

# 自动路由到最优模型
response = client.chat.completions.create(
    model="auto",                       # 或指定模型名
    messages=[{"role": "user", "content": "Hello!"}]
)
```

**Prism 特有请求头：**

| Header | 说明 | 示例 |
|--------|------|------|
| `X-Prism-Strategy` | 覆盖路由策略 | `balanced`, `cost_first`, `quality_first` |
| `X-Prism-Session-Id` | 会话亲和性（同会话走同 provider） | `session-abc123` |
| `X-Prism-Tenant-Id` | 租户路由 | `team-backend` |
| `X-Prism-Cost-Weight` | 动态 balanced 成本权重 | `0.5` |
| `X-Prism-Quality-Weight` | 动态 balanced 质量权重 | `0.3` |
| `X-Prism-Latency-Weight` | 动态 balanced 延迟权重 | `0.2` |
| `X-Prism-Max-Latency-Ms` | SLO 延迟上限 | `500` |
| `X-Prism-Debug` | 返回完整决策链路 | `true` |

**Prism 响应头：**

| Header | 说明 |
|--------|------|
| `X-Prism-Model` | 选中的模型 |
| `X-Prism-Provider` | 选中的供应商 |
| `X-Prism-Variant` | 选中的 variant |
| `X-Prism-Account` | 使用的账户 |
| `X-Prism-Decision-Time-Us` | 路由决策耗时 (µs) |
| `X-Prism-Request-Id` | 请求 ID（`prism-<时间戳hex>-<序号hex>`）|
| `X-Prism-Attempt-Count` | 重试次数 |
| `X-Prism-Bytes-Received` | 上游响应字节数 |

### Routing API

**端口：** 8080

```bash
# 路由决策（不转发请求，只返回选中的 variant）
POST /v1/route
Content-Type: application/json

{
  "messages": [{"role": "user", "content": "Write quicksort in Rust"}],
  "strategy": "balanced"
}

# 结果反馈（调整路由权重）
POST /v1/outcomes
Content-Type: application/json

{
  "variant_id": "gpt-4o@openai",
  "success": true,
  "total_latency_ms": 1200,
  "prompt_tokens": 50,
  "completion_tokens": 200,
  "user_rating": 5
}
```

### Admin API

**端口：** 8080（需 Bearer Token）

```bash
# 健康检查
GET /health
GET /readyz

# 版本信息
GET /version

# Prometheus 指标
GET /metrics

# 热重载配置
POST /admin/reload
Authorization: Bearer ${PRISM_ADMIN_TOKEN}

# 验证配置（不应用）
POST /admin/config/validate
Authorization: Bearer ${PRISM_ADMIN_TOKEN}
Content-Type: application/yaml
(body: YAML config)

# 用量查询（需 usage-db 或 usage-pg feature）
GET /admin/usage?from=2026-03-01&to=2026-03-28
Authorization: Bearer ${PRISM_ADMIN_TOKEN}

# 异步任务管理
GET    /v1/tasks                    # 列出任务
GET    /v1/tasks/{task_id}          # 查询任务状态
POST   /v1/tasks/{task_id}/cancel   # 取消任务
POST   /v1/tasks/webhook            # 上游 webhook 回调
```

---

## 路由策略详解

Prism 使用 5 层路由管线：

| 层 | 职责 | 示例 |
|---|---|---|
| **L1** | 模型选择 | "这是代码问题 → 选 GPT-4o" |
| **L2** | Provider 选择 | "用户在美东 → 选最近的 endpoint" |
| **L3** | Account 选择 | "team-a 的请求 → 用 team-a 的账户" |
| **L4** | Variant 选择 | "10% 流量走 canary 版本" |
| **L5** | 容错兜底 | "Provider 超时 → 自动重试 / fallback" |

**内置 18 个路由插件：**

| 插件 | 类型 | 说明 |
|------|------|------|
| `HealthFilter` | Filter | 过滤不健康的 variant |
| `CapabilityFilter` | Filter | 过滤不支持请求 capability 的 variant |
| `CircuitBreaker` | Filter | 过滤熔断的 provider |
| `ProtocolFilter` | Filter | 过滤协议不匹配的 |
| `PinnedModel` | Filter | 客户端指定 model 时精确匹配 |
| `InferenceLoad` | Filter | 过滤 KV cache > 95% 的 Pod |
| `CostScorer` | Scorer | 按成本打分 |
| `LatencyScorer` | Scorer | 按延迟打分 |
| `QualityScorer` | Scorer | 按 quality_tier 打分 |
| `SpecialtyScorer` | Scorer | 按 specialty 匹配打分 |
| `FeedbackScorer` | Scorer | 按历史反馈打分 |
| `GeoProximity` | Scorer | 按地理距离打分 |
| `StrategyScorer` | Scorer | 按策略权重综合打分 |
| `AffinityScorer` | Scorer | 会话亲和性打分 |
| `Canary` | Scorer | 金丝雀流量分割 |
| `Weighted` | Scorer | 按权重打分 |
| `QuotaAware` | Scorer | 配额剩余量打分 |
| `PrefixAffinity` | Scorer | Prefix cache 亲和性（自建引擎） |

---

## 可观测性

### Prometheus 指标

```bash
curl http://localhost:8080/metrics
```

**核心指标：**

| 指标名 | 类型 | 说明 |
|--------|------|------|
| `prism_routing_decision_duration_us` | Histogram | 路由决策耗时 |
| `prism_provider_requests_total` | Counter | Provider 请求数 |
| `prism_provider_errors_total` | Counter | Provider 错误数 |
| `prism_provider_latency_sum_ms` | Counter | Provider 累计延迟 |
| `prism_routes_by_model_total` | Counter | 按模型分布 |
| `prism_routes_by_provider_total` | Counter | 按 Provider 分布 |
| `prism_input_tokens_total` | Counter | Input token 总量 |
| `prism_output_tokens_total` | Counter | Output token 总量 |
| `prism_active_requests` | Gauge | 当前活跃请求数 |
| `prism_circuit_breaker_state` | Gauge | 熔断器状态 |
| `prism_rate_limit_utilization` | Gauge | 限流使用率 |

### Debug 模式

请求带 `X-Prism-Debug: true` 返回完整决策链路：

```bash
curl -H "X-Prism-Debug: true" \
  http://localhost:8000/v1/chat/completions \
  -d '{"model": "auto", "messages": [...]}'
```

响应头包含完整路由过程信息。

---

## 高级功能

### P/D 分离调度

Prefill/Decode 分离，优化 GPU 利用率。

```yaml
variants:
  - id: qwen-72b-prefill@gpu-01
    model_id: qwen3-72b
    provider_id: ai-infra
    base_url: "http://prefill-pod:8080"
    metadata:
      pd_role: "prefill"

  - id: qwen-72b-decode@gpu-02
    model_id: qwen3-72b
    provider_id: ai-infra
    base_url: "http://decode-pod:8080"
    metadata:
      pd_role: "decode"
```

Prism 自动：
1. 发送 prefill 请求到 prefill Pod (fire-and-forget)
2. 发送 decode 请求到 decode Pod（返回流式响应给客户端）
3. Prefill 超时时自动取消 decode 端
4. 按负载选择最空闲的 prefill/decode Pod

### WebSocket 代理

OpenAI Realtime API 等 WebSocket 协议自动透传。

```bash
# WebSocket 连接自动走 HTTP Upgrade
wscat -c ws://localhost:8000/v1/realtime?model=gpt-4o-realtime
```

Prism 自动检测 `Upgrade: websocket` header，跳过 body 解析，直接透传双向流。

### 异步任务管理

视频生成等长时间任务的异步管理。

```bash
# 提交异步任务
POST /v1/images/generations
→ Prism 返回 task_id + 立即 202

# 查询任务状态
GET /v1/tasks/{task_id}
→ { "status": "running", "progress": 60 }

# 取消任务
POST /v1/tasks/{task_id}/cancel

# 上游 webhook 回调（自动更新任务状态）
POST /v1/tasks/webhook
```

### WASM 插件

编译自定义 WASM 插件：

```rust
// 实现 proxy-wasm ABI
#[no_mangle]
pub extern "C" fn proxy_on_request_headers() -> i32 {
    // 改写 header, path 等
    0 // Continue
}
```

```bash
cargo build --target wasm32-unknown-unknown --release
```

配置：

```yaml
plugins:
  - name: my-plugin
    source:
      wasm: ./plugins/my-plugin.wasm
    phases: [proxy, route]
    config:
      my_option: true
```

### 配置热更新

```bash
# API 触发
curl -X POST http://localhost:8080/admin/reload \
  -H "Authorization: Bearer ${PRISM_ADMIN_TOKEN}"

# 文件监听（自动）
# 修改 prism.yaml 后 Prism 自动检测并重载
```

热更新使用 arc-swap 原子替换，zero downtime。

---

## 环境变量

所有 `PRISM_` 前缀的环境变量会覆盖配置文件中的对应值。

| 环境变量 | 对应配置 | 示例 |
|----------|----------|------|
| `PRISM_HTTP_PORT` | `server.http_port` | `8080` |
| `PRISM_GRPC_PORT` | `server.grpc_port` | `9090` |
| `PRISM_PROXY_PORT` | `server.proxy_port` | `8000` |
| `PRISM_LISTEN` | `server.listen` | `0.0.0.0:8080` |
| `PRISM_DEFAULT_STRATEGY` | `routing.default_strategy` | `balanced` |
| `PRISM_PROVIDER_<ID>_BASE_URL` | `providers.<id>.base_url` | `https://api.openai.com/v1` |
| `PRISM_PROVIDER_<ID>_API_KEY` | `providers.<id>.api_key` | `sk-xxx` |

配置文件中也可用 `${VAR_NAME}` 引用环境变量：

```yaml
accounts:
  - id: openai-main
    api_key: "${OPENAI_API_KEY}"
```

**优先级**（高覆盖低）：`Environment(30)` > `File(10)` > `Default(0)`

---

## 生产部署建议

### 最小可用配置

```yaml
server:
  proxy_port: 8000
  request_timeout_secs: 300
  retry_on_5xx: true

auth:
  enabled: true
  keys: ["${PRISM_API_KEY}"]

admin:
  enabled: true
  auth_token: "${PRISM_ADMIN_TOKEN}"

access_log:
  enabled: true
  output: file
  file_path: /var/log/prism/access.log
  format: json
```

### 安全清单

- [ ] 开启 `auth.enabled: true`
- [ ] 配置 `admin.auth_token` 保护管理接口
- [ ] API Key 通过环境变量注入（不要硬编码）
- [ ] 生产环境启用 TLS 终止（或前置 nginx/LB）
- [ ] 配置 `client_keys` 实现多租户隔离
- [ ] 限制 `cors_origins` 为实际域名

### 高可用

- 部署 ≥2 副本，前置 LB
- 每个实例独立 watch 配置文件（不走共享存储）
- Redis 可选用于分布式限流
- 配置热更新 zero downtime

### 监控

- Prometheus scrape `/metrics` 端口 8080
- 关注指标：`prism_provider_errors_total`, `prism_circuit_breaker_state`, `prism_active_requests`
- Access Log 接 ELK/Loki 做日志分析

---

## 故障排查

### 常见问题

**1. 上游返回空 body (400)**

可能原因：body 在 proxy 层被消费。检查：
- `upstream_model_name` 配置是否正确
- Access Log 的 `%ERROR%` 字段

**2. 502 Bad Gateway**

可能原因：
- 上游不可达 → 检查 `base_url`
- TLS 问题 → macOS 上 Pingora HTTPS 出站有已知问题
- 解决：配置 per-provider `request_timeout_secs`

**3. 路由不到预期模型**

调试步骤：
```bash
# 1. 用 Debug header 查看决策链路
curl -H "X-Prism-Debug: true" ...

# 2. 检查候选列表
curl http://localhost:8080/admin/candidates

# 3. 检查熔断器状态
curl http://localhost:8080/metrics | grep circuit_breaker
```

**4. 限流误判**

```bash
# 查看限流指标
curl http://localhost:8080/metrics | grep rate_limit

# 检查 client_keys 配额配置
# 注意 model_quotas 优先于全局 quota
```

### 日志级别

```bash
# 调试模式
RUST_LOG=prism_router=debug,prism_core=debug ./prism-router --config ...

# 只看路由决策
RUST_LOG=prism_core::router=debug ./prism-router --config ...

# 只看 proxy 层
RUST_LOG=prism_router::proxy=debug ./prism-router --config ...
```

---

## 完整配置示例

参见 [`examples/prism.example.yaml`](../examples/prism.example.yaml)。
