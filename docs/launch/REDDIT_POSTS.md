# Reddit Launch Posts

Different communities want different framing. Below: per-subreddit tailored drafts, with notes on each community's norms.

> **General rules across all subs:**
> - Read 2–3 weeks of recent top posts before posting. Match the tone.
> - Always disclose: "I built this" — not doing so kills credibility instantly.
> - Don't cross-post the same text. Each sub gets its own framing.
> - Stagger posts: one per day, watch each thread for first 4 hours.

---

## r/rust  (300k members)

**Title:** `Prism – an LLM routing engine in Rust (Pingora-based proxy, WASM plugins, ~90µs decisions)`

**Body:**

Hey r/rust — I've been working on this for a few months and just open-sourced it.

[**Prism**](https://github.com/jiusanzhou/prism) is a routing engine for LLM requests: you point it at multiple model providers (OpenAI, Anthropic, self-hosted vLLM, etc.), it picks the best (model, provider, account) per request based on your strategy, and either returns the decision (HTTP/gRPC API) or proxies the request transparently (Pingora-based OpenAI-compatible proxy on port 8081).

**Why this might interest r/rust specifically:**

- **Pingora integration.** The reverse-proxy mode runs on Cloudflare's Pingora. Getting per-request retry + streaming body rewriting + plugin chains working on top of Pingora's session model was a non-trivial Rust exercise — happy to write up specifics if anyone's curious.
- **WASM host (wasmtime 42).** Five-phase plugin ABI: Config / Discovery / Route / Proxy / Analyze. A single .wasm module can participate in multiple phases. Implements a subset of the proxy-wasm ABI for the proxy phase.
- **arc-swap everywhere.** Config hot-reload is 9.8 ns/req via `ArcSwap<Config>`. The whole `prism-router` is built around the principle that per-request fast paths must be lock-free.
- **No `unsafe` outside FFI boundaries.** ~1 800 tests passing, `clippy -D warnings` clean.
- **Three serving modes from one binary.** HTTP API + tonic gRPC + Pingora proxy, each toggleable via feature flag. The dependency surface stays manageable thanks to features.

Workspace layout:
- `prism-core` — routing engine, plugin traits, analyzer (no async runtime, pure logic)
- `prism-router` — HTTP/gRPC/proxy servers, Pingora integration, WASM host
- `prism-admin` — Axum admin API (TLS, Bearer auth)

Repo: https://github.com/jiusanzhou/prism

Honest feedback welcome — especially on the proxy/Pingora bits and the WASM ABI design.

---

## r/LocalLLaMA  (~200k members) — **highest-conversion sub for this project**

**Title:** `[Project] Prism – router for self-hosted vLLM/SGLang with prefix-cache-aware routing and P/D split`

**Body:**

Cross-posting this because I think this community will appreciate the inference-side features specifically.

I built [**Prism**](https://github.com/jiusanzhou/prism) (Rust, MIT) because running multiple vLLM/SGLang pods behind a naive round-robin load balancer leaves a *lot* of performance on the table:

- **Prefix-cache affinity.** Consistent-hash routing on the prompt prefix means the same conversation hits the same pod repeatedly — and that pod's KV cache stays warm. We use a pluggable index (HashPrefixIndex with xxh3 block hashing + LRU is the default).
- **KV-cache utilisation filter.** Pods reporting >95% KV cache are dropped from the candidate set before scoring.
- **In-flight tracking.** Lock-free atomics counting active requests per pod. The `InFlightLoadPlugin` deprioritises overloaded pods.
- **Prefill / Decode (P/D) split.** First-class support for disaggregated serving — prefill request is fire-and-forget to the prefill pool, decode request is dispatched to the decode pool. P/D filter plugin separates the two roles.
- **K8s auto-discovery.** A Go sidecar watches your inference Pods and pushes endpoints to Prism via SSE. Prism handles 14+ models and 15+ pods on our staging cluster end-to-end.
- **Per-provider timeouts + streaming idle timeouts.** Because "the request timed out somewhere in 600 seconds" is not useful.

Headline numbers on the routing path itself (M1 Pro): 90 µs at 100 variants, 11 ms at 10 000 variants, no GC pauses.

Pairs naturally with vLLM / SGLang / TGI. Drop in as an OpenAI-compatible proxy and you're done.

Looking for people who:
- run multiple inference pods and would benefit from prefix-cache awareness
- are doing P/D split and have feedback on the dispatch model
- have benchmarks of other routers (Envoy AI Gateway, NVIDIA's router, anything) I should compare against

Repo + setup: https://github.com/jiusanzhou/prism

---

## r/MachineLearning  ([P] flair)

**Title:** `[P] Prism – an open-source LLM router with inference-aware scheduling (Rust)`

**Body:**

Sharing a project I just open-sourced: [Prism](https://github.com/jiusanzhou/prism), an LLM routing engine in Rust.

What makes it different from the existing routers (LiteLLM, OpenRouter):

1. **Routing decisions are µs-scale, not ms-scale.** ~90 µs at 100 variants — fast enough that adding a router doesn't visibly affect end-to-end latency.
2. **Inference-aware scheduling for self-hosted clusters.** Prefix-cache affinity, KV-cache utilisation filtering, P/D split. Most routers treat your inference servers as opaque endpoints.
3. **Strategy abstraction.** Per-request you can ask for `balanced` (weighted sum), `cost_first`, `slo_first` (with latency constraint), or `quality_first`. Custom weights via headers.
4. **Outcome feedback loop.** POST `/v1/outcomes` after a request and Prism updates per-variant scores based on real-world latency / success / user rating.

Specifically *not* trying to be:
- A model gateway with built-in finetuning (that's a different product)
- A prompt management system (you keep your prompts; Prism routes them)

Repo: https://github.com/jiusanzhou/prism

Happy to discuss the routing pipeline (filter plugins → scorer plugins → strategy combination), the analyzer (lightweight prompt classification for specialty routing), or anything else.

---

## r/devops (mid-cycle, not launch day)

**Title:** `Prism – a self-hosted LLM proxy you can drop in for OpenAI/Anthropic/self-hosted models`

**Body:**

If your team is using multiple LLM providers, you probably have one of these scattered around your repos:
- a hand-rolled "if model.startswith('gpt') → openai_client else anthropic_client"
- a Python wrapper that adds 2 ms per request
- a vendor lock-in into one provider

[Prism](https://github.com/jiusanzhou/prism) (Rust, MIT) is a self-hosted proxy that consolidates this. Drop-in OpenAI-compatible API, but underneath:

- Routes to whichever (provider, account) you've configured based on a strategy you pick
- Per-team quotas (RPM / TPM / TPD / concurrent / model ACL) with header-based identity
- Distributed rate limiting (Redis Lua sliding window, with local-fallback degradation policy)
- Health checks with circuit breakers and 5xx feedback
- Hot config reload (no restarts)
- Prometheus metrics on 6 dimensions (model / provider / endpoint / account / region / variant)
- Helm chart, Docker image, full TLS support

If you operate the LLM layer for your org, this might save you a few hundred lines of glue and a couple of pager alerts.

Repo: https://github.com/jiusanzhou/prism
