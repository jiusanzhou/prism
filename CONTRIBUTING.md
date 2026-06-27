# Contributing to Prism

Thanks for considering a contribution. Prism is a community-driven project — every Issue, PR, and benchmark report makes it better.

## TL;DR

- **Bug?** → [Open an issue](https://github.com/jiusanzhou/prism/issues/new?template=bug_report.md) with reproduction steps.
- **Feature idea?** → [Start a discussion](https://github.com/jiusanzhou/prism/discussions) first, then file an issue.
- **First-time contributor?** → Look for issues tagged [`good first issue`](https://github.com/jiusanzhou/prism/labels/good%20first%20issue).
- **Maintainer response SLA:** We aim to acknowledge every issue within **24 hours**.

---

## Ways to Contribute

| Type | Effort | Where to Start |
|---|---|---|
| 🐛 Bug report | 5 min | [Bug template](.github/ISSUE_TEMPLATE/bug_report.md) |
| 📝 Docs fix (typo, clarification) | 10 min | Edit on GitHub → PR directly |
| ⭐ Star + share | 1 min | Tweet, blog, or link in your project's README |
| 🧪 Benchmark on your setup | 1 hr | Run [`benches/`](./benches/) and post results |
| 🔌 New plugin (filter/scorer/codec) | half day | See [`plugins/PLUGIN_DEV.md`](./docs/PLUGIN_DEV.md) |
| 🌐 New provider integration | half day | See `prism-router/src/proxy/providers/` |
| 🔧 Core feature | 1+ day | Discuss first in an issue |

---

## Development Setup

```bash
# Prerequisites: Rust 1.82+
git clone https://github.com/jiusanzhou/prism.git
cd prism

# Build everything
cargo build --workspace

# Run the full test suite (1800+ tests)
cargo test --workspace

# Run with all features (WASM, Redis, gRPC, proxy, usage-pg)
cargo test --workspace --all-features

# Format + lint before pushing
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

---

## Submitting a Pull Request

1. **Fork** the repo and create a topic branch off `main`:
   ```bash
   git checkout -b feat/your-feature-name
   # or fix/your-bug-fix, docs/your-doc-update
   ```
2. **Write tests** for behavior changes. Prism's CI runs the whole workspace; failing tests block merge.
3. **Keep PRs focused.** One concern per PR makes review faster.
4. **Conventional commit messages** preferred:
   - `feat: add cohere provider integration`
   - `fix(proxy): handle empty SSE chunks`
   - `docs: clarify rate-limit override priority`
   - `test: cover circuit breaker half-open state`
5. **Open the PR** with:
   - What the change does
   - Why it's needed (link issue if any)
   - How you verified it (tests added, benchmark numbers, screenshots)

---

## Code Style

- **Rust:** `cargo fmt` + `cargo clippy -- -D warnings`. No exceptions in `main`.
- **Errors:** Prefer typed errors (`thiserror`) over `anyhow` in library code. `anyhow` is fine in binaries and tests.
- **Async:** Use `tokio` runtime primitives. No blocking I/O on async paths.
- **Logging:** `tracing` crate. Use structured fields, not formatted strings.
- **Tests:** Unit tests live alongside code (`#[cfg(test)]`). Integration tests in `tests/`. E2E in `tests/e2e/`.

---

## Architecture Pointers

| You want to change... | Look here |
|---|---|
| Routing logic (filters/scorers) | `prism-core/src/plugins/` |
| Strategy evaluation | `prism-core/src/strategy/` |
| Reverse proxy behavior | `prism-router/src/proxy/` |
| HTTP API | `prism-router/src/api/` |
| Config sources | `prism-router/src/config/sources/` |
| WASM host integration | `prism-router/src/wasm/` (feature-gated) |
| Discovery (Static/WASM/K8s) | `prism-router/src/discovery/` |
| Metrics | `prism-router/src/metrics.rs` |

See [`docs/DESIGN.md`](./docs/DESIGN.md) for the full architecture write-up.

---

## Plugin Development

Prism has a 5-dimension WASM plugin system (Config / Discovery / Route / Proxy / Analyze). A single WASM module can participate in multiple phases.

- **Native Rust plugin:** implement the relevant trait (e.g., `RoutingPlugin`) and register via `PluginRegistry`.
- **WASM plugin:** see `examples/wasm-plugins/` for templates, especially `openai-compat/` for a working end-to-end example.

If you build a community plugin, open a PR adding it to the [Community Plugins](./docs/PLUGINS.md) list.

---

## Reporting Security Issues

**Please do not open public issues for security vulnerabilities.** Email `security@zoe.im` (or DM the maintainer on GitHub / Telegram `@noboddyim`) with details, and we'll respond within 48 hours.

---

## Code of Conduct

This project follows the [Contributor Covenant](./CODE_OF_CONDUCT.md). By participating you agree to uphold it.

---

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](./LICENSE) that covers the project.
