<div align="center">

# 🔺 Prism

**High-Performance LLM Intelligent Routing Engine**

*Route every request to the optimal model — automatically.*

[![Rust](https://img.shields.io/badge/Rust-Core_Engine-orange?logo=rust)](prism-core/)
[![Tests](https://img.shields.io/badge/Tests-1461_passing-brightgreen)]()
[![License](https://img.shields.io/badge/License-MIT-blue)]()

[English](./README.md) · [中文](./README.zh-CN.md)

</div>

---

## Why Prism?

When you use multiple LLMs (GPT-4o, Claude, DeepSeek, etc.) simultaneously, you face these problems:

- 💸 Simple questions on GPT-4o are expensive; complex questions on mini models give poor results
- 🔄 Provider rate-limited? Manual switching is painful
- 💰 Different teams need separate billing accounts
- 🧪 Want to A/B test models but lack the infrastructure

**Prism solves this.** It analyzes request content, determines complexity, and picks the optimal model + provider in **under 90 microseconds** — based on your strategy preferences (save money? low latency? best quality?).

### Key Features

| | Feature | Description |
|---|---|---|
| 🧠 | **Smart Routing** | Analyzes request content (code / reasoning / translation / chat) to match the best model |
| ⚡ | **Blazing Fast** | Rust core engine, **90μs** routing decisions (100 variants), linear scaling |
| 🎯 | **Multi-Strategy** | Balanced / SLO / Cost-First / Quality-First — switchable per request |
| 🔌 | **Drop-in** | OpenAI-compatible API — just change `base_url` |
| 📊 | **Observable** | Prometheus metrics with 6-dimension monitoring, request tracing, cost analysis |
| 🔄 | **Feedback Loop** | Dynamically adjusts routing weights based on actual outcomes |
| 🧪 | **A/B Testing** | Config-driven experiments with traffic splitting |
| 🏗️ | **Flexible Deploy** | Standalone HTTP + gRPC + Reverse Proxy (Pingora) |
| 🔧 | **Plugin System** | Extensible filter/scorer plugins for custom routing logic |
| 🧩 | **5-Dimension WASM** | Config / Discovery / Route / Proxy / Analyze — all extensible via WASM |
| ⚙️ | **Pluggable Config** | ConfigSource trait with file, env, WASM, and composite sources |

## How Prism Compares

| | **Prism** | LiteLLM | OpenRouter | Portkey |
|---|---|---|---|---|
| **Language** | 🦀 Rust | 🐍 Python | hosted only | 🐍 Python + 🟦 TS |
| **Routing decision latency** | **~90 µs** (100 variants) | ~1–5 ms | network RTT | ~1–3 ms |
| **Self-hosted** | ✅ Single binary | ✅ Pip / Docker | ❌ | ✅ Docker |
| **Multi-strategy routing** | ✅ 4 built-in + custom | ⚠️ Basic | ⚠️ Basic | ✅ |
| **Inference-aware** (vLLM/SGLang KV cache, P/D split) | ✅ Prefix affinity + GPU load | ❌ | ❌ | ❌ |
| **Plugin system** | ✅ 18 built-in + WASM (5 phases) | ⚠️ Callbacks | ❌ | ⚠️ Hooks |
| **Reverse proxy mode** | ✅ Pingora-based | ⚠️ Python proxy | n/a | ⚠️ Node proxy |
| **Config hot-reload** | ✅ 9.8 ns/req lookup (arc-swap) | partial | n/a | ✅ |
| **Distributed rate limiting** | ✅ Redis Lua sliding window | ✅ Redis | ✅ | ✅ |
| **License** | MIT | MIT | proprietary | MIT |

> **TL;DR:** If you need a hosted aggregator → OpenRouter. If you're a Python shop and don't mind the latency → LiteLLM. **If you run self-hosted inference (vLLM/SGLang/TGI) at scale, care about µs-level overhead, and want one Rust binary that does HTTP + gRPC + proxy + plugins — Prism is built for you.**

## Architecture

Prism is a Rust workspace with two crates:

```
                          ┌──────────────────────────────────────┐
                          │         prism-router (Rust)          │
                          │                                      │
                          │  HTTP API ──┐                        │
  Client ─────────────────│  gRPC API ──┼──▶ prism-core (Rust)  │
  (OpenAI-compatible)     │  Proxy   ───┘   │                   │
                          │                 ├─ Analyzer          │
                          │                 ├─ Plugin System     │
                          │                 ├─ Strategy Engine   │
                          │                 ├─ Model Registry    │
                          │                 ├─ Discovery Layer   │
                          │                 ├─ DataLayer         │
                          │                 └─ Feedback Loop     │
                          │                                      │
                          │  ConfigSource (File/Env/WASM)        │
                          │  PluginLoader (File/URL/OCI)         │
                          │  PluginManager (load/unload/reload)  │
                          │                                      │
                          │  Proxy Layer:                        │
                          │  ├─ Codec Plugin Chain (WASM/Native) │
                          │  ├─ Rate Limiting                    │
                          │  └─ Model Name Rewrite               │
                          └──────────────────────────────────────┘
                                        │
                          ┌─────────────┼──────────────┐
                          │             │              │
                          ▼             ▼              ▼
                       OpenAI      Anthropic       DeepSeek
                                                  (any OpenAI-compatible)
```

### 5-Layer Scheduling Engine

| Layer | Responsibility | Example |
|:---:|---|---|
| **L1** | Model Selection | *"This is a code question → pick GPT-4o"* |
| **L2** | Provider Selection | *"User is in us-east → pick nearest endpoint"* |
| **L3** | Account Selection | *"team-a's request → route to team-a's billing account"* |
| **L4** | Variant Selection | *"Send 10% traffic to canary version"* |
| **L5** | Resilience | *"Provider timeout → auto retry / fallback"* |

### Serving Modes

prism-router exposes three interfaces simultaneously, each toggleable via feature flags:

| Interface | Feature Flag | Default Port | Use Case |
|---|---|---|---|
| **HTTP API** | *(always on)* | 8080 | Routing decisions via REST, health checks, Prometheus metrics |
| **gRPC** | `grpc` | 9090 | High-performance routing service for gateway integration |
| **Reverse Proxy** | `proxy` | 8081 | Drop-in OpenAI-compatible proxy (Pingora-based), routes + forwards requests |

## Quick Start

### Build

```bash
# Requires Rust 1.82+
cargo build --release

# With all features (default)
cargo build --release -p prism-router

# HTTP-only (no gRPC, no proxy)
cargo build --release -p prism-router --no-default-features
```

### Configure

```bash
cp configs/prism.example.yaml configs/prism.yaml
# Edit prism.yaml with your API keys and model definitions
```

<details>
<summary>📄 Example configuration</summary>

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

### Run

```bash
# Direct
./target/release/prism-router --config configs/prism.yaml

# Or Docker
docker compose up -d
```

### Use

**HTTP API — Routing Decision:**

```bash
curl -X POST http://localhost:8080/v1/route \
  -H "Content-Type: application/json" \
  -d '{
    "messages": [{"role": "user", "content": "Write a quicksort in Rust"}],
    "strategy": "balanced"
  }'
```

**Reverse Proxy — Drop-in OpenAI Replacement (port 8081):**

```bash
# Same as OpenAI API — just change base_url
curl http://localhost:8081/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "auto",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

The proxy response includes routing metadata in headers:

```
X-Prism-Model: gpt-4o-mini
X-Prism-Provider: openai
X-Prism-Variant: gpt-4o-mini@openai
X-Prism-Decision-Time-Us: 92
```

## Routing Strategies

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

## Advanced Features

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

Prism adjusts routing scores based on real-world outcomes via its feedback loop.

</details>

<details>
<summary><b>Plugin System</b> — extensible routing logic</summary>

prism-core uses a plugin pipeline for routing decisions:

- **Filter Plugins** — remove candidates that don't meet requirements (capabilities, health, circuit breaker, protocol, inference load)
- **Scorer Plugins** — score remaining candidates on cost, latency, quality, specialty, feedback, geo-proximity, strategy weights, prefix affinity, inference load

Built-in plugins (18): `HealthFilter`, `CapabilityFilter`, `CircuitBreaker`, `ProtocolFilter`, `PinnedModel`, `CostScorer`, `LatencyScorer`, `QualityScorer`, `SpecialtyScorer`, `FeedbackScorer`, `GeoProximity`, `StrategyScorer`, `AffinityScorer`, `Canary`, `Weighted`, `QuotaAware`, `PrefixAffinity`, `InferenceLoad`

**Inference-aware plugins** (for self-hosted engines like vLLM / SGLang):
- `PrefixAffinityPlugin` — consistent-hash routing by prompt prefix, maximizes prefix-cache reuse
- `InferenceLoadPlugin` — filters out pods with KV cache > 95%, scores by GPU utilization and queue depth

Custom plugins implement the `RoutingPlugin` trait.

</details>

<details>
<summary><b>WASM Proxy Plugins</b> — extend the proxy layer with WASM</summary>

prism-router supports proxy-layer plugins compiled to WebAssembly via a full codec plugin chain framework. WASM plugins intercept HTTP request/response at the reverse proxy layer for protocol translation, header manipulation, and body transformation.

The plugin chain aligns with the **proxy-wasm ABI** and supports:
- **4 hook points**: `on_request_headers`, `on_request_body`, `on_response_headers`, `on_response_body`/`on_response_stream_chunk`
- **ProxyPluginChain**: request-phase in order, response-phase in reverse
- **CodecRegistry**: maps provider/protocol pairs to the appropriate plugin chain
- **wasmtime 42 host**: implements core proxy-wasm ABI subset (get/set headers, get/set buffer, log)

**Example: OpenAI Compatibility Plugin** ([`examples/wasm-plugins/openai-compat/`](examples/wasm-plugins/openai-compat/))

Normalizes non-standard requests to OpenAI format:
- Rewrites `/api/chat` → `/v1/chat/completions`
- Adds `X-Prism-Converted: true` header
- Converts `{"prompt": "..."}` → `{"messages": [...]}`
- Adds `"prism_converted": true` to responses

```bash
# Build the example plugin
cd examples/wasm-plugins/openai-compat
cargo build --target wasm32-unknown-unknown --release

# Run WASM integration tests
make test-wasm
```

Enable with feature flag: `cargo build -p prism-router --features wasm`

</details>

<details>
<summary><b>5-Dimension WASM Plugin System</b> — one module, multiple phases</summary>

A single WASM module can participate in multiple phases simultaneously. The unified plugin framework auto-detects capabilities from module exports:

| Dimension | Phase | ABI | Key Exports | Purpose |
|:---:|---|---|---|---|
| **1** | Config | Prism Config ABI | `prism_config_init`, `prism_config_load` | Configuration source plugin |
| **2** | Discovery | Prism Discovery ABI | `prism_discovery_init`, `prism_discovery_discover` | Candidate discovery |
| **3** | Route | Prism Routing ABI | `prism_route_filter`, `prism_route_score` | Routing filter/scorer |
| **4** | Proxy | proxy-wasm ABI | `proxy_on_request_headers/body`, `proxy_on_response_headers/body` | Protocol translation |
| **5** | Analyze | Prism Analyzer ABI | `analyze` | Request feature extraction |

```yaml
plugins:
  - name: my-plugin
    source:
      wasm: ./plugins/my-plugin.wasm
    phases: [proxy, route, analyze]  # Participate in multiple phases
    enabled: true
    priority: 100
```

</details>

<details>
<summary><b>Pluggable Configuration</b> — ConfigSource trait + multi-source merging</summary>

prism-router supports pluggable configuration sources via the `ConfigSource` trait:

| Source | Description |
|---|---|
| `FileSource` | Load from local YAML/JSON/TOML files |
| `EnvSource` | Override from environment variables (`PRISM_` prefix) |
| `WasmConfigSource` | Load config from any source via WASM module (feature-gated) |
| `CompositeSource` | Merge multiple sources by priority |

**Priority order** (higher overrides lower): `Base(0) < File(10) < Remote(20) < Environment(30) < Override(40)`

</details>

<details>
<summary><b>Plugin Loader</b> — multi-source plugin resolution</summary>

The `PluginLoader` resolves plugin sources to WASM bytes with caching and integrity verification:

| Source | Config | Description |
|---|---|---|
| **File** | `{ wasm: "./path.wasm" }` | Local filesystem |
| **URL** | `{ url: "https://...", sha256: "..." }` | HTTP/HTTPS with optional SHA-256 checksum |
| **OCI** | `{ oci: "registry/name:tag" }` | OCI container registry |
| **Builtin** | `{ builtin: "name" }` | Built-in Rust plugin (no WASM bytes) |

Downloaded plugins are cached locally (`~/.prism/plugins/cache/`) with SHA-256 based deduplication.

</details>

<details>
<summary><b>Plugin Manager</b> — runtime lifecycle management</summary>

The `PluginManager` provides runtime plugin lifecycle operations:

- **Load**: resolve source → compile WASM → register to phases
- **Unload**: remove from all phases → release resources
- **Reload**: atomic replace (load new → swap → unload old)
- **List**: query loaded plugin status and metadata

Events (`Loaded`, `Unloaded`, `Reloaded`, `LoadFailed`) are emitted for monitoring and logging.

</details>

<details>
<summary><b>Rate Limiting</b> — configurable, header-based rate limiting</summary>

prism-router includes a built-in rate limiting system with flexible key extraction:

- **Header-based keys**: combine any request headers (e.g., `x-model`, `x-app-id`) into rate limit keys
- **Sliding window**: request count and token count limiting per time window
- **Differential overrides**: different limits for different header value combinations
- **Most-specific match wins**: override with the most matching conditions takes priority

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
        max_requests: 80  # More specific → wins
```

</details>

<details>
<summary><b>Model Name Mapping</b> — upstream_model_name rewrite</summary>

Each `ModelVariant` can specify an `upstream_model_name` to override the `model` field in the request body before forwarding to the upstream provider. This is useful when:

- The provider's API expects a different model name (e.g., `deepseek-chat` vs your internal `deepseek-v3`)
- Self-hosted engines use path-based model names (e.g., `/data/models/qwen2.5-72b-instruct`)

```yaml
variants:
  - id: "deepseek-v3@deepseek"
    model_id: deepseek-v3
    provider_id: deepseek
    upstream_model_name: "deepseek-chat"
```

The proxy automatically rewrites the JSON body's `"model"` field before forwarding.

</details>

## Observability

```bash
# Prometheus metrics
curl http://localhost:8080/metrics
```

### Metrics Dimensions (6 categories)

| Category | Metrics |
|---|---|
| **Routing Decision** | `prism_routing_decision_duration_us` (histogram), `prism_strategy_calls_total`, `prism_fallback_triggered_total`, plugin filter/score duration |
| **Provider** | `prism_provider_requests_total`, `prism_provider_errors_total`, `prism_provider_latency_sum_ms`, `prism_rate_limit_utilization`, `prism_circuit_breaker_state` |
| **Traffic Distribution** | `prism_routes_by_model_total`, `prism_routes_by_provider_total`, `prism_routes_by_account_total`, `prism_experiment_assignments_total` |
| **Token Usage** | `prism_input_tokens_total`, `prism_output_tokens_total`, `prism_tokens_per_second` |
| **Resources** | `prism_active_requests`, `prism_queue_depth` |
| **DataLayer** | `prism_data_layer_queries_total`, `prism_data_layer_hits_total`, `prism_data_layer_latency_sum_us` |

Per-variant latency percentiles (P50/P95/P99), error rate, QPS, and TTFT are also exported.

## Project Structure

```
prism/
├── prism-core/              # 🦀 Rust routing engine (library)
│   ├── src/
│   │   ├── lib.rs               # Module declarations
│   │   ├── types.rs             # Core types: Model, Variant, Provider, Pool, etc.
│   │   ├── analyzer.rs          # Request analyzer (rule-based / feature / webhook)
│   │   ├── strategy.rs          # L1-L5 strategy definitions
│   │   ├── router.rs            # PrismRouter — main routing logic
│   │   ├── selector.rs          # Candidate scoring & selection
│   │   ├── registry.rs          # ModelRegistry — model/variant/pool management
│   │   ├── plugin.rs            # RoutingPlugin trait & pipeline
│   │   ├── plugins/             # 18 built-in plugins (filter + scorer)
│   │   ├── discovery/           # Pluggable discovery layer
│   │   │   ├── mod.rs               # Discovery trait
│   │   │   ├── static_discovery.rs  # Static config (default)
│   │   │   └── wasm_discovery.rs    # WASM discovery (feature-gated)
│   │   ├── data.rs              # DataLayer — metrics storage abstraction
│   │   ├── feedback.rs          # Feedback collector (outcome → weight adjustment)
│   │   └── ffi.rs               # C FFI interface for embedding
│   └── Cargo.toml
├── prism-router/            # 🚀 Standalone routing service
│   ├── src/
│   │   ├── main.rs              # Entrypoint: HTTP + gRPC + Proxy servers
│   │   ├── api.rs               # Axum HTTP handlers (route, outcomes, health, metrics)
│   │   ├── grpc.rs              # tonic gRPC RouterService implementation
│   │   ├── proxy.rs             # Pingora reverse proxy + upstream_model_name rewrite
│   │   ├── protocol.rs          # Protocol detection (completions, embeddings, etc.)
│   │   ├── metrics.rs           # MetricsCollector — 6-dimension Prometheus metrics
│   │   ├── config/              # Pluggable configuration sources
│   │   │   ├── mod.rs               # ConfigSource trait, CompositeSource
│   │   │   ├── types.rs             # PrismConfig full type definitions
│   │   │   ├── file_source.rs       # FileSource — YAML/JSON/TOML loading
│   │   │   ├── env_source.rs        # EnvSource — environment variable loading
│   │   │   ├── wasm_source.rs       # WasmConfigSource (feature-gated)
│   │   │   └── registry.rs          # build_registry() — PrismConfig → ModelRegistry
│   │   ├── state.rs             # AppState (shared Router + Metrics)
│   │   ├── ratelimit.rs         # Rate limiting (sliding window + header-based keys)
│   │   ├── unified_plugin.rs    # Unified plugin framework (PluginRegistry + PluginDescriptor)
│   │   ├── plugin_loader.rs     # Plugin loader (File/URL/OCI/Builtin + cache + SHA-256)
│   │   ├── plugin_manager.rs    # Plugin manager (load/unload/reload + event callbacks)
│   │   └── codec/               # Proxy plugin chain (protocol translation)
│   │       ├── mod.rs               # ProxyPlugin trait + ProxyContext
│   │       ├── chain.rs             # ProxyPluginChain
│   │       ├── registry.rs          # CodecRegistry
│   │       └── wasm_host.rs         # wasmtime host (feature-gated)
│   └── Cargo.toml
├── examples/
│   └── wasm-plugins/
│       └── openai-compat/       # Example WASM plugin — OpenAI compatibility
├── proto/                   # 📋 Protobuf definitions
│   └── prism/router/v1/
│       └── router.proto
├── configs/                 # ⚙️ Configuration files
│   ├── prism.example.yaml
│   ├── prism.yaml
│   └── prometheus.yml
├── Makefile                 # Build & test commands
├── Dockerfile               # Multi-stage build
├── docker-compose.yml       # Prism + Redis + PostgreSQL + Prometheus
└── DESIGN.md                # Full technical design document
```

## Development

```bash
cargo build --release        # Build everything
cargo test                   # Run all tests (803 passing)
cargo bench                  # Benchmarks (prism-core + prism-router)
cargo fmt --all              # Format code
cargo clippy -- -D warnings  # Lint check
```

### Feature Flags

| Flag | Default | Description |
|---|---|---|
| `grpc` | ✅ | Enable gRPC RouterService (tonic) |
| `proxy` | ✅ | Enable Pingora reverse proxy |
| `wasm` | ❌ | Enable WASM proxy plugin support (wasmtime) in prism-router |
| `wasm` (prism-core) | ❌ | Enable WASM discovery plugin support |

## Performance

### Routing Decision Latency

| Variants | Routing | Full Proxy Pipeline | Notes |
|----------|---------|--------------------:|-------|
| **100** | 90 µs | 100 µs | Typical small-scale deployment |
| **1,000** | 900 µs | 980 µs | Mid-scale with 32p × 32m |
| **10,000** | 11 ms | 11 ms | Large multi-provider fleet |
| **100,000** | 146 ms | 132 ms | Extreme scale (317p × 317m) |

> Full proxy pipeline = rate limiting + routing + codec body transform + metrics recording.
> Pipeline overhead is **~10µs** — routing dominates at all scales.

### Component Benchmarks

| Component | Metric | Latency |
|-----------|--------|--------:|
| Rate limiter | Global rule check | 100 ns |
| Rate limiter | Per-model keyed | 150 ns |
| Rate limiter | 5 rules sequential | 500 ns |
| Rate limiter | Override resolution (5 overrides) | 300 ns |
| Codec chain | Request body transform (2KB) | 2 µs |
| Codec chain | SSE stream chunk (1 plugin) | 100 ns |
| Codec registry | Protocol-pair lookup | 78 ns |
| Config hot-reload | **arc-swap load (per-request)** | **9.8 ns** |
| Config hot-reload | arc-swap store (reload) | 22 µs |
| Config hot-reload | Registry rebuild (400 variants) | 295 µs |
| Metrics | Single variant record | 80 ns |
| Metrics | 6-dimension record | 1.7 µs |
| Metrics | Prometheus format output | 28 µs |

### Run Benchmarks

```bash
# Core routing benchmarks
cargo bench --bench routing

# Proxy layer benchmarks (ratelimit, codec, hotreload, metrics, e2e)
cargo bench --bench proxy

# E2E only
cargo bench --bench proxy -- "e2e/"
```

### Test Coverage

| Suite | Count |
|-------|------:|
| Default (`cargo test`) | 803 |
| With WASM (`--features wasm`) | 945 |
| E2E integration | 24 |
| **Total** | **~970** |

## Tech Stack

| Component | Technology | Purpose |
|---|---|---|
| prism-core | Rust | Routing engine, request analysis, plugin scoring |
| prism-router | Rust (Axum + tonic + Pingora) | HTTP API, gRPC service, reverse proxy |
| Monitoring | Prometheus | Metrics collection (6-dimension) |
| State (optional) | Redis | Real-time QPS / latency / error rate |
| Logs (optional) | PostgreSQL | Audit logging & cost analysis |

## Documentation

- [Full Technical Design Document](./DESIGN.md) — Architecture, 5-layer strategy engine, object model, deployment modes
- [Configuration Example](./configs/prism.example.yaml)
- [Protobuf Definitions](./proto/prism/router/v1/router.proto)

## Roadmap

What's next, roughly in priority order. PRs welcome on any of these — open an issue first to align on approach.

- 🚧 **Usage tracking persistence** — token usage to PG/SQLite for audit & billing
- 🚧 **Prometheus `/metrics` polish** — alignment with industry-standard label conventions
- 🚧 **OpenTelemetry tracing** — `tracing-opentelemetry` → Jaeger / Tempo
- 🚧 **Helm chart** — first-class K8s deployment
- 🚧 **WebSocket / streaming protocols** — beyond SSE
- 🚧 **GPU filter plugin** — multi-tenant GPU isolation for self-hosted clusters
- 💡 **Community ideas** → see [Discussions](https://github.com/jiusanzhou/prism/discussions/categories/ideas)

See the full roadmap in [`MEMORY.md`](./MEMORY.md) (work log) and tagged issues for in-flight work.

## Community

- 💬 **Questions / ideas** → [GitHub Discussions](https://github.com/jiusanzhou/prism/discussions)
- 🐛 **Bug / feature** → [Issues](https://github.com/jiusanzhou/prism/issues)
- 🤝 **First-time contributor** → look for [`good first issue`](https://github.com/jiusanzhou/prism/labels/good%20first%20issue), then read [CONTRIBUTING.md](./CONTRIBUTING.md)
- 🔒 **Security disclosure** → `security@zoe.im` (please, not public issues)

If Prism saved you time, **starring the repo** is the cheapest way to help others discover it — and it lets us know what to build next.

## Don't Want to Self-Host?

A managed Prism with team accounts, hosted config, dashboards, and SLAs is on the way. **[Drop your email here](https://github.com/jiusanzhou/prism/discussions/categories/announcements)** (or watch this repo) to get notified when it lands.

> Self-hosted is — and will always be — fully open source under MIT. The hosted version is for teams that don't want to operate the control plane themselves.

## License

[MIT](./LICENSE) © Zoe and Prism contributors.

You can use Prism in commercial products, fork it, modify it, and ship it — no permission needed. If you build something cool with it, we'd love to hear about it in [Discussions](https://github.com/jiusanzhou/prism/discussions).
