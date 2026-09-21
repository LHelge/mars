---
id: rgrvp
title: Move session creation out of routes/sessions.rs into session/ so a launch needs no HTTP request and no user
status: open
priority: P1
created: "2026-09-21T20:07:41.516448919Z"
updated: "2026-09-21T20:07:41.516448919Z"
tags:
  - orchestrator
  - sessions
  - refactor
  - dispatcher
parent: qabvt
---

## Summary
"Through the same path as a user launch" (`ARCHITECTURE.md`, "After v1: dispatcher and scheduled agents") is not possible today: the whole of "Launching a session for a task" lives in the route module — `create`, `insert_claiming`, `LaunchBase`, `first_message`, `queue_first`, `validate_launch_prompt` and `default_title` in `orchestrator/src/routes/sessions.rs` — and takes a `user.id: Uuid`. Move it into `orchestrator/src/session/` behind one entry point that the route, the dispatcher and the scheduler all call. No behaviour change for a user launch.

## Acceptance criteria
- [ ] One function (on `SessionService` or a new `session/create.rs`) takes the project id, a validated launch request (profile id, optional task reference, optional message, optional title, optional `base_ref`) and a launch actor — `User(id)`, `Dispatcher` or `Schedule`, three variants because `vx7sq` stores which one as `sessions.launch_source` — and returns the `Session`. It keeps the documented order: project readiness, profile lookup, prompt validation, base resolution under the git lock before any database transaction, token generation, insert (claiming under `TrackerMutation` when there is a task), `Launcher::launch` (ADR 0021, ADR 0029).
- [ ] With `Dispatcher` or `Schedule` as actor: `sessions.created_by` is NULL, the mutation's `TaskActor` is `System`, and a caller `message` is queued with `user_id: None`.
- [ ] `routes/sessions.rs::create` is reduced to DTO parsing, the call, and the 201; its error answers (400 unknown profile, 400 unresolved base, 404 task, 409 not ready, 409 task is not claimable) are byte-for-byte unchanged.
- [ ] Whether served states are enforced is the caller's choice (a parameter or a separate check the dispatcher makes before calling); a user launch still ignores them.
- [ ] `ARCHITECTURE.md`, "Orchestrator internals" names the new home if its module layout lists the old one. `SPEC.md` does not change.

## Testing
- The existing session route tests pass unchanged — they are the proof of "no behaviour change".
- New integration tests drive the entry point directly with the orchestrator as actor: a task launch claims with a `System` `claimed` event and a NULL `created_by`; a task-less ephemeral launch with a message runs it as the prompt; a lost claim rolls the session row back.