---
id: m5jtw
title: Tracker update deadlocks against a session's event batch under load (40P01 on PUT /tasks/{n})
status: done
priority: P1
created: "2026-09-21T16:20:17.645713501Z"
updated: "2026-09-21T18:14:12.259560621Z"
tags:
  - orchestrator
  - bug
  - tracker
  - locking
parent: "579dz"
attempts: 1
---

Problem: found while implementing 4srw8 (2026-09-21), in a Playwright run on a loaded machine (five E2E stacks on 4 cores). `tests/handoffs.spec.ts` › `a review of a superseded revision says the hand-off changed` failed in its arrangement: `PUT /api/projects/{id}/tasks/1` answered 500, and the orchestrator log named a Postgres deadlock, SQLSTATE 40P01, two backends waiting on the same `sessions` row in opposite order (reported as `FOR KEY SHARE`, i.e. the foreign-key check of a row that references `sessions`). The log was deleted with the stack, so the exact statements are not recorded; the agent's reading was a lock-ordering race between the tracker/session-link path and another writer of the session row.

Hypothesis to confirm first: a tracker mutation takes the project row lock and then touches the session row (a session link, lease or hand-off insert whose FK check takes KEY SHARE on `sessions`), while the session owner's event batch holds the session row lock (`FOR UPDATE`/`NO KEY UPDATE`) and then reaches for the project row or a task row (lease release, a tracker verb run on behalf of the session, a state_change that parks/ends). ARCHITECTURE.md, "Task tracker" and "Event delivery" fix "one project row lock per tracker mutation, one session row lock per event batch, any git lock before any database lock" but the order between the project lock and the session lock is what this deadlock is about.

Acceptance: reproduce with a test that runs a tracker mutation naming a session concurrently with that session's event batch / state change (loop until it interleaves, or use two explicit transactions) and fails on 40P01 today. Establish and document one order between the project row lock and the session row lock (ARCHITECTURE.md, "Task tracker" / "Event delivery"; an ADR if an alternative is rejected) and make every path follow it; or, where an order cannot be imposed, retry the transaction on 40P01 a bounded number of times instead of answering 500. No 500 for a serialisation/deadlock failure reaches a client without a retry. Tests at the `TrackerMutation` seam per CLAUDE.md, "Tracker tests".

References: orchestrator/src/tracker/, orchestrator/src/repositories/tasks/, the session owner's event-batch write, orchestrator/src/routes/ tasks PUT handler. Contract: ARCHITECTURE.md, "Task tracker" and "Event delivery"; ADR 0021, ADR 0028. Discovered from: 4srw8 (epic 579dz).