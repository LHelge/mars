---
id: qhyhw
title: Let a session that is still creating be ended, and never leave its container running after a refused or early end
status: done
priority: P2
created: "2026-09-20T18:38:00.594595523Z"
updated: "2026-09-20T21:25:08.273470318Z"
tags:
  - orchestrator
  - sessions
  - frontend
parent: "6s8j7"
attempts: 1
---

## Summary
Found by the 2acdq session E2E specs. `POST /sessions/{id}/end` on a session in `creating` answers `409 {"status":409,"error":"session is creating"}`, while `frontend/src/session/SessionActions.tsx` renders the End button for `creating` and `ARCHITECTURE.md`, "Session lifecycle", describes `done` as "ended by a user or policy" without excluding `creating`. The refused end does not stop the launch either: the container starts afterwards. Probe: create → `state: creating` → `POST /end` → 409 → 5 s later `state: running`, container up. In the E2E run the title-defaults scenario left three such containers per pass until the test helper learned to wait out `creating` (`endSession` in `frontend/tests/utils/resources.ts`).

## Documents
- `SPEC.md` "Sessions" (`POST /sessions/{id}/end`, status codes); `ARCHITECTURE.md` "Session lifecycle" (state table and transitions), "Recovery".

## Acceptance criteria
- [ ] Decide and document the contract in `SPEC.md` and `ARCHITECTURE.md`: either `end` is accepted in `creating` (the launch is cancelled or the container removed as soon as it exists, and the session reaches `done`), or it is refused with 409 and the UI does not offer End while `creating`. Prefer the first: a user who launched by mistake should not have to wait for the container.
- [ ] Whichever is chosen, no path leaves a container running for a session the user was told could not be ended or was ended: an integration test with the mock engine ends a session during `creating` and asserts the final state and that no container remains.
- [ ] `SessionActions.tsx` matches the contract; a unit test covers the button's availability per state.
- [ ] `frontend/tests/utils/resources.ts` `endSession` loses its wait-out-`creating` workaround if `end` becomes accepted, and `sessions.spec.ts` gains a scenario ending a session right after launch.
- [ ] Backend and frontend quality chains pass.
