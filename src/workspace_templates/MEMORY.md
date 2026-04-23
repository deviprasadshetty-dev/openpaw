
# MEMORY.md - What You Know About This Place

_This is your working memory for facts about the environment, project conventions, and recurring patterns. Keep it concise and actionable._

## What Goes Here

- **Project conventions** — naming patterns, folder structure, tech stack
- **Environment facts** — OS quirks, installed tools, path conventions
- **Recurring pitfalls** — errors you hit before and how you solved them
- **Workflow patterns** — "deploy via script X", "test with command Y"
- **Codebase rules** — linting config, test patterns, review norms

## What Does NOT Go Here

- Transient details ("today I fixed bug #123" — that goes in skills or goals)
- User preferences (those belong in `USER.md`)
- Raw conversation logs

## Bounded Storage

This file has a strict size limit. When it gets full, you must **consolidate and compress** — merge related entries, drop stale facts, and rewrite summaries to be denser. Never let it grow unbounded.

**Self-pruning rules:**
1. If two entries say similar things, merge them into one tighter sentence
2. If a fact hasn't been relevant in 30+ days, drop it
3. Prefer patterns over instances ("use `cargo test`" beats "on 2024-03-15 I ran `cargo test`")

## Format

Use short sections. Bullet points over paragraphs. Each line should be a signal, not a story.

```markdown
## Project: OpenPaw

- Rust workspace, cargo-based
- Tests: `cargo test --workspace`
- Lint: clippy + rustfmt (enforced in CI)
- Key crates: tokio, serde, axum

## Environment

- OS: Windows 11 / WSL2
- Shell: PowerShell + nushell
- Node: v20 (for frontend builds)
```

---

_This file is living documentation. Update it when you learn something worth keeping. Compress it when it grows too long._
