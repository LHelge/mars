---
id: bppkk
title: "Add SessionService actions: send_input with parked relaunch, stop, end with fetch-back and lease release, retry, sync, delete with directory and transcript removal"
status: done
priority: P1
created: "2026-09-16T20:32:52.275635850Z"
updated: "2026-09-19T01:23:07.575959224Z"
tags:
  - orchestrator
  - sessions
  - git
depends_on:
  - "9wxhs"
  - bt9q6
parent: s52qg
attempts: 1
---

## Summary
Implement `SessionService` in `orchestrator/src/session/service.rs`: the one place that applies the lifecycle rules for user-initiated actions so that REST routes, the WebSocket handler and cron jobs share identical behaviour. It covers input submission (with the "message to a parked session relaunches it" rule and ephemeral refusal), stop, end (stop, fetch-back, container removal, `done`, lease release hook), retry (`failed → parked`, relaunch when a message is given), sync (fetch-back with a `git` event) and delete (directory and CLI transcript removal).

## Documents
- `ARCHITECTURE.md` "Session lifecycle" (state table: input accepted in creating/running/parked, parked input triggers relaunch, `done` not resumable, `failed` retry only for conversational; ephemeral accept only their launch prompt), "Stop semantics" (end = stop, final fetch of the session branch into the mirror, container removal; directory kept until delete), "Storage" (delete removes the session directory and, with `cli_session_id`, `<claude>/projects/-session-work/<cli_session_id>.jsonl` and any directory of the same name), "Git model" → "Fetch-back" (on session end and on explicit sync: `git -C <mirror> fetch <work> session/<sid>:refs/sessions/<sid>` force, under the project git lock), "Task tracker" → "Liveness" (ending a session from the UI releases its leases immediately).
- `SPEC.md` "Sessions" (`POST .../input` 202, relaunches if parked, 409 ephemeral; `POST .../stop` 202; `POST .../end` → `Session`; `POST .../retry {message?}` conversational only from `failed` → `parked` then relaunched at once when `message` given, 409 ephemeral; `POST .../sync` → `{ref, commit}`; `DELETE` 204 must be `done` or `failed`), "AgentEvent" (`git { op: "sync", ok, detail }`, `state_change`).
- ADRs 0003, 0020.

## Acceptance criteria
- [ ] `send_input(session_id, input: SessionInput, user_id: Option<Uuid>, client_id: Option<String>) -> Result<()>`: load the session; `Session::accepts_input()?` (ephemeral → 409 `ephemeral sessions accept no input`; done/failed → 409 `session is <state>`); `registry.submit(...)`; on `ParkedNeedsRelaunch` call `launcher.launch(id, LaunchMode::Resume)`; on `Rejected(reason)` return `Error::Conflict(reason)`; `Forwarded`/`Queued` → `Ok(())`.
- [ ] `stop(session_id) -> Result<()>`: state must be `running` (else 409 `session is <state>`); `registry.stop(id)`; returns immediately.
- [ ] `end(session_id) -> Result<Session>`: allowed from `running` and `parked` only (creating/done/failed → 409). `running`: `registry.stop(id)` and await the owner's exit (poll the session state every 100 ms until it leaves `running`, bounded by `STOP_GRACE_SECS + 10 s`; on timeout `engine.kill(cid, "SIGKILL")`, `engine.remove(force)` and continue); then, from `parked` (or `failed` if the run ended badly during the stop), fetch-back under the project git lock and append `git { op: "sync", ok, detail }` (a fetch-back failure is recorded, not fatal); `transition(parked|failed → done, reason = "ended by user")`; ensure no container remains and `container_id` is null; `registry.remove(id)`; invoke the `on_session_ended(id)` hook; return the session.
- [ ] `retry(session_id, message: Option<String>, user_id) -> Result<Session>`: `Session::can_retry()?` (ephemeral → 409 `ephemeral sessions are not retried`; not `failed` → 409 `session is <state>`); `transition(failed → parked, reason = "retried by user")` which clears `error`; if `message` is `Some`, call `send_input` (which queues and resumes); return the session as read after the transition.
- [ ] `sync(session_id) -> Result<SyncResult { ref: String, commit: String }>`: state must not be `creating` (409); fetch-back under the project git lock; append `git { op: "sync", ok: true, detail: {ref, commit} }` on success or `ok: false, detail: {error}` and return the git error mapped through `Error` on failure.
- [ ] `delete(session_id) -> Result<()>`: state must be `done` or `failed` (409 `session must be done or failed`); if a container is still recorded, remove it (force, ignore NotFound); remove `DATA_DIR/sessions/<sid>` recursively (ignore NotFound); if `cli_session_id` is set, remove `DATA_DIR/projects/<pid>/claude/projects/-session-work/<cli_session_id>.jsonl` and the directory `.../-session-work/<cli_session_id>/` if present (ignore NotFound); `registry.remove(id)`; `SessionRepository::delete(id)` (cascades events and secret uses).
- [ ] `SessionService` is constructed from `AppState` (`SessionService::new(&state)`) and used by the routes task and by the real-time epic's WebSocket handler.

## Implementation notes
- Files: `orchestrator/src/session/service.rs`, `orchestrator/src/session/mod.rs`.
- Fetch-back is the git epic's function; the service only sequences it and writes the outcome event. Lock order: git lock, then the event transaction.
- `end` from `parked` never starts a container.
- The `on_session_ended` hook is the `AppState` field introduced by the owner task; the tracker epic installs the lease release there.

## Edge cases
- `end` racing with the idle reaper parking the session: `transition` from the observed state fails with a conflict; re-read the state once and retry the transition from the new state before giving up.
- `retry` of a session that failed during `creating` (no `cli_session_id`, possibly no clone): after `failed → parked`, the resume path falls back to a fresh launch (defined in the launcher task).
- `delete` when the directory removal fails with a permission error: return `Error::Internal`, leave the row (the next delete retries).
- `send_input` to a `creating` session queues without launching anything.
- `sync` while `running`: allowed; the fetch reads whatever is committed in the work tree.

## Testing
- Integration tests in `orchestrator/tests/session_service.rs` with `TestApp`, the mock engine and a real bare repository: message to a parked session triggers a resume (mock engine sees a new create with `--resume`); ephemeral input → 409; done → 409; stop from parked → 409; end from running sends SIGINT, reaches `done`, writes the `git` sync event, updates `refs/sessions/<sid>` in the mirror, removes the container, invokes the hook once; end from parked skips the engine; retry from failed conversational → parked with `error` null and, with a message, a relaunch; retry ephemeral → 409; sync returns `{ref, commit}` and appends the event; delete from done removes `sessions/<sid>` and the CLI transcript file, deletes the row and its events; delete from running → 409.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Git operations": `fetch_back` and the project git lock.
- "Task tracker": `release_leases_for_session` installed in `on_session_ended` (until then the hook is a no-op).
- "Container engine adapter": `kill`, `remove`, mock engine.