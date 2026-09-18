---
id: j2b83
title: Add the git REST routes under /projects/{pid}/git with 400/401/409/422 contracts and tests
status: in_progress
priority: P1
created: "2026-09-16T20:33:45.045085157Z"
updated: "2026-09-18T02:14:14.749209525Z"
tags:
  - orchestrator
  - git
depends_on:
  - "8ak3d"
parent: z4u4e
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Expose the git operations to the UI as the five documented endpoints in one route module, `orchestrator/src/routes/git.rs`, with private DTOs, JWT auth, the documented statuses (422 with `conflicts` for merge/rebase conflicts, 409 for non-fast-forward pushes and for stale/unapproved task hand-offs, 400 for bad ref kinds and malformed bodies) and integration tests for the happy path and each error path. The task form of `POST .../merge` is parsed and validated here and delegated to the hand-off verification that the Code hand-offs epic wires in.

## Documents
- `SPEC.md` "Git (`/api/projects/{pid}/git`)" (the full table, `SessionBranch`, `Diff`, `MergeInput`, ref-name rules, 400 for upstream refs as targets/push sources, 409 vs 422 rules, `force: true` requirement, mutual exclusivity of `source` vs `task_id`+`handoff_id`, `head` vs `handoff_id` on diff)
- `SPEC.md` "REST API" (status table, error body shape, bare JSON)
- `CLAUDE.md` "API conventions", "Backend conventions" (one module per resource exporting `routes() -> Router<AppState>`, DTOs private)
- `ARCHITECTURE.md` "Git model" (task merge: verify current and approved under the git lock; generic merges do not record approval)
- ADR 0007

## Acceptance criteria
- [ ] `routes::git::routes() -> Router<AppState>` nested so the paths are `GET /api/projects/{pid}/git/session-branches`, `GET /api/projects/{pid}/git/diff`, `POST /api/projects/{pid}/git/merge`, `POST /api/projects/{pid}/git/rebase`, `POST /api/projects/{pid}/git/push`; all require a valid JWT (401 otherwise); unknown `pid` -> 404; project not `ready` -> 409.
- [ ] `GET .../session-branches` -> 200 `SessionBranch[]`.
- [ ] `GET .../diff?head=&base=` or `?handoff_id=&base=` -> 200 `Diff`; both or neither of `head`/`handoff_id` -> 400; `handoff_id` not belonging to the project -> 404; `base` defaults to `default_branch`; unresolvable ref or no merge base -> 400.
- [ ] `POST .../merge` body `MergeInput = { target, message? } & ({ source } | { task_id, handoff_id })`: both alternatives or neither -> 400; `target` not an integration head -> 400; success -> 200 `{ commit }`; conflict -> 422 `{ status: 422, error, conflicts: string[] }`; task form: 409 when the hand-off is not the task's current one or not `approved`, and `task_id` accepts a UUID (a task number is MCP-only).
- [ ] `POST .../rebase {branch, onto}` -> 200 `{ commit }`; 422 on conflict; 400 when `branch` is an upstream ref or `onto` is a session ref.
- [ ] `POST .../push {ref, remote_branch?, force?: false}` -> 200 `{ remote_branch, commit }`; 409 on non-fast-forward with local refs preserved; 400 for an upstream ref or an invalid `remote_branch`; `force` defaults to `false` and only `true` adds `--force`.
- [ ] All handlers pass `GitActor::User(claims.user_id)`; `message` limited to 10 KiB (400 above).
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- File: `orchestrator/src/routes/git.rs`; register in the API router (`routes/mod.rs`). DTOs: `MergeBody` deserialised with `#[serde(deny_unknown_fields)]`? No: keep lenient deserialisation but validate the alternative explicitly (`source.is_some() ^ (task_id.is_some() && handoff_id.is_some())`), returning `Error::BadRequest("merge takes either source or task_id and handoff_id")`. `DiffQuery { head: Option<String>, handoff_id: Option<Uuid>, base: Option<String> }`.
- Task form: call `GitService::merge_handoff(project_id, task_id, handoff_id, target, message, actor)`; in this epic that method acquires the lock and calls a `HandoffVerifier` hook — define `pub trait HandoffVerifier: Send + Sync { async fn approved_commit(&self, project_id, task_id, handoff_id) -> Result<ApprovedHandoff { commit, source_branch }> }` in `git/service.rs` with `Error::Conflict` for stale/unapproved and `Error::NotFound` for unknown; the Code hand-offs epic provides the implementation backed by `task_handoffs`; this task ships a `NoHandoffs` default implementation returning `Error::Conflict("task hand-offs are not available")` so the route compiles and the 400-mixing test runs.
- 409 for a project that is not `ready`: check `projects.status` before touching git (`Error::Conflict("project is not ready")`).
- Log handler entry at `debug` with `project_id = %pid` and the op; never log bodies.

## Edge cases
- `source` naming a session of another project: 400 (`SessionRepository::get(project_id, session_id)` scoping).
- `target` given as `refs/remotes/origin/main` (fully qualified upstream): 400.
- Empty `conflicts` never appears: a 422 always has at least one path.
- Concurrent merges to the same target serialise on the git lock; the second sees the first's result (the test can run two requests sequentially and assert both succeed with distinct commits).

## Testing
- `TestApp` integration tests in `orchestrator/tests/git_routes.rs` using a helper that: creates a temp upstream bare repo with `main`, inserts a `ready` project row pointing at it (`remote_url = file path is not https`: therefore insert the row with a fake `https://example.invalid/repo.git` `remote_url` and configure `origin` to the temp path directly with `git remote set-url` in the test helper, documented in the helper), runs `init_project_repo`, inserts session rows and creates work clones with commits.
- Per endpoint: happy path; 401 without token; 400 bad ref kind (`target: "origin/main"`, `branch: "origin/main"`, `ref: "origin/main"`); 404 unknown project; 409 (project `cloning` for all; non-fast-forward for push; task form with the `NoHandoffs` verifier); 422 with `conflicts` for merge and rebase; diff 400 for both/neither selectors; merge 400 for mixed forms; push `force: true` succeeds after a 409.
- Assert response shapes with `response.json::<T>()` against the `SPEC.md` types; assert `refs/heads/main` unchanged after every failure case by reading the mirror with `git::refs::resolve`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Authentication, users, invites and email": JWT extractor / `Claims` for handlers and `TestApp` helpers to obtain a bearer token.
- "Database schema, models, repositories and test harness": `ProjectRepository`, `SessionRepository` inserts for fixtures.
- "Code hand-offs and review": the real `HandoffVerifier` implementation and the task-form 409 tests against `task_handoffs` rows.
- "Projects, agent profiles and shared directories": `POST /projects` and the clone job (not needed by these tests, which seed rows directly).