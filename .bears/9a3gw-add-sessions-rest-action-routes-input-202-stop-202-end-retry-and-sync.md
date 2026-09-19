---
id: "9a3gw"
title: "Add sessions REST action routes: input (202), stop (202), end, retry and sync"
status: done
priority: P1
created: "2026-09-16T20:34:24.967648834Z"
updated: "2026-09-19T02:02:23.044086212Z"
tags:
  - orchestrator
  - sessions
depends_on:
  - tjccc
  - bppkk
parent: s52qg
attempts: 1
---

## Summary
Add the five action endpoints of the Sessions table to `orchestrator/src/routes/sessions.rs`, each a thin handler over `SessionService`: `POST /sessions/{id}/input` (202), `POST /sessions/{id}/stop` (202), `POST /sessions/{id}/end`, `POST /sessions/{id}/retry` and `POST /sessions/{id}/sync`. The handlers own request validation and status mapping only; every lifecycle rule stays in the service so the WebSocket path behaves identically.

## Documents
- `SPEC.md` "Sessions" table rows: `POST /sessions/{id}/input` `SessionInput` → 202 (same as sending over the socket; relaunches if parked; 409 for an ephemeral session); `POST /sessions/{id}/stop` → 202 (SIGINT then SIGTERM after grace); `POST /sessions/{id}/end` → `Session` (stop, fetch-back, `done`); `POST /sessions/{id}/retry {message?}` → `Session` (conversational only, from `failed`: `parked`, then relaunched at once when `message` is given; 409 for an ephemeral session); `POST /sessions/{id}/sync` → `{ref, commit}`; "REST API" (202 for accepted asynchronous input and stop requests; 409 for a session not in a state that accepts the action); "WebSocket: session stream" (`SessionInput` shape; a `message` is accepted for a conversational session in `creating`, `running` or `parked`; the restart limitation paragraph).
- `ARCHITECTURE.md` "Session lifecycle", "Stop semantics".
- ADR 0020 (202 acknowledges acceptance, not delivery).

## Acceptance criteria
- [ ] `POST /api/sessions/{id}/input` body is a `SessionInput` (`{kind: "message", text}` or `{kind: "answer", reply_to, text}`); empty `text` → 400 `text must not be empty`; malformed body → 400; calls `SessionService::send_input(id, input, Some(caller user id), None)`; success → 202 with an empty body; ephemeral → 409 `ephemeral sessions accept no input`; `done`/`failed` → 409 `session is <state>`; rejected answer → 409 with the registry's reason; 404 unknown.
- [ ] `POST /api/sessions/{id}/stop` → 202 empty body when `running`; 409 `session is <state>` otherwise; 404 unknown.
- [ ] `POST /api/sessions/{id}/end` → 200 `Session` in state `done`; 409 from `creating`, `done`, `failed`; 404.
- [ ] `POST /api/sessions/{id}/retry` body `{message?}` (an absent or empty body is accepted) → 200 `Session` in state `parked` with `error` null; 409 `ephemeral sessions are not retried`; 409 `session is <state>` when not `failed`; 404.
- [ ] `POST /api/sessions/{id}/sync` → 200 `{ "ref": "refs/sessions/<id>", "commit": "<sha>" }`; 409 while `creating`; git failure → the git epic's error mapping (500 for an internal git failure, with the `git` event `ok: false` recorded); 404.
- [ ] All five require a JWT and honour the password-change gate.

## Implementation notes
- Files: `orchestrator/src/routes/sessions.rs`.
- `SessionInput` deserialises with `#[serde(tag = "kind")]`; reuse the type from `session/input.rs`.
- 202 responses use `StatusCode::ACCEPTED` with no body; do not return the session (the client learns the state from the WebSocket `session` message or a `GET`).
- `retry` with a message calls the service, which queues and resumes; the route does not touch the launcher directly.

## Edge cases
- `input` with `reply_to` on a session with no pending prompt → 409 `prompt already consumed`.
- `stop` immediately followed by `end`: `end` observes `running` or `parked` and works either way (the service handles the race).
- `text` longer than 1 MiB → 400 `text too long` (a generous cap so a pasted log cannot exhaust memory; document it in `SPEC.md` "Sessions" in the same commit).

## Testing
- Extend `orchestrator/tests/sessions_api.rs` (TestApp, mock engine, real bare repository): input to a `creating` session → 202 and queued (drained on the fixture `init`); input to a `parked` session → 202 and the mock engine records a new create with `--resume`; input to an ephemeral session → 409; input to a `done` session → 409; malformed input → 400; stop on running → 202 and the mock engine receives `SIGINT`; stop on parked → 409; end on running → 200 `done`, container removed, `refs/sessions/<id>` present, `git` sync event appended; end on creating → 409; retry on failed conversational → 200 `parked`, `error` null; retry with message → relaunch recorded; retry on ephemeral → 409; sync → `{ref, commit}` matching the work tree HEAD; sync on creating → 409; 401 on every route.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `SPEC.md` "Sessions": add the `text` size cap (1 MiB) to the `POST /sessions/{id}/input` description.

## Assumes from other epics
- "Authentication, users, invites and email": JWT extractor.
- "Real-time delivery": the WebSocket handler will call the same `SessionService::send_input`/`stop`; nothing here is WS-specific.

## Correction: no `prompt` event and no `answer` input (ADR 0033, task r6yek)
The live probe showed the pinned CLI never asks the host a question under `--permission-mode bypassPermissions --permission-prompts none`, so `SessionInput` has the single kind `message`, `user_message` has no `reply_to`, and `AgentEvent` has no `prompt` (`docs/decisions/0033-no-interactive-prompts-in-v1.md`; `SPEC.md`, "AgentEvent" and "WebSocket: session stream"). Where the text above disagrees, this section wins.
- `POST /api/sessions/{id}/input` takes `{kind:"message", text}` only; an `answer` body is a 400 like any unknown kind. Drop the `reply_to` / `prompt already consumed` 409 edge case.
