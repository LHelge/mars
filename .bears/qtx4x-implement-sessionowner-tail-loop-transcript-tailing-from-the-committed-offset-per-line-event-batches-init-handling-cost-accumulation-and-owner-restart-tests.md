---
id: qtx4x
title: "Implement SessionOwner tail loop: transcript tailing from the committed offset, per-line event batches, init handling, cost accumulation, and owner restart tests"
status: open
priority: P1
created: "2026-09-16T20:30:45.126836266Z"
updated: "2026-09-17T20:04:50.000932899Z"
tags:
  - orchestrator
  - sessions
  - agent
  - tests
depends_on:
  - "88tdh"
  - nky3h
  - n7tzv
parent: s52qg
---

## Summary
Implement the read side of `SessionOwner` in `orchestrator/src/session/owner.rs`: tail `log/stream.jsonl` from the last committed `_offset`, translate each complete native line through the `AgentBackend`, commit the line's events with its end offset and counters in one transaction, handle `system`/`init` (store `cli_session_id`, move `creating`/`parked` → `running`, warn when `mars-orchestrator` is not connected, flush queued inputs), fold `result` into the cost counters, and track pending prompts. This task also delivers the epic's owner-restart test suite: feed a transcript line by line, kill and restart the owner mid-file, and prove `events` has no gaps and no duplicates. Stdin writing, exit handling and stop semantics are the next task; this one only defines the hooks they plug into.

## Documents
- `ARCHITECTURE.md` "Session owner task" (loop steps 1 and 5; no in-memory seq counter; offset rechecked under the lock; a line's events, offset and counters commit together), "Launch sequence" (init → `running`, `cli_session_id` stored, flush queued inputs, generated task message first), "Durability and recovery" (`_offset` is the byte offset just past the native line; advanced only on the last event of a line; resume from the last committed offset), "Cost accounting" (`result.total_cost_usd` and `usage` accumulated in the same transaction as the event; per-turn vs cumulative rule follows open question 5), "MCP design" (launch_warning when `init` does not list `mars-orchestrator` as connected), "Claude Code invocation" (`cli_session_id` from `init.session_id`).
- `SPEC.md` "AgentEvent" (`init { cli_session_id, model?, tools, mcp_servers: {name, status}[], resumed }`, `result { cost_usd?, usage? }`, `prompt`, `launch_warning`), "Sessions" (`Session.last_activity_at`, counters).
- `docs/data-model.md` `events`, `sessions` (`last_activity_at`, `cost_usd`, `input_tokens`, `output_tokens`, `cli_session_id`).
- ADRs 0010, 0021, 0028.

## Acceptance criteria
- [ ] `SessionOwner::spawn(OwnerContext { session_id, kind, dirs: SessionDirs, backend: Arc<dyn AgentBackend>, start_offset: u64, resumed: bool, stdin: Option<Box<dyn AsyncWrite + Send + Unpin>>, container_id: Option<String>, commands: OwnerRx, state: AppState }) -> JoinHandle<()>` runs the loop; on return it removes itself from the registry.
- [ ] Tailing: open `stream.jsonl`, seek to `start_offset`, poll for new bytes every 100 ms (a simple interval; no inotify dependency), buffer bytes until `\n`, and process only complete lines. A trailing partial line is kept in the buffer and not committed.
- [ ] Per complete line ending at byte offset `end`: `events = backend.translate(line, &mut translate_state)`; if `events` is empty, advance the in-memory offset only (nothing to commit; the line is re-read harmlessly after a restart); otherwise call `SessionRepository::append_native_line(tx, sid, &events, end, committed_offset, cost_delta)` in one transaction and set `committed_offset = end` only after commit. A `Conflict("transcript offset moved")` means another writer owns the file: log `error!` with `session_id` and exit the loop without transitioning the session.
- [ ] `init` event in the batch: before committing the batch, in the same transaction, `set_cli_session_id` and `transition(creating|parked → running, reason = "init received")`; the batch commit therefore carries the `init` event, the `state_change` and the `session_state` notify together. If `init.mcp_servers` has no entry `{name: "mars-orchestrator", status: "connected"}` (compare the status string case-insensitively), append `launch_warning { message: "MCP server mars-orchestrator is not connected (status: <status or 'missing'>)" }` in the same batch. After commit: `registry.mark_running(sid)` and hand each drained `QueuedInput` to the input hook (`self.deliver_input`, implemented by the next task; here it is a method that records the `user_message` and writes stdin if a writer exists).
- [ ] `result` event: compute `CostDelta` from `cost_usd` and `usage.input_tokens`/`usage.output_tokens` (missing → 0) using the accumulation rule constant `COST_ACCOUNTING: CostAccounting::{PerTurn, Cumulative}` (default `PerTurn`; the agent epic flips it if the live probe shows cumulative reporting, in which case the delta is the increase over the previous `result` of this run); pass it to `append_native_line`; `registry.clear_prompt(sid)`; call the `on_result` hook (ephemeral end-of-run, next task).
- [ ] `prompt` event committed at `seq` → `registry.set_prompt(sid, seq, prompt_id)`.
- [ ] Adoption restores translation state before live tailing: restart after a committed subagent call and before its result still emits exactly one matching `subagent_end`; a delayed echo of a recorded input remains suppressed. Replaying committed history writes no events or counters. Test cumulative accounting across adoption when that mode is selected, with the previous result's baseline restored.
- [ ] `OwnerCommand::Shutdown` exits the loop cleanly after finishing the line in progress.
- [ ] The owner never logs event payloads at `info` or above and never holds the registry mutex across an `.await`.

## Implementation notes
- Files: `orchestrator/src/session/owner.rs`, `orchestrator/src/session/mod.rs`.
- Loop shape: `tokio::select!` over the tail interval, `commands.recv()`, and (next task) the container wait future; keep the tail step as `async fn read_available_lines(&mut self) -> Result<Vec<(String, u64)>>` so tests can drive it directly.
- Translation state belongs to the CLI process, not the owner task. Before adoption or owner restart tails beyond the committed offset, reconstruct the active process's open subagent calls, denied-tool bookkeeping and input-echo hashes from retained transcript/history. Restore the last accounted cumulative result when the probe selects cumulative cost accounting. Reconstruction must respect process-launch boundaries, discard replayed output, and neither append events nor add costs nor resend inputs. Fresh process launches, including resume/retry, start fresh translation state. See `ARCHITECTURE.md`, "Durability and recovery"; this does not add durable input delivery (ADR 0020).
- Byte offsets are `u64` from the file, stored in `_offset` as a JSON number; `MAX((payload->>'_offset')::bigint)` is the repository's read.

## Edge cases
- The file is truncated/replaced (offset beyond EOF): log `warn!`, reset the read position to EOF and continue; do not rewind to 0 (that would duplicate history).
- A line longer than 16 MiB: translate anyway; the adapter truncates tool results at 256 KiB per event.
- `init` arrives while the session is already `running` (CLI restarted inside the container): `transition` returns a conflict; treat as non-fatal, log `warn!`, still store `cli_session_id`.
- Two `init` events in one run (resume echoes): only the first transitions.

## Testing
- `orchestrator/tests/session_owner.rs` (`TestApp`, mock engine, `tempfile` data dir, the fixture transcript `tests/fixtures/claude/<pinned>/conversation.jsonl` from the agent epic or a minimal stream-json fixture committed here):
  - Feed lines one at a time with `\n`; assert `events.seq` is `1..=n` contiguous and `MAX(_offset)` equals the file length after each line.
  - Write half a line without `\n`; assert nothing is committed; complete it; assert one batch.
  - Kill mid-file: after line k commits, send `Shutdown` (and separately `abort()` the JoinHandle), spawn a new owner with `start_offset = max_offset(sid)`, append the remaining lines; assert `COUNT(*) = MAX(seq)`, every `_offset` distinct and increasing, and the multiset of `kind`s equals the fixture's expected sequence (no gaps, no duplicates).
  - `init` fixture line moves `creating → running`, stores `cli_session_id`, emits `state_change`, and emits `launch_warning` when `mcp_servers` lacks `mars-orchestrator`.
  - Two `result` lines accumulate `cost_usd`, `input_tokens`, `output_tokens` per the `PerTurn` rule and advance `last_activity_at`.
  - `prompt` fixture line registers the pending prompt in the registry.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `ARCHITECTURE.md` "Cost accounting": if the agent epic has already resolved open question 5, set `COST_ACCOUNTING` accordingly; otherwise leave the document untouched and the constant at `PerTurn` with a comment citing the open question.

## Assumes from other epics
- "Claude Code agent backend and event translation" delivers `AgentBackend::translate`, `TranslateState`, `AgentEvent` (with `init`, `result`, `prompt` payload types) and the fixture transcripts.
- "Database schema, models, repositories and test harness" delivers `TestApp::spawn()` with a mock engine and a fixed data directory.

## Correction: `running` on stdin attach, not on `init` (ADR 0032, task 3z8xu)
The pinned CLI writes nothing, `init` included, until its first stdin line (`ARCHITECTURE.md`, "Launch sequence"; `docs/decisions/0032-run-state-on-stdin-attach.md`). Where the text above disagrees, this section wins.
- Handling `system`/`init` means `set_cli_session_id` and the `launch_warning` rule only. No `creating`/`parked` to `running` transition, no `registry.mark_running` and no queue drain happen on `init`; those belong to the launcher at stdin attach. An `init` for a session that is not `running` is logged at `warn!` and still stores the id.
- Tests: drop the `creating -> running` transition and its `state_change` from the `init` fixture assertion; add one that a session already `running` with `cli_session_id` null gets it set by the first `init`, and that a later `init` of the same process changes nothing.
