---
id: yefdk
title: "Fold the shallow tracker files: comments.rs and the links.rs reads into their aggregates, task_session row type private to the repository"
status: in_progress
priority: P3
created: "2026-09-17T20:02:11.431896282Z"
updated: "2026-09-21T02:24:59.851871808Z"
tags:
  - orchestrator
  - tracker
  - architecture
depends_on:
  - tepsh
parent: kg8cx
attempts: 1
---

## Summary
Apply the deletion test to the file boundaries in `repositories/tasks/` and `models/`. `comments.rs` (two methods, validation in the model, scope check delegated) and the two read queries in `links.rs` are indistinguishable from the board reads in `rows.rs`; `models/task_session.rs` is a `FromRow` target with no behaviour and says so. Fold them so that a reader of "a task" finds its comments, sessions and hand-offs in one place, and keep `touch_task_session` (the one ADR 0030 write) where the mutation commits.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals" (module layout: `models/` domain types + validation, `repositories/` one struct per aggregate).
- ADR 0030.

## Acceptance criteria
- [ ] `repositories/tasks/comments.rs` and the read half of `links.rs` are merged into the file that owns task reads (`rows.rs` or a new `reads.rs`); `touch_task_session` stays beside `append_task_events` in the commit path.
- [ ] `models/task_session.rs` becomes a private row type in the repository, or the DTO from the tracker epic (`TaskSessionLinkDto`) replaces it; `models/mod.rs` stops re-exporting it.
- [ ] `models/mod.rs` remains the module list; no other change to the barrel.
- [ ] Nothing in `SPEC.md` or `docs/data-model.md` changes (tables and endpoints are untouched).

## Implementation notes
- Files: `orchestrator/src/repositories/tasks/{mod,rows,links,comments}.rs`, `orchestrator/src/models/{mod,task_session}.rs`.
- Small, mechanical; do it after `tepsh` so the signatures are settled.

## Testing
- Existing tests keep passing after import changes.
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- None: the documented layout (`models/`, `repositories/`) is unchanged.