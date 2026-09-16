---
id: s52qg
title: "Session lifecycle: launcher, owner, recovery and sessions API"
type: epic
status: open
priority: P1
created: "2026-09-16T20:13:19.124265874Z"
updated: "2026-09-16T20:15:29.852434868Z"
tags:
  - orchestrator
  - sessions
depends_on:
  - naqhy
  - pkaee
  - "8vnwy"
---

## Scope

`session/` and the sessions REST API: the core runtime of the product.

- Launcher: session row insert with fresh MCP token hash (ADR 0029), `mcp.json` written atomically, mirror fetch (skipped if < 30 s old, failure as `launch_warning`), base-ref resolution and clone, secrets resolution and the both-credentials refusal, container spec assembly (mounts including CLI state dir and shared dirs, env, labels), start and stdin attach; resume with a new token; `MARS_TASK_ID` and the generated task message when launched for a task (the tracker-side claim is provided by the Task tracker epic; this epic calls it in the same transaction).
- `SessionOwner` loop: tail `stream.jsonl` from the committed offset, translate, append events with `_offset` in one transaction per native line with `pg_notify`, record inputs as `user_message` before writing stdin, watch container exit, `state_change` events, `parked`/`failed`/`done` rules, `last_activity_at`, cost and token accumulation from `result`, ephemeral end-of-run (fetch-back, stop, `done`), `launch_warning` when `mars-orchestrator` is not connected in `init`.
- `SessionRegistry` with per-session input channels, queued inputs while `creating`/`parked`, relaunch on message to a parked session, answer rejection when the prompt was consumed.
- Stop semantics (`SIGINT`, `STOP_GRACE_SECS`, `SIGTERM`, signal recorded), end (stop, fetch-back, container removal, lease release hook), retry, sync, delete (directory and transcript removal).
- Recovery on restart: adopt containers by label, resume tailing from `MAX(_offset)`, mark missing containers `parked`, fail `creating` sessions.
- REST: all rows of the Sessions table, `GET /sessions` across projects, `GET /sessions/{id}/events` pagination, `POST .../input` (202), stop, end, retry, sync, `GET .../tasks`; title defaulting rules.
- Session `state` notify channel (`session_state`).

## Documents

`ARCHITECTURE.md` "Session owner task", "Session lifecycle", "Launch sequence", "Stop semantics", "Cost accounting", "Durability and recovery", "Restart procedure", "MCP design" (config file and token rotation); `SPEC.md` "Sessions"; `docs/data-model.md` `sessions`, `events`; ADRs 0003, 0010, 0020, 0021, 0028, 0029.

## Acceptance criteria

- [ ] Session owner tests feed a transcript line by line through the stub image or a file, kill and restart the owner mid-file, and assert `events` has no gaps and no duplicates.
- [ ] Integration tests with the mock engine cover every session endpoint's happy path and error paths (409 wrong state, 409 project not ready, 400 bad base ref, ephemeral refusals).
- [ ] An end-to-end run against the stub image on a real engine reaches `running`, accepts a message, parks on stop and resumes.
- [ ] Restart adoption is tested: a running container is re-adopted with the token unchanged; a `creating` session is failed.

## Out of scope

WebSocket/SSE delivery (Real-time delivery epic); idle and stuck reapers (Background jobs epic); tracker claim logic (Task tracker epic).