# Changelog

## v0.1.0 (2026-03-09)

First production release of Prism — an AI model router and proxy.

### Core Routing
- 5-layer routing engine: Model → Provider → Account → Variant → Pool
- 17 built-in routing plugins (affinity, canary, circuit breaker, cost, feedback, geo proximity, health filter, inference load, in-flight load, latency, P/D routing, pinned model, prefix affinity, protocol filter, quality, quota-aware, specialty, strategy, weighted)
- P/D (Prefill/Decode) disaggregated inference routing with load-aware selection
- Pluggable prefix cache index (xxh3 block hash + LRU)
- In-flight request tracking with lock-free atomic counters

### Proxy
- Pingora 0.8 reverse proxy with OpenAI-compatible API (`/v1/chat/completions`)
- SSE streaming forwarding
- Request body model extraction and routing
- Model alias / fallback mapping
- 5xx retry with failover (Pingora retry loop)
- Per-provider request timeout and streaming idle timeout
- Passive 5xx health feedback → circuit breaker

### Multi-Tenant
- Client API key authentication
- Quota enforcement: RPM, TPM, TPD (global + per-model)
- Concurrent request limits
- Model ACL (allowlist per key)

### Observability
- Access log: Envoy-style `%COMMAND_OPERATOR%` format, 40+ operators, JSON/Text output, file rotation
- Metrics: 6-dimension drill-down (model/provider/endpoint/account/region/variant)
- Health check + circuit breaker with passive feedback

### Service Discovery
- Static configuration
- Sidecar SSE protocol (Go k8s-discovery compatible)
- WASM discovery plugins

### Provider Plugins
- Built-in: OpenAI compatible, AWS Bedrock, Google Vertex AI
- WASM provider plugin framework
- Provider-free variant routing (K8s direct pod access)

### Operations
- Config hot-reload (arc-swap atomic replacement + file watcher)
- TLS termination (optional)
- Admin UI: 12 management pages (React + shadcn/ui), bearer token auth
- Docker image: `ghcr.io/jiusanzhou/prism`
- CI/CD: GitHub Actions (fmt → clippy → test → Docker build + GHCR push)
- Example config: `examples/prism.example.yaml`

### Performance
- Route decision: p50=61µs, p99<5ms
- Throughput: 15K req/s concurrent routing
- Provider plugin dispatch: 330ns/call

### Testing
- 1623 tests, 0 failures
- Unit tests for all core modules
- E2E tests: HTTP proxy, gRPC, streaming, retry, quota, load balancing
- 0 clippy warnings

### Security
- Pingora 0.8 (fixes 2 critical HTTP request smuggling CVEs)
- jsonwebtoken 10 (fixes auth bypass)
- lru 0.16 (fixes stacked borrows issue)
