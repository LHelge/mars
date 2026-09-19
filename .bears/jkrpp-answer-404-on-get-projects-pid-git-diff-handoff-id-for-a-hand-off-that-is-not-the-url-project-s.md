---
id: jkrpp
title: Answer 404 on GET /projects/{pid}/git/diff?handoff_id= for a hand-off that is not the URL project's
status: done
priority: P2
created: "2026-09-18T02:39:21.825207561Z"
updated: "2026-09-19T23:34:22.165415791Z"
tags:
  - orchestrator
  - git
  - tracker
depends_on:
  - j2b83
  - dgxzq
parent: xjaah
---

## Summary
`SPEC.md` "Git" says a `handoff_id` on the diff endpoint "must belong to the URL project" and the git routes task (j2b83) specifies 404 for one that does not. The git epic shipped without `task_handoffs` access: `GitService::diff` with `DiffSelector::Handoff(id)` resolves `refs/handoffs/<id>` in the project's mirror and answers 400 `UnknownRef` when the ref is missing, which is the right answer for an unknown id but cannot tell a hand-off of another project from a nonexistent one. `tests/git_routes.rs` currently asserts "unknown hand-off id -> 400".

## Documents
- `SPEC.md` "Git (`/api/projects/{pid}/git`)": "a hand-off must belong to the URL project and selects its immutable commit without fetch-back".

## Acceptance criteria
- [ ] Before resolving the ref, `GitService::diff` (or the route) looks the hand-off up through the `HandoffVerifier` / `task_handoffs` and answers `Error::NotFound` when no row with that id belongs to a task of `project_id`; an id with a row in the project but no ref (interrupted publication) stays 400 `UnknownRef`.
- [ ] `tests/git_routes.rs` gains "hand-off of another project -> 404" and keeps "unknown id -> 400" (or switches it to 404 if the lookup makes every unknown id a 404; say which in `SPEC.md`).
- [ ] `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- The lookup needs the `TaskHandoffVerifier` from dgxzq (or a small `find_handoff_in_project` on the hand-offs repository); add a `handoff_in_project(project_id, handoff_id) -> Result<bool>` method to the `HandoffVerifier` trait with `NoHandoffs` answering `false`, so the git epic's unit tests keep compiling.
