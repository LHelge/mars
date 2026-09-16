---
id: "7u3pu"
title: Implement the mirror fetch routine and the project branch listing
status: open
priority: P1
created: "2026-09-16T20:29:46.431192814Z"
updated: "2026-09-16T20:29:46.431192814Z"
tags:
  - orchestrator
  - git
  - projects
  - cron
depends_on:
  - cfrb3
parent: z4u4e
---

## Summary
Deliver the one fetch routine that the cron mirror-fetch job, `POST /projects/{id}/fetch` and the fresh-session launch all reuse: `git fetch --prune origin` under the project git lock with the project credential, touching only upstream-tracking refs and tags and updating `projects.last_fetched_at`. Also deliver the `Branch[]` listing behind `GET /projects/{id}/branches` (integration heads, upstream-tracking refs and session refs).

## Documents
- `ARCHITECTURE.md` "Git model" -> "Project clone" (cron every `MIRROR_FETCH_INTERVAL_SECS`, `last_fetched_at`, fresh launch and `POST /projects/{id}/fetch` run the same fetch), "Ref ownership" (a fetch must preserve an unpushed merge on an integration branch even if upstream moves or deletes that branch)
- `ARCHITECTURE.md` "Launch sequence" (fetch skipped if fetched < 30 s ago; failure is a `launch_warning`)
- `ARCHITECTURE.md` "Background jobs" (mirror fetch row)
- `SPEC.md` "Projects" (`POST /projects/{id}/fetch`, `GET /projects/{id}/branches`, `Branch` shape, "Fetching refreshes upstream-tracking refs without moving integration heads or session refs")
- `docs/data-model.md` `projects.last_fetched_at`, `secret_uses` (mirror-fetch job records neither `session_id` nor `user_id`)
- `README.md` "Configuration" (`MIRROR_FETCH_INTERVAL_SECS`), "Operating notes" (fetch/integration explanation)
- ADR 0017

## Acceptance criteria
- [ ] `git::mirror::fetch_upstream(guard: &ProjectGitGuard, paths, credential: Option<&GitCredential>) -> Result<(), GitError>` runs `git fetch --prune origin` in the project repository with the credential config attached when present.
- [ ] `git::mirror::fetch_project(state: &AppState, project_id: Uuid, actor: &GitActor, max_age: Option<Duration>) -> Result<FetchOutcome { fetched: bool, at: DateTime<Utc> }>` acquires the project git lock, skips (`fetched: false`) when `max_age` is given and `last_fetched_at` is newer than `now - max_age`, otherwise obtains the credential via `GitCredentialProvider::credential_for(project_id, actor, ..)`, runs `fetch_upstream` and updates `projects.last_fetched_at = NOW()` through `ProjectRepository` after the command succeeds (no database transaction is open while git runs).
- [ ] The routine never moves `refs/heads/*`, `refs/sessions/*` or `refs/handoffs/*`, proven by the "unpushed merge survives" test below.
- [ ] `git::mirror::list_branches(paths, project_id) -> Result<Vec<Branch>>` returns integration heads (`kind: head`, name `main`), upstream refs (`kind: upstream`, name `origin/main`) and session refs (`kind: session`, name `<uuid>`, `session_id` set), ordered heads, upstream, sessions, each alphabetically; tags and hand-off refs excluded.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/git/mirror.rs` (extend), `ProjectRepository::touch_last_fetched(project_id)` (`UPDATE projects SET last_fetched_at = NOW() WHERE id = $1`) — if the DB epic did not add it, add it here and refresh `.sqlx/` with `cargo sqlx prepare`.
- `GitActor::System` for the cron job (no `secret_uses` actor columns), `GitActor::User(id)` for `POST /projects/{id}/fetch`, `GitActor::User(created_by)` for a fresh launch (the launcher passes `max_age = Some(30s)`).
- `--prune` deletes `refs/remotes/origin/<b>` for branches deleted upstream and, because of the tag refspec, prunes tags deleted upstream; both are upstream-owned namespaces.
- `list_branches` reuses `git::refs::list` with patterns `refs/heads/`, `refs/remotes/origin/`, `refs/sessions/`.

## Edge cases
- Project not `ready` (still cloning or in `error`): `fetch_project` returns `Error::Conflict("project is not ready")` (409) before taking the lock; the cron job only selects `ready` projects.
- Upstream unreachable: `GitError::Command` propagates; `last_fetched_at` unchanged; the launcher turns it into a `launch_warning`, the cron job logs and retries next interval, the REST endpoint answers 500 with a generic message.
- Concurrent fetches on one project serialise on the git lock; the second sees a fresh `last_fetched_at` only if it passes `max_age`, otherwise fetches again (harmless).

## Testing
- Real repositories: after `init_project_repo`, advance upstream `main` and add a branch and a tag; `fetch_upstream` updates `refs/remotes/origin/main`, adds the new upstream branch and tag, and leaves `refs/heads/main` unchanged. Delete the upstream branch; the next fetch prunes `refs/remotes/origin/<b>` but keeps `refs/heads/<b>`.
- "Fetch preserves an unpushed integration merge": create a commit directly on `refs/heads/main` in the mirror (simulating a local merge) that upstream does not have, move upstream `main` forward, fetch, assert `refs/heads/main` still points at the local commit and `refs/remotes/origin/main` at the upstream one.
- `TestApp` test for `fetch_project`: `last_fetched_at` updated; with `max_age = 30 s` a second call returns `fetched: false`; the mock credential provider recorded `GitActor::System`.
- `list_branches` shape and ordering test including a session ref and excluding a tag.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `ProjectRepository` with insert/get so tests can create a `ready` project row.
- "Projects, agent profiles and shared directories": the `POST /projects/{id}/fetch` and `GET /projects/{id}/branches` handlers that call these functions.
- "Background jobs": the cron job that iterates `ready` projects and calls `fetch_project` with `GitActor::System`.