---
id: nky3h
title: "Add SessionRegistry: per-session owner handles, input channels, queued inputs, prompt tracking and rejection rules"
status: done
priority: P1
created: "2026-09-16T20:29:35.110640920Z"
updated: "2026-09-18T20:35:54.486981309Z"
tags:
  - orchestrator
  - sessions
depends_on:
  - cksdv
parent: s52qg
attempts: 1
---

## Summary
Implement `SessionRegistry` in `orchestrator/src/session/registry.rs`: the in-memory map from session id to the running owner's handle, holding the single-writer input channel, the queue used while a session is `creating` or `parked`, the pending-prompt cursor used to reject stale answers, and a per-session launch guard. REST, the WebSocket handler and the cron reapers all reach a session's owner only through this registry, which is what makes stdin single-writer.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals" (`AppState` holds the `SessionRegistry`), "Session owner task" (inputs come from the registry channel; the owner is the only stdin writer), "Session lifecycle" (inputs while `creating`/`parked` are queued and delivered after the init event; ephemeral sessions accept only their launch prompt), "Event delivery" (answer with `reply_to: <seq>` is rejected with `input_rejected` if the prompt was consumed: any later input accepted, or the turn ended), "Input delivery across restarts" (queues are in memory; no durability).
- `SPEC.md` "WebSocket: session stream" (`SessionInput` shape: `{kind:"message", text}` | `{kind:"answer", reply_to, text}`; `client_id` echoed back), "Sessions" (`POST /sessions/{id}/input` → 202, relaunches if parked).
- ADR 0020.

## Acceptance criteria
- [ ] `SessionInput` enum (serde tag `kind`: `message { text }`, `answer { reply_to: i64, text }`) lives in `orchestrator/src/session/input.rs` (or `events/`) and is the type used by the agent backend's `encode_input`.
- [ ] `QueuedInput { input: SessionInput, user_id: Option<Uuid>, client_id: Option<String>, accepted_at: DateTime<Utc> }`.
- [ ] `OwnerCommand::{ Input(QueuedInput), Stop, Shutdown }` is the channel message type; `Stop` means "SIGINT now, SIGTERM after grace"; `Shutdown` tells the owner to exit its loop without touching the container (used by orchestrator shutdown and tests).
- [ ] `SessionRegistry` (Clone, `Arc<Mutex<..>>` inside) API: `register(session_id, kind, phase: Phase::Creating) -> OwnerRx` (creates the `mpsc::channel<OwnerCommand>(64)` and returns the receiver for the owner); `mark_running(session_id) -> Vec<QueuedInput>` (switches phase to `Running` and drains the queue in FIFO order); `mark_parked(session_id)` (drops the sender, keeps the entry's queue and phase `Parked`); `remove(session_id)`; `submit(session_id, QueuedInput) -> SubmitResult`; `stop(session_id) -> Result<()>` (sends `OwnerCommand::Stop`; `Error::Conflict("session is not running")` when no live owner); `set_prompt(session_id, seq, prompt_id)`, `clear_prompt(session_id)`; `is_live(session_id) -> bool`; `try_begin_launch(session_id) -> Option<LaunchGuard>` (a guard that prevents two concurrent launches/resumes of one session; `None` when one is in progress).
- [ ] `SubmitResult::{ Forwarded, Queued, ParkedNeedsRelaunch, Rejected(String) }`: `Forwarded` when the phase is `Running` and the send succeeded; `Queued` when `Creating` (or `Parked` with a relaunch already in progress); `ParkedNeedsRelaunch` when `Parked` with no launch in progress (the input is queued and the caller must call the launcher's resume); `Rejected("prompt already consumed")` for an `answer` whose `reply_to` differs from the pending prompt's `seq` or when no prompt is pending; `Rejected("session has no owner")` when the session id is unknown to the registry and not parked.
- [ ] Accepting any input (queued or forwarded) clears the pending prompt, so a second answer to the same prompt is rejected.
- [ ] The registry never inspects `SessionKind` for ephemeral rejection; that rule is applied by the session service (`Session::accepts_input`) before `submit`. The registry stores `kind` only so the WebSocket handler can read it without a DB round-trip.
- [ ] `AppState` gains `session_registry: SessionRegistry`; `TestApp` exposes it.

## Implementation notes
- Files: `orchestrator/src/session/registry.rs`, `orchestrator/src/session/input.rs`, `orchestrator/src/session/mod.rs`, `orchestrator/src/prelude/state.rs` (AppState field), `orchestrator/tests/common/mod.rs`.
- Use `tokio::sync::mpsc` for the channel and `std::sync::Mutex` for the map (no `.await` while holding it). `LaunchGuard` clears its flag on `Drop`.
- Log with structured fields (`session_id = %id`), never the input text (rule: never log event payloads at info or above).
- The queue is bounded at 256 entries; beyond that `submit` returns `Rejected("input queue full")`.

## Edge cases
- `submit` for a session whose owner channel is closed (owner just exited) must not return `Forwarded`: treat a failed `try_send` as `Queued` with the phase flipped to `Parked` and return `ParkedNeedsRelaunch` so the caller relaunches.
- `mark_running` on an unknown id is a no-op returning an empty vector (owner adopted after restart registers itself first).
- Queued inputs are dropped on `remove` (session failed/ended) — log at `warn` with the count, not the content.

## Testing
- Unit tests in the module with `#[tokio::test]`: queue while creating then `mark_running` drains in order; forward when running is received on the owner receiver; answer accepted only when `reply_to` matches, rejected after another input was accepted and after `clear_prompt`; `ParkedNeedsRelaunch` on the first input to a parked session and `Queued` on the second while the `LaunchGuard` is held; `try_begin_launch` returns `None` while a guard is alive and `Some` after drop; closed channel flips to parked.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Repository scaffolding, tooling and CI" and "Database schema, models, repositories and test harness" deliver `AppState` and `TestApp`.
- "Claude Code agent backend and event translation" consumes `SessionInput` in `AgentBackend::encode_input`; agree on the type location (`session/input.rs`, re-exported from the prelude).

## Correction: `running` on stdin attach, not on `init` (ADR 0032, task 3z8xu)
The pinned CLI writes nothing, `init` included, until its first stdin line (`ARCHITECTURE.md`, "Launch sequence"; `docs/decisions/0032-run-state-on-stdin-attach.md`). Where the text above disagrees, this section wins.
- Inputs arriving while `creating` or `parked` are queued and delivered when the session goes `running`, which is when stdin is attached, not after the `init` event. The `mark_running` API is unchanged; only its caller and timing differ.

## Correction: no `prompt` event and no `answer` input (ADR 0033, task r6yek)
The live probe showed the pinned CLI never asks the host a question under `--permission-mode bypassPermissions --permission-prompts none`, so `SessionInput` has the single kind `message`, `user_message` has no `reply_to`, and `AgentEvent` has no `prompt` (`docs/decisions/0033-no-interactive-prompts-in-v1.md`; `SPEC.md`, "AgentEvent" and "WebSocket: session stream"). Where the text above disagrees, this section wins.
- `SessionInput` has no `answer` variant, so the registry tracks no pending prompt: drop `set_prompt` and the pending-prompt rejection rules. `input_rejected` reasons are about deliverability only (ephemeral session, wrong state, unknown kind).
