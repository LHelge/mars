---
id: xdz2q
title: Add POST /projects/{id}/fetch and GET /projects/{id}/branches
status: open
priority: P2
created: "2026-09-16T20:31:49.216725076Z"
updated: "2026-09-16T20:31:49.216725076Z"
tags:
  - orchestrator
  - projects
  - git
depends_on:
  - cgj5v
parent: pkaee
---

## Summary
Add the two read-mostly git-facing project endpoints: an on-demand mirror fetch that reuses the git epic's fetch routine and updates `last_fetched_at`, and the branch listing that exposes integration heads, upstream-tracking refs and session refs with their commits so the session-launch form can offer bases. Both mount into `routes/projects.rs`.

## Documents
- `SPEC.md` "Projects" rows `POST /projects/{id}/fetch` (→ `Project`, runs a mirror fetch now) and `GET /projects/{id}/branches` (→ `Branch[]`: integration heads, upstream-tracking refs and session refs), and the paragraph defining `Branch = { name, kind: "head" | "upstream" | "session", commit, session_id? }`, "Integration heads use names such as `main`; upstream-tracking refs use `origin/main`. Fetching refreshes upstream-tracking refs without moving integration heads or session refs."
- `ARCHITECTURE.md` "Git model" → "Project clone" (cron and on-demand fetch are the same routine; `last_fetched_at`), "Ref ownership" (`refs/remotes/origin/<b>`, `refs/heads/<b>`, `refs/sessions/<sid>`; `refs/handoffs/*` never listed), "Serialization" (fetch under the project git lock; read-only ref resolution captured under the lock).
- `docs/data-model.md` `projects.last_fetched_at`, `secret_uses` (`purpose = 'git'`, `user_id` for REST).

## Acceptance criteria
- [ ] `POST /api/projects/{id}/fetch` → 200 `Project` with a fresh `last_fetched_at` after a successful `git fetch --prune origin` on `repo.git` under the project git lock; 409 "project is not ready" when `status != ready`; 404 unknown; 401 unauthenticated. A fetch failure propagates the git epic's `GitError` mapping (its documented status) and leaves `status`, `status_message` and `last_fetched_at` unchanged.
- [ ] The credential use for an on-demand fetch is recorded with `purpose = git`, `user_id = current user`, `session_id = NULL`.
- [ ] `GET /api/projects/{id}/branches` → 200 `Branch[]`: for each `refs/heads/<b>` an entry `{name: "<b>", kind: "head", commit}`; for each `refs/remotes/origin/<b>` `{name: "origin/<b>", kind: "upstream", commit}`; for each `refs/sessions/<sid>` `{name: "refs/sessions/<sid>", kind: "session", commit, session_id: "<sid>"}`; ordered heads, then upstream, then session, each by name; `refs/tags/*` and `refs/handoffs/*` are excluded; `session_id` is omitted (not null) for non-session entries.
- [ ] `branches` returns 409 "project is not ready" when the mirror does not exist yet (`cloning`/`error`), 404 unknown, 401 unauthenticated.
- [ ] `branches` reads under the project git lock (one `git for-each-ref` call) so it never observes a half-written seeding.

## Implementation notes
- Files: `orchestrator/src/routes/projects.rs` (two handlers), possibly `orchestrator/src/git/` (a `list_refs(path, patterns) -> Vec<(String, String)>` helper using `git for-each-ref --format='%(refname) %(objectname)' refs/heads refs/remotes/origin refs/sessions` if the git epic did not deliver one; add it there, argv array, no shell).
- Fetch: call the git epic's mirror-fetch routine (the same function the cron job and the launcher call), then `ProjectRepository::touch_fetched(id)`, then return `get(id)`. Do not reimplement the fetch.
- `Branch` is a route-private DTO with `#[serde(skip_serializing_if = "Option::is_none")] session_id`.
- Sequence for both handlers: load project → check `ready` → acquire git lock → git command → release → database update (fetch only). No database transaction is open while waiting for the git lock.

## Edge cases
- A mirror with an integration head that upstream has since deleted still lists the head (integration heads are never pruned) and no longer lists the upstream ref.
- `refs/sessions/<sid>` whose session row was deleted may still exist until orphan cleanup; list it anyway with `session_id` parsed from the ref name (a non-UUID suffix is skipped with a `tracing::warn!`).
- Concurrent fetch and clone job or deletion: serialized by the git lock; after deletion the handler's `get` returns 404.
- Symbolic refs (`HEAD`) are not in the requested namespaces and are not listed.

## Testing
- Integration tests in `orchestrator/tests/projects.rs` with a `file://` bare repository and `wait_for_clone`:
  - `fetch` on a `ready` project → 200 and `last_fetched_at` advances; add a commit and a second branch to the bare repo, fetch → `branches` shows `origin/<new>` as `upstream` but no `head` for it, and the original head's commit is unchanged while `origin/<b>` moved.
  - `fetch` on a `cloning` project → 409; on a project whose remote directory was removed → the git error status and `status` still `ready`.
  - `branches` on a fresh project → heads and upstream entries for every branch, none of kind `session`; after creating `refs/sessions/<uuid>` directly with `git update-ref` in `repo.git` → one `session` entry with `session_id`; a `refs/tags/v1` and a `refs/handoffs/<uuid>` created the same way do not appear.
  - `branches` on a `cloning` project → 409; unknown → 404; both endpoints 401 without a token.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- `SPEC.md` "Projects": the `Branch` paragraph does not say what `name` a session entry carries; add the phrase "session refs use their full name `refs/sessions/<id>`" in the same commit.

## Assumes from other epics
- "Git operations": the mirror-fetch routine (shared with the cron job), the per-project git lock, `GitCredentialProvider` with a REST-user actor, `GitError`'s HTTP mapping in `Error`.
- "Background jobs": the cron mirror fetch calls the same routine; this task does not schedule anything.