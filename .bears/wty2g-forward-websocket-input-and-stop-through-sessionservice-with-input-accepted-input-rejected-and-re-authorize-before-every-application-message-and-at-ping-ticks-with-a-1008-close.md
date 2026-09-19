---
id: wty2g
title: Forward WebSocket input and stop through SessionService with input_accepted/input_rejected, and re-authorize before every application message and at ping ticks with a 1008 close
status: in_progress
priority: P1
created: "2026-09-16T20:46:37.465370822Z"
updated: "2026-09-19T21:42:52.252743924Z"
tags:
  - orchestrator
  - realtime
  - sessions
  - auth
depends_on:
  - "3c37q"
parent: h8kw9
attempts: 1
---

## Summary
Fill in the WebSocket's write side: `input` messages go through `SessionService::send_input` so the socket and REST behave identically (relaunch of a parked session, refusal of ephemeral sessions, stale-answer rejection), answered with `input_accepted` or `input_rejected` carrying the client's `client_id`; `stop` goes through `SessionService::stop`. Every application message and every ping tick first runs `reauthorize`; a failure sends `error { message: "authentication required" }` and closes with code 1008 without touching the agent session.

## Documents
- `SPEC.md` "WebSocket: session stream" (client `input { client_id, input: SessionInput }`, `stop {}`; server `input_accepted { client_id, seq }`, `input_rejected { client_id, reason }`; "`client_id` is a client-generated string echoed back so optimistic UI can reconcile. A `message` is accepted for a conversational session in `creating`, `running` or `parked`; ephemeral sessions reject all additional input, matching the REST contract. An `answer` whose `reply_to` prompt has already been consumed is rejected."; the ADR 0020 paragraph: `input_accepted` "acknowledge[s] acceptance by the orchestrator, not guaranteed delivery"), "Sessions" (`POST /sessions/{id}/input` 202, relaunches if parked, 409 ephemeral; `POST /sessions/{id}/stop` 202), "Authentication" ("checked again before every incoming WebSocket application message (including terminal bytes), and at the existing WebSocket ping ... ticks"; "WebSocket sends the existing `error` message with `authentication required` and closes with code 1008"; "Database failures must not authorize input").
- `ARCHITECTURE.md` "Event delivery" ("Input is single-writer: the WebSocket handler forwards inputs to the session's owner through the registry, which serialises them ... the answer is rejected with an `input_rejected` message on the socket rather than being written to the CLI"), "User authentication and revocation", "Session lifecycle" (accepts-input table).
- ADRs 0020, 0025.

## Acceptance criteria
- [ ] `handle_client_message` runs `reauthorize(state, principal)` before every `ClientMessage` (the terminal task adds binary frames); `Err(Revoked)` or `Err(Unavailable)` → send `ServerMessage::Error { message: AUTH_REQUIRED.into() }`, close with `CloseFrame { code: 1008, reason: "authentication required" }`, end the task. The ping-tick hook from the read-side task runs the same check before each ping.
- [ ] `Input { client_id, input }` → `SessionService::send_input(session_id, input, Some(principal.user_id), Some(client_id.clone()))`: `Ok(())` → `InputAccepted { client_id, seq }` where `seq = SessionRepository::max_seq(session_id)` read after acceptance; `Err(Error::Conflict(reason))` → `InputRejected { client_id, reason }` with the service's exact strings (`ephemeral sessions accept no input`, `session is done`, `session is failed`, `prompt already consumed`, `input queue full`, `session has no owner`); any other error → `error { message: "internal error" }`, close 1011, `tracing::error!`.
- [ ] `Stop` → `SessionService::stop(session_id)`: `Ok` → no frame (the `state_change` event and `session` message follow through the read side); `Err(Conflict)` (session not `running`) → ignored with `debug!`, because the protocol defines no stop rejection and `error` implies close.
- [ ] `client_id` is echoed verbatim; `client_id` longer than 128 bytes → `InputRejected { reason: "client_id too long" }` without calling the service.
- [ ] A revoked user's running agent session is unaffected: no stop, no state change, only the socket closes; the terminal task extends disposal to the terminal.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/ws/input.rs` (new: `handle_input`, `handle_stop`), `orchestrator/src/ws/mod.rs` (dispatch and the re-authorization guard as one function `ensure_authorized(ctx) -> Result<(), CloseNow>` used by both the message path and the ping tick).
- `SessionService::new(&state)` once per socket task; it owns the parked-relaunch and ephemeral rules, so the handler never inspects `kind` itself.
- `seq` in `input_accepted` is undefined by the spec beyond its type. Add after the `client_id` paragraph of `SPEC.md` "WebSocket: session stream": "`seq` in `input_accepted` is the highest committed sequence at acceptance; the `user_message` event recording the input follows later with the same `client_id`." in the same commit.
- Reauthorization per message is one `find_by_id` on the pool per frame; the specification asks for it and v1 accepts the cost. No transaction, no lock.
- Never log input text (`never log event payloads at info or above`; user input is transcript content, ADR 0027).

## Edge cases
- `input` while the session is `creating`: the service queues it and returns `Ok`; `input_accepted` is still sent (acceptance by the orchestrator, ADR 0020).
- `input` to a `parked` session triggers a resume; the `state_change` and `session` frames arrive through the read side.
- `answer` with a stale `reply_to` → `input_rejected { reason: "prompt already consumed" }` and nothing reaches the CLI.
- Two sockets on one session both sending: the registry serialises; each socket gets its own `input_accepted`.
- `Unavailable` from `reauthorize` (database down) → 1008 exactly like `Revoked`; the input is never forwarded.

## Testing
- `orchestrator/tests/ws_input.rs` with `TestApp` and a `running` conversational session whose owner is the mock-engine-backed owner harness (or, if that harness is not yet usable, a registry-registered fake owner: `registry.register` + `mark_running` and assertions on the received `OwnerCommand`): `input {kind: message}` → `input_accepted` with the client id and `seq >= max_seq before`, followed by a `user_message` event frame carrying `client_id` and `user_id` (owner harness) or the `OwnerCommand::Input` with the same `client_id` (fake owner); ephemeral session → `input_rejected` `ephemeral sessions accept no input` and nothing forwarded; `done` session → `session is done`; `parked` conversational session → accepted and the mock engine records a new container create (resume); stale `answer` → `prompt already consumed`; `stop` on running → mock engine receives `SIGINT` (or `OwnerCommand::Stop`); `stop` on parked → no frame, socket stays open; `client_id` of 129 bytes → `client_id too long`; revocation: bump `auth_version` then send `input` → `error { authentication required }`, close code 1008, session state unchanged, nothing forwarded; revocation at a ping tick without client traffic → same close within two ping intervals; deleted user → 1008; `must_change_password` set after open → 1008.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `SPEC.md` "WebSocket: session stream": the `seq` clarification sentence (see notes). Everything else implements the documented contract as written.

## Assumes from other epics
- "Session lifecycle": `SessionService::{send_input, stop}` with the documented `Conflict` strings, `SessionRegistry` with `register`/`mark_running`/`OwnerCommand`, and `SessionRepository::max_seq`.
- "Authentication": `auth_version` and `must_change_password` mutations to trigger revocation in tests.

## Correction: no `prompt` event and no `answer` input (ADR 0033, task r6yek)
The live probe showed the pinned CLI never asks the host a question under `--permission-mode bypassPermissions --permission-prompts none`, so `SessionInput` has the single kind `message`, `user_message` has no `reply_to`, and `AgentEvent` has no `prompt` (`docs/decisions/0033-no-interactive-prompts-in-v1.md`; `SPEC.md`, "AgentEvent" and "WebSocket: session stream"). Where the text above disagrees, this section wins.
- Replace the edge case "`answer` with a stale `reply_to` -> `input_rejected`" with an undeliverable-input case (input to an ephemeral session, or in a state that takes none).
