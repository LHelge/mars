---
id: kb48s
title: "Tracker mutation and session owner transcript commit can deadlock (40P01): the HTTP request answers 500 instead of retrying"
status: done
priority: P1
created: "2026-09-21T14:05:55.342238521Z"
updated: "2026-09-21T17:35:30.473439872Z"
tags:
  - orchestrator
  - tracker
  - session
  - flaky
attempts: 1
---

## Summary
Found while implementing qfc9t (epic qprj6) on 2026-09-21, in one frontend e2e run on Podman: `tests/handoffs.spec.ts` "the merge control is shut without an approval and a superseded review is refused" failed once because `PUT /api/projects/{id}/tasks/1` answered 500. The orchestrator log showed a Postgres deadlock (SQLSTATE 40P01) between the session owner's transcript-commit transaction and a tracker mutation:

```
Process 107 waits for ShareLock on transaction 1239; blocked by process 105.
Process 105 waits for ShareLock on transaction 1238; blocked by process 107.
… while locking tuple (0,117) in relation "sessions"
    SELECT 1 FROM ONLY "public"."sessions" x WHERE "id" = $1 FOR KEY SHARE OF x
```

The owner logged `could not commit a transcript line; retrying … deadlock detected` and retried; the HTTP request did not, and the user got a 500. Re-running the spec alone passed, and the coordinator's full e2e runs on `main` in the same epic passed, so it is a race, not a regression of that epic (no Rust source changed there).

## Documents
- `ARCHITECTURE.md`, "Task tracker" and "Event delivery": the lock order (one project row lock per tracker mutation, one session row lock per event batch, ADR 0021, 0028). If the fix changes or adds an ordering rule — session row relative to project row — it is stated there.

## Acceptance criteria
- [ ] The two transactions are identified and the cycle is explained: which rows each takes, in which order (the `FOR KEY SHARE` on `sessions` is a foreign-key check, presumably from a row the tracker mutation writes that references the session — a session link, a lease, an event — while the owner holds the session row lock and wants something the mutation holds).
- [ ] The cycle is removed by ordering (preferred: both paths take the locks in one documented order), not only papered over by a retry. If a retry on 40P01 is also added to `TrackerMutation`, it is bounded and documented.
- [ ] A test that reproduces the interleaving deterministically (or with a tight loop that failed before the fix) at the `TrackerMutation` seam (`tests/tracker_mutation.rs`) or the session owner suite.
- [ ] The backend quality chain passes.

## Notes
- The session in that scenario is one the task is linked to (review/merge hand-off flow), which is why the two paths meet on the same `sessions` row.
