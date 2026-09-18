---
id: bt9q6
title: "Implement SessionOwner input, exit and stop handling: user_message before stdin, parked/failed rules, SIGINT-then-SIGTERM with signal recorded, ephemeral end-of-run"
status: open
priority: P1
created: "2026-09-16T20:32:13.309252641Z"
updated: "2026-09-16T20:32:13.309252641Z"
tags:
  - orchestrator
  - sessions
  - engine
depends_on:
  - qtx4x
parent: s52qg
---

## Summary
Complete `SessionOwner` in `orchestrator/src/session/owner.rs` with its write and exit sides: record each input as a `user_message` event before encoding and writing it to the attached stdin, watch the container for exit and apply the `parked`/`failed` rule with a `state_change` event, implement stop as `SIGINT` then `SIGTERM` after `STOP_GRACE_SECS` with the last signal recorded, and run the ephemeral end-of-run (fetch-back, stop, `done`) when `result` arrives. After this task a launched session can be talked to, stopped and parked.

## Documents
- `ARCHITECTURE.md` "Session owner task" (steps 2 and 3: inputs recorded as `user_message` before writing; exit → `state_change` and `parked` on clean exit or SIGINT stop, `failed` on non-zero exit outside a stop), "Session lifecycle" (state table: `parked` has no container; `failed` has `sessions.error`; ephemeral `running → done` on result), "Stop semantics" (SIGINT via `kill --signal`, `STOP_GRACE_SECS` default 20, then SIGTERM, exit 143, either way `parked`, signal recorded so the UI can say stopped vs killed; end = stop + fetch-back + container removal), "Claude Code invocation" (ephemeral: when `result` arrives the owner runs the fetch-back, stops the container and marks `done`; never parked/resumed/retried), "Event delivery" (answer rejection), "Input delivery across restarts".
- `SPEC.md` "AgentEvent" (`user_message { text, user_id, client_id?, reply_to? }`, `state_change { from, to, reason, signal? }`, `git { op: "sync", ok, detail }`, `error { message, fatal }`), "WebSocket" (`SessionInput`).
- `README.md` "Configuration" (`STOP_GRACE_SECS`).
- `docs/data-model.md` `sessions` (`container_id` NULL once removed; `parked_at`; `ended_at`).
- ADRs 0003, 0010, 0020.

## Acceptance criteria
- [ ] `deliver_input(&mut self, q: QueuedInput)`: append `user_message { text, user_id: q.user_id, client_id: q.client_id, reply_to (for answers) }` via `append_event` in its own transaction; then `backend.encode_input(&q.input)` and write the newline-terminated line to stdin and flush. A write failure appends `error { message: "failed to write input to CLI: <io error>", fatal: false }`, logs `warn!`, and does not retry (ADR 0020). For an `answer`, if the registry's pending prompt no longer matches, drop it with a `warn!` (the registry rejected it upstream; this is a race guard). After any successful delivery, `registry.clear_prompt(sid)`.
- [ ] Container exit is observed through `engine.wait(container_id)` in the `select!`; on exit the owner first drains the transcript to EOF (so the final `result` line is committed before the transition), then applies: stop requested → `transition(running → parked, reason = "stopped by user", signal = last signal sent)`; exit code 0 and no stop → `parked`, `reason = "CLI exited"`; non-zero exit and no stop → `transition(running → failed, reason = "CLI exited with status <code>", error = same text)`; container no longer exists (engine NotFound) → `parked`, `reason = "container disappeared"`. If the session is still `creating` when the container exits (no `init` seen) → `failed`, `error = "CLI exited with status <code> before init"`.
- [ ] Ephemeral sessions: exit before `result` → `failed` with `error = "CLI exited with status <code> before result"`; `on_result` hook: run the fetch-back of `session/<sid>` into `refs/sessions/<sid>` under the project git lock and append `git { op: "sync", ok, detail: { ref, commit } | { error } }`; then if the container is still running wait up to `STOP_GRACE_SECS` for it to exit on its own and send `SIGTERM` otherwise; `transition(running → done, reason = "result received")`; call the `on_session_ended(sid)` hook (lease release, wired later).
- [ ] After every transition out of `running`: remove the container (`engine.remove(id, force = true)`, ignoring NotFound), `set_container_id(sid, None)`, `registry.mark_parked(sid)` for `parked` or `registry.remove(sid)` for `done`/`failed`, close stdin, return from the loop.
- [ ] `OwnerCommand::Stop`: `engine.kill(cid, "SIGINT")`, remember `stop_signal = Sigint` and arm a `tokio::time::sleep(STOP_GRACE_SECS)`; when it fires without an exit, `engine.kill(cid, "SIGTERM")`, `stop_signal = Sigterm`. A second `Stop` while one is pending is ignored. `state_change.signal` carries the last signal sent.
- [ ] `last_activity_at` is advanced by every appended event, including `user_message`.
- [ ] `Config.stop_grace_secs` is read from `STOP_GRACE_SECS` (default 20); `TestApp` sets it to 1.

## Implementation notes
- Files: `orchestrator/src/session/owner.rs`; `orchestrator/src/prelude/config.rs` if `stop_grace_secs` is missing.
- The stdin writer is the engine's attach stream (`Box<dyn AsyncWrite + Send + Unpin>`); after adoption (recovery task) it is a fresh attach. Keep `stdin: Option<..>` so an owner without an attach (engine failure) still tails and records inputs.
- Lock order for the ephemeral fetch-back: project git lock first, then the event transaction (session row lock); never the reverse.
- Provide `on_session_ended: Option<Arc<dyn Fn(Uuid) -> BoxFuture<'static, ()> + Send + Sync>>` on `AppState` (default no-op) as the hook the tracker's lease release plugs into; the launch-for-task task and the session service call it too.

## Edge cases
- Exit observed while lines are still being written (CLI exits right after `result`): the drain-to-EOF before the transition covers it; use a final read with a 200 ms settle loop (read until two consecutive empty reads).
- `engine.wait` fails because the engine is unreachable: log `error!`, keep tailing, retry `wait` with backoff (1 s, 2 s, ... capped at 30 s); do not park the session on a transient engine error.
- Stop received while the session is `creating` (no container yet): ignore and log; the route answers 409 before it gets here.
- Input for an ephemeral session never reaches the owner (service rejects it); if one does, drop it with `warn!`.
- Container removed by the orphan-cleanup job while parked: not this owner's concern (it has already returned).

## Testing
- Extend `orchestrator/tests/session_owner.rs` with the mock engine's controllable exit and a capturing stdin writer: a `message` input produces a `user_message` event before the mock stdin sees the encoded line, with `user_id` and `client_id`; a write failure produces the non-fatal `error` event; exit 0 → `parked`, `state_change{from: running, to: parked, reason: "CLI exited"}`, container removed, `container_id` null, `parked_at` set; exit 137 → `failed` with exact `error`; `Stop` → mock engine receives `SIGINT`, then (grace 1 s) `SIGTERM`, exit 143 → `parked` with `signal: "SIGTERM"`; `Stop` followed by exit within grace → `signal: "SIGINT"` and no SIGTERM sent; ephemeral fixture with a `result` line → `git` sync event, `done`, `ended_at` set, container removed, `on_session_ended` hook invoked; ephemeral exit before result → `failed`; exit while `creating` → `failed` with the "before init" error; answer with a stale `reply_to` is dropped.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Container engine adapter": `kill(id, signal)`, `wait(id) -> exit code`, `remove(id, force)`, mock engine with scriptable exit codes and a capturing stdin.
- "Git operations": `fetch_back(session) -> (ref, commit)` under the project git lock.
- "Claude Code agent backend": `AgentBackend::encode_input`.
- "Task tracker": `release_leases_for_session(session_id, reason)` behind the `on_session_ended` hook.

## Correction: no `prompt` event and no `answer` input (ADR 0033, task r6yek)
The live probe showed the pinned CLI never asks the host a question under `--permission-mode bypassPermissions --permission-prompts none`, so `SessionInput` has the single kind `message`, `user_message` has no `reply_to`, and `AgentEvent` has no `prompt` (`docs/decisions/0033-no-interactive-prompts-in-v1.md`; `SPEC.md`, "AgentEvent" and "WebSocket: session stream"). Where the text above disagrees, this section wins.
- `user_message` is `{ text, user_id, client_id? }`; there is no `reply_to`.
