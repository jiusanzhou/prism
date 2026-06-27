# Prism — Launch Kit

Materials for going public: Show HN, Reddit, blog outreach, Discord seeding.

Files in this directory:

- [`SHOW_HN.md`](./SHOW_HN.md) — Hacker News "Show HN" post draft
- [`REDDIT_POSTS.md`](./REDDIT_POSTS.md) — Per-subreddit tailored drafts (r/rust, r/LocalLLaMA, r/MachineLearning, r/devops)
- [`BLOGGER_OUTREACH.md`](./BLOGGER_OUTREACH.md) — Cold email template + target list
- [`LAUNCH_CHECKLIST.md`](./LAUNCH_CHECKLIST.md) — Pre-launch sanity check (24h SLA prep, Issues triage rotation, etc.)

## Strategy in one paragraph

Prism is a Rust LLM router. Our hook is **µs-level routing latency + inference-aware scheduling for self-hosted vLLM/SGLang clusters** — neither LiteLLM nor OpenRouter has the latter. Target audiences, in order: (1) infra engineers running their own LLM stacks (highest pain, highest ROI), (2) platform teams aggregating commercial APIs, (3) Rust enthusiasts. We **don't lead with "alternative to LiteLLM"** — we lead with a concrete problem ("we cut routing overhead from 3 ms to 90 µs and added prefix-cache-aware scheduling on top of vLLM").

## Sequence

1. **T-7 days:** README polished, Discussions enabled, 5+ `good first issue` tagged, Discord (or use Discussions), v0.1.x release with Docker image.
2. **T-3 days:** Soft-seed in 2–3 small communities (r/rust, personal Twitter). Listen for objections, fix top complaints.
3. **T-0 (Tuesday/Wednesday, 9 AM ET):** Show HN.
4. **T+1 day:** Reddit posts staggered across r/LocalLLaMA, r/devops.
5. **T+3 days:** Blogger outreach (5 personalized emails — quality, not volume).
6. **T+7 days:** Retrospective post (what worked, real user feedback, what's next).
