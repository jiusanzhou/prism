---
name: 🐛 Bug Report
about: Something isn't working as documented
title: "[Bug] "
labels: bug
---

## What happened?

<!-- A clear, one-paragraph description of the bug. -->

## Reproduction

<!-- Minimal config + commands. Smaller is better. -->

```yaml
# Your prism.yaml (redact API keys)
```

```bash
# How you started Prism and triggered the bug
```

## Expected vs. Actual

- **Expected:** …
- **Actual:** …

## Environment

- Prism version / commit: `cargo run -- --version` or `git rev-parse --short HEAD`
- OS: <!-- e.g., Ubuntu 22.04, macOS 14.5 -->
- Rust version: `rustc --version`
- Deployment: <!-- standalone / Docker / Kubernetes -->
- Features enabled: <!-- e.g., proxy, grpc, wasm, redis -->

## Logs

<details>
<summary>Relevant log output (RUST_LOG=debug if possible)</summary>

```
paste logs here
```

</details>

## Additional context

<!-- Workarounds tried, related issues, anything else. -->
