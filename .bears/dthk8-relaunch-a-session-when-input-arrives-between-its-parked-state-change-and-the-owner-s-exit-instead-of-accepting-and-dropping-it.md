---
id: dthk8
title: Relaunch a session when input arrives between its parked state change and the owner's exit, instead of accepting and dropping it
status: done
priority: P1
created: "2026-09-20T18:37:52.058689806Z"
updated: "2026-09-20T20:10:16.698739930Z"
tags:
  - orchestrator
  - sessions
parent: "6s8j7"
attempts: 1
---

## Summary
Found by the 2acdq session E2E specs. A message sent to a session the instant it reads `parked` is answered 202 and silently dropped; the session is never relaunched and stays parked. The session owner writes the `parked` state change, then removes the container and clears `container_id`, and only when its task returns does `SessionOwner::spawn` call `registry.mark_parked`. An input arriving in that window finds `Phase::Running` with a still-open channel, so `submit_to` answers `Forwarded` and `send_input` never calls `launcher.launch(…, Resume)`. The window is about 300 ms under Podman (container removal); once `container_id` is null it is about 1 ms.

Evidence (probe: mid-turn stop, then `POST /sessions/{id}/input` the moment `GET /sessions/{id}` reads `parked`, four rounds): r1 running, r2 still parked after 90 s, r3 running, r4 still parked. Each lost input logged nothing at `info`.

## Documents
- `SPEC.md` "Sessions" (`POST /sessions/{id}/input` → 202, relaunches if parked).
- `ARCHITECTURE.md` "Session lifecycle" (`parked → running` on user message; state table "Container: removed" for `parked`), "Event delivery" (owner, registry). ADR 0020 covers loss on restart, not this.

## Acceptance criteria
- [ ] An input accepted with 202 for a session whose `parked` state change is committed is never dropped: it either reaches the running CLI or causes a relaunch with `Resume` that delivers it. Decide where the race closes (the registry phase changing in the same step as the state change, the owner draining its channel into a relaunch on exit, or `send_input` deciding from the row under the session lock) and say why in the commit; keep the locking discipline of `ARCHITECTURE.md` (one session row lock per event batch, git lock before database lock).
- [ ] An integration test with the mock engine reproduces the window deterministically (input submitted after the `parked` write and before the owner's exit) and asserts the session reaches `running` and the message appears in `events`.
- [ ] A dropped or re-routed input is logged with structured fields (`session_id = %id`), never the payload.
- [ ] `frontend/tests/sessions.spec.ts`, "sending a message to a parked session relaunches it": remove the `waitForContainerRemoved` wait that hides the window, so the scenario sends as soon as the row reads `parked`.
- [ ] `ARCHITECTURE.md` "Session lifecycle" states the guarantee; backend and frontend quality chains pass.
