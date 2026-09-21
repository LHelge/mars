---
id: kz9mx
title: Intermittent 500 from PUT /projects/{pid}/tasks/{n} with a revision hand-off in the Playwright suite
status: done
priority: P2
created: "2026-09-21T06:01:32.790150544Z"
updated: "2026-09-21T17:35:30.445702072Z"
tags:
  - orchestrator
  - tracker
  - git
  - tests
  - flaky
---

## Summary
Seen once on 2026-09-21 while verifying round 1 of epic rdkmk (main at ba0ed19, full `npm run test:e2e`, 91 passed / 1 failed): `tests/handoffs.spec.ts:684` "the merge control is shut without an approval and a superseded review is refused" failed in its arrangement, `publishRevision` → `PUT /projects/{pid}/tasks/1` with `handoff: { kind: "revision", ... }` answered `500 {"status":500,"error":"internal error"}`. The whole spec passed 9/9 on an immediate rerun, and the other scenarios using the same `publishRevision` helper passed in the failing run, so it is intermittent. The orchestrator log of the failing run was removed by `test:e2e:down` (no `E2E_KEEP_LOG`), so the `tracing::error!` behind the 500 is unknown.

## Documents
- `ARCHITECTURE.md` "Task tracker", code hand-offs (publication: fetch-back from the session's work clone, git lock before database lock)
- `SPEC.md` "Tasks" (`PUT /projects/{pid}/tasks/{n}` with `handoff`)

## Acceptance criteria
- [ ] Reproduce (e.g. `E2E_KEEP_LOG=1 npx playwright test tests/handoffs.spec.ts --repeat-each=20`, under CPU/disk load) and capture the error line.
- [ ] A 500 here is a bug whatever the trigger: either fix the race (likely candidates: fetch-back racing the just-written commit in the work clone, or a lock-order/serialization failure surfacing as an internal error instead of a retry or 409) or map it to the documented status.
- [ ] A regression test at the level the cause lives at.

## Notes
- The machine was loaded at the time: three `task-implementer` agents were dispatched (and may have been compiling) during the second half of the run, so load is a plausible trigger. The rerun that passed was under the same or heavier load.
- Not related to the agent-credentials change: the failing call is a tracker mutation and the session was already `running`.
## Resolution (2026-09-21)
Closed with kb48s, which saw the same scenario (`tests/handoffs.spec.ts`, "the merge control is shut without an approval and a superseded review is refused") fail at the same call with the orchestrator log kept: a Postgres deadlock (`40P01`) between this tracker mutation and the session owner's transcript commit, surfacing as the generic 500. The cause and fix are ADR 0041 (tracker row locks are `FOR NO KEY UPDATE`); the regression tests are the two interleavings in `orchestrator/tests/tracker_mutation.rs`. This occurrence's own log was lost, so the identification rests on the identical scenario, call and status, not on its error line; reopen if the 500 is seen again after that change.
