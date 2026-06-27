# Pre-Launch Checklist

Do not click "Make public" / "Submit to HN" until **every** unchecked box is green. Missing one of these will cost more than the launch day delay.

## 24h Before — Repo Hygiene

### Must-have files (at repo root)
- [ ] `README.md` — Quick Start runs in <30 seconds end-to-end (timed with a stopwatch)
- [ ] `LICENSE`
- [ ] `CONTRIBUTING.md`
- [ ] `CODE_OF_CONDUCT.md`
- [ ] `.github/ISSUE_TEMPLATE/{bug_report,feature_request,question}.md` + `config.yml`
- [ ] `.github/PULL_REQUEST_TEMPLATE.md`
- [ ] `docs/DESIGN.md` linked from README
- [ ] `CHANGELOG.md` with at least `v0.1.x` entries

### Quick Start sanity
- [ ] `docker compose up -d` works on a clean machine (test in a fresh VM/container)
- [ ] `curl http://localhost:8081/v1/chat/completions -d '...'` returns 200 with a real provider key
- [ ] First-time clone → `cargo build --release` completes without nightly features or external setup
- [ ] `cargo test --workspace` passes on macOS *and* Linux
- [ ] CI badge in README is green (not stale, not red)

### GitHub repo settings
- [ ] Repo description: tight one-liner ("High-performance LLM routing engine — Rust, 90µs decisions, inference-aware")
- [ ] Topics: `rust`, `llm`, `proxy`, `routing`, `openai`, `anthropic`, `vllm`, `sglang`, `wasm`, `pingora`
- [ ] Homepage URL set (docs site or landing page if any)
- [ ] **Discussions enabled** (Categories: Announcements / Q&A / Ideas / Show and Tell)
- [ ] **Issues enabled**, templates verified
- [ ] **Releases**: at least `v0.1.0` tagged, with binary attached and release notes
- [ ] **Packages**: Docker image pushed to `ghcr.io/jiusanzhou/prism:0.1.x` *and* `:latest`
- [ ] **Sponsor button** (optional): GitHub Sponsors / OpenCollective / Ko-fi
- [ ] **Vulnerability alerts** ticked (Dependabot)

### Issue triage prep
- [ ] At least **5 issues** tagged `good first issue` with clear scope + acceptance criteria
- [ ] At least **3 issues** tagged `help wanted` for medium-effort contributions
- [ ] Roadmap issue pinned: "v0.2 — what's coming"

---

## Launch Day — Hour-by-hour

### T-3h
- [ ] Final README polish — read it like a stranger would
- [ ] Have the Show HN draft open in a tab
- [ ] Phone on Do Not Disturb except GitHub + HN notifications
- [ ] Coffee 🫖

### T-0 (Tue/Wed 9 AM ET = 9 PM Beijing)
- [ ] Submit Show HN with **exact** title from `SHOW_HN.md`
- [ ] Tweet the same minute (one short tweet, link to HN, not GitHub — drive traffic *through* HN)
- [ ] Pin GitHub repo to your profile
- [ ] Post in any private Slack/Discord communities where it's appropriate

### T+15min
- [ ] Reply to first commenter, friendly, substantive
- [ ] If story hits front page → calm down, breathe, **don't reply to trolls**

### T+1h, +2h, +4h
- [ ] Reply to every new comment within ~30 min
- [ ] File every concrete feedback as a GitHub issue, credit the HN handle in the issue body
- [ ] If something's broken, hotfix to `main` and reply on HN within the same hour

### T+24h
- [ ] Post the "what worked, what's next" follow-up comment on the HN thread
- [ ] Move to Reddit (r/rust first, see [REDDIT_POSTS.md](./REDDIT_POSTS.md))

---

## Anti-checklist — Things that will burn the launch

- ❌ Posting on Monday (people are catching up) or Friday (people are leaving)
- ❌ Title with hype words: "revolutionary", "next-gen", "AI-powered" — HN downvotes these on reflex
- ❌ Arguing with critics. State your trade-off, link to docs, move on.
- ❌ Replying with "this is in the README" — copy the relevant sentence into your reply, save them the click
- ❌ Asking for upvotes. Anywhere. Ever. (Bans your story instantly on HN.)
- ❌ Cross-posting identical text to multiple subreddits same day → spam filter

---

## After-launch (Week 1)

- [ ] Day 2: Reddit r/rust
- [ ] Day 3: Reddit r/LocalLLaMA
- [ ] Day 4: Twitter retrospective thread + Discussions "What we heard at launch" post
- [ ] Day 5: This Week in Rust PR
- [ ] Day 7: 1 blog post recapping launch + what's next (long-tail SEO)

## Metrics to track (don't obsess)

| Metric | Where | Reasonable target Week 1 |
|---|---|---|
| GitHub stars | repo | 200–1 000 |
| HN points | hn.algolia.com | 100+ = front page ~hour |
| Unique cloners | repo Insights / Traffic | 100+ |
| Docker pulls | ghcr.io | track baseline only |
| Issues filed | repo | 10–30 (sign of real engagement) |
| Discussions started | repo | 3–10 |
| Newsletter signups (if any) | wherever you set it up | 50–200 |

**The single most important number:** *number of issues filed with concrete reproduction steps*. That's "people are actually trying this", which is the only metric that matters.
