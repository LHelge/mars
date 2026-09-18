---
id: "8ak3d"
title: "Add GitService: lock-scoped composite operations, session fetch-back rules, actor attribution and git outcome events"
status: done
priority: P1
created: "2026-09-16T20:33:07.410019602Z"
updated: "2026-09-18T02:18:21.142514791Z"
tags:
  - orchestrator
  - git
  - sessions
depends_on:
  - "6gvak"
  - "7u3pu"
  - "7q4qt"
  - "48hgg"
  - nzchj
  - "55pqv"
parent: z4u4e
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Compose the primitives into the operations the REST routes, the MCP tools, the session endpoints (`end`, `sync`) and the Code hand-offs epic call: each acquires the project git lock once, syncs every participating session ref first, resolves refs to fixed commits, obtains credentials and identity through `GitCredentialProvider` with the right `GitActor`, runs the primitive, and records the outcome as a `git` event on the affected session(s) through the shared event-append primitive. This is the single code path for humans and agents (ADR 0007); the routes and tools become thin.

## Documents
- `ARCHITECTURE.md` "Git model" -> "Fetch-back" (before any merge, rebase or push involving the session), "Merge, rebase, push", "Serialization" (acquire once, hold through completion; git lock before any database lock), "Diff" (internal fetch-back emits no `git` event; explicit sync retains its event; `handoff_id` never syncs), "Commit identity"
- `ARCHITECTURE.md` "Session owner task" (git operations insert `git` events through the same repository call whether or not an owner exists), "MCP design" -> "Side effects" (`list_session_branches` emits no event; git operations retain their session `git` outcome events)
- `SPEC.md` "AgentEvent" (`{ kind: "git"; op: "sync" | "merge" | "rebase" | "push"; ok: boolean; detail: unknown }`), "Git", "Sessions" (`POST /sessions/{id}/sync` -> `{ref, commit}`)
- `docs/data-model.md` `events` (lock session row, `MAX(seq)+1`, `pg_notify('session_events')` in the same transaction), `secret_uses`
- ADR 0007, ADR 0021, ADR 0028

## Acceptance criteria
- [ ] `git::service::GitService` (constructed from `AppState` parts: pool, `DataPaths`, `Arc<ProjectGitLocks>`, `Arc<dyn GitCredentialProvider>`) with:
  - `sync_session(project_id, session_id, actor) -> Result<SyncOutcome { r#ref: String, commit: String }>`: lock, `fetch_back`, emit `git` event `op: "sync"` on that session (`ok: true, detail: { ref, commit }`; on failure `ok: false, detail: { ref, error }` then return the error).
  - `sync_session_silent(guard, project_id, session_id)`: the same without an event, for diff and hand-off publication.
  - `list_session_branches(project_id) -> Vec<SessionBranch>`: no lock needed for reads? No: take the lock briefly to list refs and compute counts so a concurrent write-back cannot produce a torn view; emit no event.
  - `diff(project_id, DiffSelector::{Head(String) | Handoff(Uuid)}, base: Option<String>) -> Diff`: under the lock, silently sync when `head` parses to `Session`, resolve `base` (default `Head(default_branch)`) and `head` (`Handoff(id)` resolves `refs/handoffs/<id>` and never syncs), release the lock, run `git::diff::diff`.
  - `merge_branch(project_id, source: String, target: String, message: Option<String>, actor) -> MergeOutcome`: under the lock sync the source session (if `Session`), resolve both, `commit_identity`, run `merge`, emit `git` event `op: "merge"`.
  - `merge_commit(project_id, source_commit: String, source_label: String, target: String, message, actor) -> MergeOutcome`: the task-merge building block: takes a guard already held by the caller (`&ProjectGitGuard`) and a pinned commit, no sync; the Code hand-offs epic wraps it with the hand-off verification.
  - `rebase(project_id, branch: String, onto: String, actor) -> RebaseOutcome`: sync the branch session (if `Session`), resolve, rebase, emit `op: "rebase"` with `detail.work_tree`.
  - `push(project_id, r#ref: String, remote_branch: Option<String>, force: bool, actor) -> PushOutcome`: sync the session (if `Session`), resolve, `credential_for(project_id, actor, 5 min)`, push, emit `op: "push"` with `detail.compare_url`.
  - `fetch_project(project_id, actor, max_age)` re-exported from the mirror task.
- [ ] Event targets: the `git` event is written on every session whose ref took part as `source`, `branch` or `ref` (deduplicated), and additionally on the calling session when `actor` is `GitActor::Session(id)` not already included; operations touching no session (e.g. `origin/main` into `main` by a user) write no session event and rely on the returned body. Each write is one transaction per session via the shared event-append primitive (session row lock, `MAX(seq)+1`, `pg_notify('session_events', '<sid>:<seq>')`), never inside a transaction that holds the project row.
- [ ] `detail` shapes (serde structs in `models/git.rs`, documented in `SPEC.md`):
  - sync: `{ ref, commit }` / failure `{ ref, error }`
  - merge: `{ source, target, commit?, fast_forward?, conflicts?, requested_by, error? }`
  - rebase: `{ branch, onto, commit?, conflicts?, work_tree?: "updated"|"reconciliation_required"|"not_applicable", requested_by, error? }`
  - push: `{ ref, remote_branch, commit?, force, compare_url?, requested_by, error? }`
  where `requested_by` is `"user:<uuid>"`, `"session:<uuid>"` or `"system"` and `error` is the generic user-facing message (never stderr, never a credential).
- [ ] Ref validation errors (`InvalidRef`, wrong kind) are raised before any sync or fetch so a bad request has no side effects; `Conflict` and `NonFastForward` outcomes still record an `ok: false` event with `conflicts` or `error` filled.
- [ ] `SPEC.md` "AgentEvent" gains the `detail` shapes above under the `git` kind (one short paragraph or a `ts` block), and the `git` event target rule ("written on every session whose ref participates and on the calling session") is added to `ARCHITECTURE.md` "Git model" -> "Merge, rebase, push" in the same commit.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/git/service.rs`; `orchestrator/src/models/git.rs` (detail structs); `AppState` gains `git: Arc<GitService>` (or a constructor `GitService::from_state(&AppState)` if a cycle with `AppState` is awkward).
- Session existence and project membership are checked through `SessionRepository::get(project_id, session_id)` before taking the git lock (a plain read, no `FOR UPDATE`), so an unknown session id in `source`/`branch`/`ref` is a 400 `InvalidRef`-style error ("unknown session"), consistent with the sessions endpoint's 400 for an unresolvable base.
- Order inside each mutating operation: (1) validate and parse refs, (2) look up sessions/project rows without locks, (3) acquire the git lock, (4) sync participating sessions silently, (5) resolve to commits, (6) credential/identity (writes `secret_uses` — a plain insert, no project row lock), (7) run the primitive, (8) release the lock, (9) write `git` events, (10) return. Event writes after the lock keep "git lock before any database lock" trivially true and keep the lock short.
- `commit_identity` is obtained for merge and rebase only; push needs `credential_for` only.
- The `git` event write uses the DB epic's `EventRepository::append(session_id, kind = "git", payload)` (or equivalent) which performs the row lock, sequence derivation and `pg_notify`.

## Edge cases
- Session in state `creating` (no work clone yet): sync returns `UnknownRef` -> 400 `session/<sid> has no work tree yet`; no event.
- Session `done`/`failed`: operations on its ref are allowed (the branch remains in the mirror); the session may lack a work dir after deletion, in which case a sync fails but a merge of the existing `refs/sessions/<sid>` should still work: therefore, when the work dir is missing but `refs/sessions/<sid>` exists, skip the sync with a `debug!` instead of failing. Document this rule in the doc comment.
- Event write failure after a successful git operation: log `error!` and still return the successful body (the mirror is already updated; the UI refreshes on next load).
- Two operations on different projects never contend; two on the same project serialise; a caller must not hold a database transaction with the project row while calling the service (doc section `# Ordering`).

## Testing
- `TestApp` integration tests (mock credential provider; project row inserted via `ProjectRepository`, mirror created with `init_project_repo` against a temp upstream, session rows via `SessionRepository`, work clones via `create_work_clone`): `sync_session` writes one `git` event `{op: "sync", ok: true}` with `seq` 1 and a `session_events` notification (assert via `LISTEN` on a raw connection or by reading `events`); `merge_branch` of a session into `main` writes an `ok: true` merge event on that session and, when `actor = Session(other)`, also on `other`; a conflict writes `ok: false` with `conflicts` and returns 422-mapped error; `rebase` writes `work_tree`; `push` non-fast-forward writes `ok: false` and returns the 409-mapped error while `refs/heads/main` is unchanged; `diff` with a session head syncs silently (event count unchanged) and with `handoff_id` never touches the work clone (remove the work dir first); `list_session_branches` writes nothing.
- Ordering test: invalid target `origin/main` returns 400 without creating any event or `secret_uses` row.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `SPEC.md` "AgentEvent": document the `git` event `detail` shapes.
- `ARCHITECTURE.md` "Git model" -> "Merge, rebase, push": one sentence on which sessions receive the outcome event.

## Assumes from other epics
- "Database schema, models, repositories and test harness": the event-append primitive with session row lock and `pg_notify`, `SessionRepository`/`ProjectRepository` reads, `TestApp` with a per-test `DATA_DIR` temp directory (if `TestApp` lacks it, add `data_dir: TempDir` there as part of this task).
- "Code hand-offs and review": wraps `merge_commit` with hand-off verification and calls `sync_session_silent` + `retain_handoff` for publication.
- "Session lifecycle": `POST /sessions/{id}/end` and `/sync` call `sync_session`.
- "MCP server and agent tools": tools call these methods with `GitActor::Session`.