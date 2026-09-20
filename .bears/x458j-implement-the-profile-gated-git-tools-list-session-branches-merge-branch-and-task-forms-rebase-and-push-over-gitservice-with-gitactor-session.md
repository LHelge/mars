---
id: x458j
title: "Implement the profile-gated git tools list_session_branches, merge (branch and task forms), rebase and push over GitService with GitActor::Session"
status: done
priority: P1
created: "2026-09-16T20:45:49.832206573Z"
updated: "2026-09-20T08:19:06.482491159Z"
tags:
  - orchestrator
  - mcp
  - git
depends_on:
  - h9u3c
parent: qgj33
attempts: 1
---

## Summary
Implement the four git tools as thin adapters over the git epic's `GitService`, attributed to the calling session. `list_session_branches` is a read with no event; `merge` accepts the branch form and the task form (`task_id` by UUID or number, verified through the hand-off epic's `HandoffVerifier`); `rebase` rewrites a session or integration branch onto an integration or upstream ref and, for the caller's own branch, reconciles the work tree when clean; `push` sends one ref upstream, treating non-fast-forward as `conflict` and adding `--force` only when `force: true`. Profile gating is already enforced by the dispatcher; these handlers assume the tool is allowed.

## Documents
- `SPEC.md` "MCP tool contracts" → `list_session_branches` (`{}` → `{ branches: SessionBranch[] }`), `merge` (`MergeInput` as REST with `task_id` also accepting a number; `{ commit }` or `conflict` with `data.conflicts`; task form rejects stale or unapproved hand-offs and uses the pinned commit; branch form: `origin/main` permitted as source, target must be an integration head, session refs synced first), `rebase` (`{ branch, onto }` → `{ commit }` or `conflict`; an upstream ref may be `onto`, never `branch`; caller's own branch → work tree updated when clean), `push` (`{ ref, remote_branch?, force?: false }` → `{ remote_branch, commit }`; only integration heads or session refs; non-fast-forward → `conflict` without changing local refs; force requires `force: true` and the profile's `push`; session refs pushed as `refs/heads/session/<id>` by default), intro (`list_session_branches` emits no `git` event; git operations retain their outcome events).
- `SPEC.md` "Git" (`SessionBranch` shape, ref-name rules, `MergeInput` alternatives mutually exclusive, 409/422 rules that map to `conflict`).
- `ARCHITECTURE.md` "Git model" → "Merge, rebase, push", "Serialization" (git lock before any database lock; never inside a tracker transaction), "MCP design" → "Side effects"; ADR 0007 (one code path for humans and agents; every remote write attributed), ADR 0018 (task merge uses the pinned approved commit).
- `SPEC.md` "AgentEvent" (`git` event `op`, `ok`, `detail`).

## Acceptance criteria
- [ ] `list_session_branches::handle` → `GitService::list_session_branches(ctx.project_id)` → `BranchesOutput`; no event is written (asserted).
- [ ] `merge::handle`: exactly one of `source` or (`task_id` and `handoff_id`) must be present, else `invalid_argument` `merge takes either source or task_id and handoff_id`; `message` longer than 10 KiB → `invalid_argument`; branch form → `GitService::merge_branch(project_id, source, target, message, GitActor::Session(ctx.session_id))`; task form → resolve `task_id` via `TaskRepository::resolve(project_id, TaskRef)` (`not_found` "task not found") then `GitService::merge_handoff(project_id, task_uuid, handoff_id, target, message, actor)`; output `CommitOutput { commit }`; a merge conflict → `conflict` with `data.conflicts` (paths in git's order); stale or unapproved hand-off → `conflict` with the verifier's message; bad ref kinds → `invalid_argument`.
- [ ] `rebase::handle` → `GitService::rebase(project_id, branch, onto, actor)` → `CommitOutput`; conflicts → `conflict` with `data.conflicts`; `branch` naming an upstream ref or `onto` naming a session ref → `invalid_argument`; when `branch` is the caller's session, the service's work-tree reconciliation result appears in the `git` event `detail.work_tree` (`updated` or `reconciliation_required`), and the tool output stays `{ commit }`.
- [ ] `push::handle`: `force` defaults to `false`; `GitService::push(project_id, ref, remote_branch, force, actor)` → `PushOutput { remote_branch, commit }`; non-fast-forward → `conflict` (message from the service, e.g. `push rejected: upstream has advanced`) with local refs unchanged; upstream refs or `refs/handoffs/*` as `ref` → `invalid_argument`; a session ref with no `remote_branch` pushes to `session/<id>`.
- [ ] Every mutating tool results in exactly the `git` outcome events the `GitService` contract specifies on the calling session (and on the sessions whose refs took part); the handlers add no events of their own and never open a tracker transaction.
- [ ] `tests/mcp_git_tools.rs` with real bare repositories (the git epic's test helper: temp upstream, `ready` project, session rows with work clones and commits; the caller is one of those sessions with a profile whose `mcp_tools` lists all four): `list_session_branches` returns the caller's branch with `ahead`/`behind` and writes no event; branch-form `merge` of the caller's session into `main` → `{ commit }` resolvable in the mirror and one `git` event `op: "merge", ok: true` on the caller; a conflicting merge → `conflict` with `data.conflicts` containing the path and `refs/heads/main` unchanged; `merge` with both forms or neither → `invalid_argument`; task-form merge with an approved current hand-off → merges the pinned commit even after the session branch advanced; with an unapproved or superseded hand-off → `conflict` and `main` unchanged; `task_id` given as `"#1"` works; `rebase` of the caller's branch onto `main` after `main` advanced → new commit, work tree at the new tip when clean, `detail.work_tree = "updated"`; with a dirty work tree → `reconciliation_required` and the tool still returns `{ commit }`; `rebase` with `branch: "origin/main"` → `invalid_argument`; `push` of `main` to the temp upstream → `{ remote_branch: "main", commit }` and upstream `refs/heads/main` equals it; after the upstream advances independently, `push` → `conflict` and the mirror ref unchanged; `push` with `force: true` succeeds; `push` of a session ref without `remote_branch` lands on upstream `refs/heads/session/<id>`; `push` of `origin/main` → `invalid_argument`; a profile without `push` calling `push` → `forbidden` (dispatcher regression).
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/mcp/tools/list_session_branches.rs`, `merge.rs`, `rebase.rs`, `push.rs`, `orchestrator/tests/mcp_git_tools.rs`.
- Convert `McpMergeInput` to the git epic's `MergeInput`/service arguments after resolving `task_id`; do not duplicate ref parsing — pass strings to `GitService`, which validates ref kinds and raises the 400-mapped errors that become `invalid_argument`.
- The SPEC sentence "Force pushes are refused unless `force` is true and the profile has `push` in `mcp_tools`" is satisfied by the dispatcher gate plus the `force` flag; do not add a second permission check, but keep a unit test documenting that `force` without `true` never passes `--force`.
- Git is never mocked; tests need the `git` binary and use `tempfile` directories as in the git epic.

## Edge cases
- The caller's session has no work clone yet (`creating`): the service's `sync` failure → `invalid_argument` (400-mapped) with the service's message; no event.
- Merge `message` empty string → treated as absent.
- `handoff_id` that is not a UUID → serde failure → `invalid_argument` from the dispatcher.
- Two agents merging into `main` at once serialise on the project git lock; the second merge sees the first's result.

## Testing
- Integration tests as listed; assert mirror refs with the git epic's `refs::resolve` helper and events by reading `events` rows.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Git operations: mirror, clones, integration and REST API": `GitService::{list_session_branches, merge_branch, merge_handoff, rebase, push}`, `GitActor::Session`, `git` outcome events with `detail.work_tree`, the real-repository test helper (tasks "Add GitService..." and "Add the git REST routes...").
- "Code hand-offs and review": the `HandoffVerifier` implementation behind `merge_handoff` and a way to create approved/unapproved hand-offs in tests.
- "Task tracker: states, tasks, leases, dependencies and events": `TaskRepository::resolve`.