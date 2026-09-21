---
id: de4rw
title: "Copy the file and web tools into curiosity: read, write, edit, grep, paths, truncate, web"
status: open
priority: P2
created: "2026-09-21T12:18:33.210714Z"
updated: "2026-09-21T20:26:14.838451270Z"
tags:
  - curiosity
  - tools
depends_on:
  - pmm8u
parent: w9nsq
---

## Summary
The tools that touch files and the web, and the `Tool`/`ToolSet`/`ToolContext` machinery they implement. Source: `../midgaard/src/tool.rs` and `src/tools/` at `0684a87`.

## Acceptance criteria
- [ ] Copied with their tests: `tool.rs`, `tools/{mod,paths,truncate,read,write,edit,grep,web}.rs`.
- [ ] `tool.rs` loses its `session` coupling: a `ToolContext` carries the working directory, the event emitter and the cancellation token and nothing about roles, tasks or worktrees. `ToolOutput::ending` stays only if something in Curiosity can end a session from a tool; otherwise it goes.
- [ ] Path confinement (`paths.rs`) is relative to the working directory the agent was started in (`/session/work` in a Mars container). Decide and document whether leaving it is refused or allowed: in Mars the container is the boundary (ADR 0012) and agents legitimately read shared directories mounted elsewhere, so the default is **allow with absolute paths**, confinement being opt-in.
- [ ] `web.rs` keeps html-to-markdown; a fetch failure is a tool error the model sees, never a process error.
- [ ] Not copied: `tools/task.rs` (Bears), `tools/browser.rs` (chromiumoxide; backlog).

## Implementation notes
- Edges: every tool depends on `tool` and `tools::{paths,truncate}` only; `tool.rs` depends on `bus`, `event`, `session`, `tools`.
- Tool names and JSON schemas are what the model was evaluated with (GLM-5.3-flash over OpenRouter); do not rename them in passing.

## Testing
- The copied tests pass against `tempfile` directories. Quality chain passes.