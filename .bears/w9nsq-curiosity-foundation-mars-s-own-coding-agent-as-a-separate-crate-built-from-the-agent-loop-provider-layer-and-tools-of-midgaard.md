---
id: w9nsq
title: "Curiosity foundation: Mars's own coding agent as a separate crate, built from the agent loop, provider layer and tools of midgaard"
type: epic
status: open
priority: P2
created: "2026-09-21T12:12:07.368539Z"
updated: "2026-09-21T20:26:14.534019620Z"
tags:
  - curiosity
  - agent
---

## Scope
Curiosity is the coding agent Mars ships beside Claude Code: a single static Rust binary that runs inside a session container and reaches models through `rig` (OpenRouter first). It starts as a **copy** of selected modules of `../midgaard` (MIT, same author; Mars supersedes midgaard, so there is no dependency between the projects and the copy is free to diverge). It lives in a new top-level crate `curiosity/` that is **not** a workspace member of `orchestrator/`: the wire protocol is the whole contract, no types are shared, and rig's dependency tree stays out of the orchestrator build and its `SQLX_OFFLINE` setup.

This epic ends with an agent that runs headless, one prompt per process, against OpenRouter or the mock provider. ACP, MCP and persistence are the next epic.

## What is copied and what stays behind
Taken: `agent.rs`, `model.rs`, `model/mock.rs`, `event.rs`, `bus.rs` (without `history`/`runtime`), `tool.rs` (without `session`), `tools/{read,write,edit,grep,paths,truncate,web,bash}.rs`, `procs.rs` (`bash` without `role`/`worktree`).
Left behind: `fleet`, `tui`, `store` and the Bears task tool, `worktree`, `role` and the minijinja prompts, `runtime`, `history`, `decision`, `bug`, `artifact`, `init`, `tools/browser.rs`.

## Acceptance criteria
- [ ] `curiosity/` builds, is `cargo fmt` and `cargo clippy --all-targets -- -D warnings` clean, and has its own CI workflow; `CLAUDE.md` names its scope and quality chain.
- [ ] `curiosity run` answers a prompt with tools in a working directory, against OpenRouter (`OPENROUTER_API_KEY`) and against the mock provider, writing its events as JSON lines.
- [ ] No reference to midgaard, Bears, roles, worktrees or the TUI remains in the crate.
- [ ] The copied tests pass in their new home.