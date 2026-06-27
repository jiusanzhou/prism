# Blogger / Newsletter Outreach

Cold-email template + target list. Same logic as the cold email episode: **find the right person, write a personal message, say clearly why their readers care.**

## Template

> **Subject:** `Quick share — open-source Rust LLM router (Prism), might fit your readers`
>
> Hi {{FIRST_NAME}},
>
> I read your post on **{{SPECIFIC_RECENT_POST}}** — {{ONE_GENUINE_SENTENCE_OF_REACTION}}. The bit about {{SPECIFIC_DETAIL}} resonated because we hit the same wall.
>
> I just open-sourced **Prism** ([github.com/jiusanzhou/prism](https://github.com/jiusanzhou/prism)) — an LLM routing engine in Rust. It's similar in spirit to LiteLLM, but with two angles that don't exist elsewhere:
>
> 1. **µs-level routing decisions.** ~90 µs at 100 variants (vs. 1–5 ms in Python routers), thanks to a Rust core + `arc-swap` config + lock-free atomics.
> 2. **Inference-aware scheduling.** First-class prefix-cache affinity, KV-cache utilisation filtering, and P/D-split support for self-hosted vLLM/SGLang clusters — most routers treat your inference servers as opaque endpoints.
>
> If you're considering a piece on **{{TOPIC_THAT_FITS}}**, I'd love to send you:
> - a 5-minute Docker-compose demo (single command, both proxy and a mock vLLM)
> - the benchmark methodology + raw numbers vs. LiteLLM
> - a 15-min call walking through the architecture
>
> No expectation — just thought it might be useful. Happy to answer anything from your readers' angle.
>
> Cheers,
> Zoe
> Maintainer, Prism
> {{TWITTER_OR_GITHUB_HANDLE}}

**Rules:**

- `{{SPECIFIC_RECENT_POST}}` must be **a real, recent post you actually read**. If you can't fill it in, don't send the email.
- One email per person, ever. Don't follow up after 1 week — they saw it.
- Personalize at least the first two sentences. The rest can be templated.

---

## Target list (build this before launch)

Format: `Name | Outlet | Why they fit | Recent post to reference | Email`

### Top priority — write personally
- [ ] Simon Willison (simonwillison.net) — LLM tooling, broad reach, posts working code. Reference: any of his recent llm CLI posts.
- [ ] Swyx (latent.space) — AI Engineer newsletter, infra-curious audience.
- [ ] Pamela Fox (blog.pamelafox.org) — Azure/AI dev tools, reads project READMEs carefully.
- [ ] Eugene Yan (eugeneyan.com) — applied LLM systems, more research-y but reads.
- [ ] Chip Huyen (huyenchip.com) — ML systems, "Machine Learning Systems Design" author.

### Newsletters (different format — pitch the editor)
- [ ] **TLDR AI** — submit at tldr.tech/tips
- [ ] **The Sequence** — Jesus Rodriguez
- [ ] **AlphaSignal** — Lior Sinclair
- [ ] **Latent Space** — Swyx (above)
- [ ] **Ben's Bites** — bensbites.com
- [ ] **The Rundown** — rowan.so
- [ ] **Last Week in AI** — Andrey Kurenkov

### Rust-focused
- [ ] **This Week in Rust** — *just submit via PR*: https://github.com/rust-lang/this-week-in-rust → an extremely high-leverage 30-minute task. Categories: "Crate of the Week" candidate or "Newsletters from Rust Community".
- [ ] **Filtra.io's Rust newsletter** — rustweekly.com
- [ ] **Console** — console.dev (devtool newsletter, ~70k readers)

### LLM infra blogs
- [ ] **Modal Labs blog** — they cover infra-adjacent work
- [ ] **Anyscale blog** — similar
- [ ] **vLLM project blog** — if Prism integration is meaningful, an integration writeup co-authored could be huge

---

## Realistic conversion

- 10 emails sent → expect **1–2 responses**, of which 0–1 turn into coverage.
- Newsletter submissions → response rate is "they include it or they don't"; no human-loop expected.
- This Week in Rust → near-certain pickup if PR is well-formed (it's a community newsletter).

The point of this outreach is **trust transfer**, not volume. One mention in Simon Willison's blog > 100 Reddit upvotes.
