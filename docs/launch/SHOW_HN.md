# Show HN: Prism — A Rust LLM router with 90µs routing decisions and inference-aware scheduling

> **Posting target:** Hacker News `Show HN` — best slot Tue/Wed 9 AM ET.
> **Title (≤80 chars):** `Show HN: Prism – Rust LLM router with 90µs routing and vLLM/SGLang awareness`

---

## Body

Hi HN — I'm Zoe. Over the last few months I've been building [**Prism**](https://github.com/jiusanzhou/prism), an LLM routing engine in Rust, and today is its public launch.

**The problem.** Most teams running multiple LLMs (commercial APIs + self-hosted vLLM/SGLang) end up writing the same glue code: "route easy questions to the cheap model", "fail over when one provider rate-limits", "split traffic for A/B tests", "respect KV-cache locality on the inference cluster". The existing options either add 1–5 ms of Python overhead per request (LiteLLM) or are hosted-only (OpenRouter). Neither understands what's happening *inside* your inference servers.

**What Prism does.**

- Picks the optimal `(model, provider, account, variant)` for each request in **~90 µs** at 100 variants, linear scaling (benchmarks in repo).
- Drop-in: exposes an OpenAI-compatible reverse proxy on port 8081 (Pingora-based). Change one URL, you're done.
- Inference-aware for self-hosted clusters: prefix-cache affinity via consistent hashing, KV-cache utilisation filtering, prefill/decode split for disaggregated serving.
- 4 routing strategies out of the box (balanced / cost-first / SLO-first / quality-first) + 18 built-in filter/scorer plugins.
- 5-dimension WASM plugin system (Config / Discovery / Route / Proxy / Analyze) — extend any phase without recompiling Prism.
- Single Rust binary. HTTP + gRPC + reverse proxy from one process, each toggleable via feature flag.

**Numbers** (M1 Pro, full numbers in [`README#performance`](https://github.com/jiusanzhou/prism#performance)):

| Scenario | Latency |
|---|---|
| Route 100 variants | 90 µs |
| Route 1 000 variants | 900 µs |
| Route 10 000 variants | 11 ms |
| Config hot-reload load (per-request) | 9.8 ns (arc-swap) |
| 6-dim metric record | 1.7 µs |

**What's already working in production.** We've been running Prism in front of an internal LLM gateway and a 15-pod K8s vLLM cluster (auto-discovered via a Go sidecar) for the past few months. ~1 800 tests in CI, Docker image, Helm chart on the way.

**Why I built it.** I got tired of `litellm.completion()` adding more latency than the local network round-trip to the inference server, and tired of writing the same retry/affinity/load-balancing logic for every project. Prism is what I wished existed.

**What's MIT-licensed and forever free.** Everything in the repo. A hosted control plane is in the works for teams that don't want to run it themselves — but the router itself stays open source.

**What I'd love feedback on:**
1. Is the strategy abstraction (`balanced` / `cost_first` / `slo_first` / `quality_first` + custom weights) the right shape, or should it be more declarative ("optimise for X subject to constraint Y")?
2. The WASM plugin ABI — does it cover what you'd want to extend?
3. Any obvious benchmark scenarios I'm missing? I'd love comparisons against Envoy AI Gateway, NVIDIA Triton's router, anything in this space.

Repo + getting started in 30 seconds: https://github.com/jiusanzhou/prism

Happy to dive into any part of the implementation — the routing pipeline, the Pingora integration (which was non-trivial), the WASM host, the K8s discovery sidecar, the prefix-cache index. AMA.

---

## Response playbook (24 hours after posting)

1. **First 15 minutes:** reply to top 3 comments personally. Even "good question, here's the trade-off…" is better than nothing.
2. **Don't argue.** If someone says "this is just LiteLLM in Rust" — agree on the surface area, then point to the concrete differentiator (inference awareness, latency).
3. **Capture every concrete feedback** as a GitHub issue with the user's HN handle credited. Then reply on HN: *"Filed as #N — thanks."* This converts an HN drive-by into a tracked promise.
4. **Don't pin replies to anything snarky.** It always backfires on HN.
5. **End-of-day summary:** post one update comment with "Top feedback so far: X, Y, Z — here's what I'm doing about each."

---

## Title variants to A/B mentally before posting

- `Show HN: Prism – Rust LLM router with 90µs routing and vLLM/SGLang awareness` ✅ recommended
- `Show HN: Prism – I rewrote LiteLLM in Rust because Python added 3ms per request` — punchier but invites a flamewar
- `Show HN: A Rust-based LLM router that understands what's happening inside your inference cluster` — descriptive but long
