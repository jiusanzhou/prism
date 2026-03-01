<div align="center">

# 🔺 Prism

**High-Performance LLM Intelligent Routing Engine**

*Route every request to the optimal model — automatically.*

[![Rust](https://img.shields.io/badge/Rust-Core_Engine-orange?logo=rust)](prism-core/)
[![Tests](https://img.shields.io/badge/Tests-97_passing-brightgreen)]()
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

**Prism solves this.** It analyzes request content, determines complexity, and picks the optimal model + provider in **under 8 microseconds** — based on your strategy preferences (save money? low latency? best quality?).

### Key Features

| | Feature | Description |
|---|---|---|
| 🧠 | **Smart Routing** | Analyzes request content (code / reasoning / translation / chat) to match the best model |
| ⚡ | **Blazing Fast** | Rust core engine, **8μs** routing decisions, P99 < 5ms |
| 🎯 | **Multi-Strategy** | Balanced / SLO / Cost-First / Quality-First — switchable per request |
| 🔌 | **Drop-in** | OpenAI-compatible API — just change `base_url` |
| 📊 | **Observable** | Prometheus metrics with 6-dimension monitoring, request tracing, cost analysis |
| 🔄 | **Feedback Loop** | Dynamically adjusts routing weights based on actual outcomes |
| 🧪 | **A/B Testing** | Config-driven experiments with traffic splitting |
| 🏗️ | **Flexible Deploy** | Standalone HTTP + gRPC + Reverse Proxy (Pingora) |
| 🔧 | **Plugin System** | Extensible filter/scorer plugins for custom routing logic |

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
                          │                 ├─ DataLayer         │
                          │                 └─ Feedback Loop     │
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
X-Prism-Decision-Time-Us: 8
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

- **Filter Plugins** — remove candidates that don't meet requirements (capabilities, health, circuit breaker, protocol)
- **Scorer Plugins** — score remaining candidates on cost, latency, quality, specialty, feedback, geo-proximity, strategy weights

Built-in plugins: `HealthFilter`, `CapabilityFilter`, `CircuitBreaker`, `ProtocolFilter`, `PinnedModel`, `CostScorer`, `LatencyScorer`, `QualityScorer`, `SpecialtyScorer`, `FeedbackScorer`, `GeoProximity`, `StrategyScorer`, `AffinityScorer`, `Canary`, `Weighted`

Custom plugins implement the `RoutingPlugin` trait.

</details>

<details>
<summary><b>WASM Proxy Plugins</b> — extend the proxy layer with WASM</summary>

prism-router supports proxy-layer plugins compiled to WebAssembly. These intercept HTTP request/response at the reverse proxy layer for protocol translation, header manipulation, and body transformation.

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

Enable with feature flag: `cargo build --features wasm`

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
│   │   ├── plugins/             # 15 built-in plugins (filter + scorer)
│   │   ├── data.rs              # DataLayer — metrics storage abstraction
│   │   ├── feedback.rs          # Feedback collector (outcome → weight adjustment)
│   │   └── ffi.rs               # C FFI interface for embedding
│   └── Cargo.toml
├── prism-router/            # 🚀 Standalone routing service
│   ├── src/
│   │   ├── main.rs              # Entrypoint: HTTP + gRPC + Proxy servers
│   │   ├── api.rs               # Axum HTTP handlers (route, outcomes, health, metrics)
│   │   ├── grpc.rs              # tonic gRPC RouterService implementation
│   │   ├── proxy.rs             # Pingora reverse proxy (OpenAI-compatible)
│   │   ├── protocol.rs          # Protocol detection (completions, embeddings, etc.)
│   │   ├── metrics.rs           # MetricsCollector — 6-dimension Prometheus metrics
│   │   ├── config.rs            # YAML config loading → Registry construction
│   │   └── state.rs             # AppState (shared Router + Metrics)
│   └── Cargo.toml
├── proto/                   # 📋 Protobuf definitions
│   └── prism/router/v1/
│       └── router.proto
├── configs/                 # ⚙️ Configuration files
│   ├── prism.example.yaml
│   ├── prism.yaml
│   └── prometheus.yml
├── Makefile                 # Build & test commands
├── Dockerfile               # Multi-stage build (Rust + Go FFI gateway)
├── docker-compose.yml       # Prism + Redis + PostgreSQL + Prometheus
└── DESIGN.md                # Full technical design document
```

## Development

```bash
cargo build --release        # Build everything
cargo test                   # Run all tests (97 passing)
cargo bench                  # Benchmarks (prism-core)
cargo fmt --all              # Format code
cargo clippy -- -D warnings  # Lint check
```

### Feature Flags

| Flag | Default | Description |
|---|---|---|
| `grpc` | ✅ | Enable gRPC RouterService (tonic) |
| `proxy` | ✅ | Enable Pingora reverse proxy |
| `wasm` | ❌ | Enable WASM plugin support (wasmtime) in prism-core |

## Performance

| Metric | Measured | Target |
|---|---|---|
| Routing decision | **8μs** | < 500μs |
| P99 latency | < 5ms | < 5ms |
| Memory footprint | ~30MB | < 100MB |

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

## License

MIT
