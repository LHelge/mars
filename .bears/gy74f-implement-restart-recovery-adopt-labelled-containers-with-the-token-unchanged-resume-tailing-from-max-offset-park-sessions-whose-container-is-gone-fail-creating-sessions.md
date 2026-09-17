---
id: gy74f
title: "Implement restart recovery: adopt labelled containers with the token unchanged, resume tailing from MAX(_offset), park sessions whose container is gone, fail creating sessions"
status: open
priority: P1
created: "2026-09-16T20:33:19.244397462Z"
updated: "2026-09-17T04:57:45.226464470Z"
tags:
  - orchestrator
  - sessions
  - engine
depends_on:
  - bt9q6
parent: s52qg
---

## Summary
Implement `orchestrator/src/session/recovery.rs` and wire it into `main.rs` between the engine startup probe and the cron service: list containers labelled `mars.session_id`, re-create a `SessionOwner` for every `running` session whose container is alive (reattach stdin, tail from `MAX(_offset)`, keep the MCP token hash and `mcp.json` untouched), mark `running` sessions without a container `parked` with a `state_change`, and mark every `creating` session `failed` with the documented reason.

## Documents
- `ARCHITECTURE.md` "Restart procedure" (steps 1–4 verbatim: migrations; list by label, re-create owner, reattach stdin, resume from `MAX(_offset)`, adoption leaves token hash and config unchanged, container gone → `parked` with a `state_change` saying so; `creating` → `failed` with reason `orchestrator restarted during creation`; then cron), "Durability and recovery", "Engine adapter" (list with label filter; startup probe runs before adopting any session), "MCP design" (adoption retains the token), "Background jobs" (orphan cleanup removes containers whose session is parked/done/failed/missing; not recovery's job).
- `README.md` "Operating notes" (restart does not stop running containers; they are re-adopted).
- ADRs 0010, 0020, 0029.

## Acceptance criteria
- [ ] `pub async fn recover(state: &AppState) -> Result<RecoveryReport { adopted: usize, parked: usize, failed: usize }>` is called from `main.rs` after migrations and the engine probe and before `CronService` starts and before the listeners accept traffic (so a stale owner cannot race a new launch).
- [ ] Containers are listed once with the label filter `mars.session_id`; the map is keyed by the label value parsed as `Uuid` (unparseable labels are logged and skipped).
- [ ] For each session in `running`: container present and running → `engine.attach_stdin`, `registry.register(sid, kind, Phase::Running)` (an adopted owner is already past `init`), `SessionOwner::spawn` with `start_offset = max_offset(sid)`, `resumed = true` is irrelevant (no init expected), `container_id` as recorded, and no call to `rotate_token`/`write_mcp_json`; container present but exited → run the owner's exit path (drain file to EOF, apply the exit rule, remove container) — implement by spawning the owner and letting `engine.wait` return immediately; container absent → `transition(running → parked, reason = "container missing after orchestrator restart")`, `set_container_id(None)`.
- [ ] `fail_all_creating("orchestrator restarted during creation")`; for each failed session remove any container carrying its label and invoke the `on_session_ended` hook (releases a task claimed at launch).
- [ ] Sessions in `parked`, `done` or `failed` are not touched, whatever the engine lists; stray containers are left to the orphan-cleanup job.
- [ ] Every outcome is logged at `info` with `session_id` and the action (`adopted`, `parked`, `failed`); the report totals are logged once.
- [ ] A failure adopting one session (engine error on attach) marks that session `parked` with `reason = "could not reattach after restart: <engine message>"` and continues with the others; `recover` returns `Ok` unless listing containers itself fails, which is fatal at startup.

## Implementation notes
- Files: `orchestrator/src/session/recovery.rs`, `orchestrator/src/session/mod.rs`, `orchestrator/src/main.rs`.
- Reuse the owner-spawn helper from the launcher, with an explicit adoption mode: restore the existing process's translation state through `qtx4x` before tailing beyond `start_offset`. Fresh launches reset state; adoption restores open subagents, denied-tool bookkeeping, input-echo hashes and any cumulative-cost baseline. Restoration publishes no events, increments no counters and resends no input.
- The `mcp_token_hash` must be read before and after adoption in tests to prove it is unchanged; the code simply never writes it.
- Also expose `recover` on `TestApp` (`app.recover().await`) so tests can simulate a restart against the same database and data directory after clearing the registry.

## Edge cases
- Two containers with the same session label (crash between create and remove on a resume): adopt the running one, remove the others.
- A session `running` in the database but the engine reports the container as `created`/not started: treat as gone (park and remove the container).
- `MAX(_offset)` beyond the current file size (file replaced): the owner's tail handles it (seek to EOF with a warning).
- Adoption tests also restart between a subagent call and its result and before a delayed input echo; assert a matching `subagent_end`, suppressed echo, unchanged committed history and no double-counted costs.
- Recovery must not wait for `init`; the registry phase is `Running` immediately and queued inputs from before the restart are gone (ADR 0020).

## Testing
- `orchestrator/tests/session_recovery.rs` with `TestApp`, the mock engine pre-seeded with listed containers, and a `tempfile` data dir: (1) a `running` session with a listed running container and 5 committed events at offset X — after `recover`, the registry has a live owner, `mcp_token_hash` and `mcp.json` bytes are unchanged, appending lines to `stream.jsonl` produces events `6..` with distinct increasing `_offset` and no duplicates of lines before X; (2) a `running` session with no container → `parked`, `state_change` with the exact reason, `container_id` null; (3) a `creating` session → `failed` with `error = "orchestrator restarted during creation"`, its labelled container removed, hook invoked; (4) a `parked` session with a stray container → untouched; (5) attach failure → parked with the reattach reason, other sessions still adopted; (6) a listed container whose engine state is exited with code 0 → `parked` with `reason = "CLI exited"`.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Container engine adapter": `list_by_label`, `inspect`/state, `attach_stdin`, `remove`, mock engine seeding of listed containers.
- "Repository scaffolding, tooling and CI": `main.rs` startup order (config, pool, migrations, probe, recovery, cron, listeners).
- "Task tracker": lease release through `on_session_ended`.