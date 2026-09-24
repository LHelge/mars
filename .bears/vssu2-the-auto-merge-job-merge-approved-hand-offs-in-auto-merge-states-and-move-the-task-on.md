---
id: vssu2
title: "The auto-merge job: merge approved hand-offs in auto_merge states and move the task on"
status: done
priority: P1
created: "2026-09-22T20:20:39.083803641Z"
updated: "2026-09-24T15:41:05.179478644Z"
tags:
  - orchestrator
  - git
  - tracker
  - cron
depends_on:
  - ymav9
  - cyuj4
parent: tykeu
attempts: 1
---

Implements ADR 0045's job. The behaviour, step by step, is `ARCHITECTURE.md`, "Task tracker" → "Automatic merges" (read it whole; it defines the ancestor check, the comments' exact wording, the race with a person's move and the failure rules), plus the `auto-merge` row of "Background jobs", the `Requested-By: system` trailer in "Git model" → "Commit identity", and `SPEC.md`, "TaskEvent" (`state_changed` with actor `system`; `escalated` for a missing approval).

## What to do
- A new job in `cron/` (e.g. `cron/auto_merge.rs`): one-minute timer, single-flight, woken by the same notifications as the dispatcher (any `task_events`, the listener resync) through a waker on the shared listener fan-out like `cron/`'s dispatcher waker — reuse or generalise that waker rather than copying it. Log no-work at `debug`.
- Per run: `ready`, non-`automation_paused` projects; candidates are unheld, unblocked tasks in `auto_merge` states, ordered by priority then number, as a lock-free read.
- Per task, under the project git lock held through the tracker commit: re-read; escalate a task without an approved current hand-off (reason text in `ARCHITECTURE.md`); ancestor check of the hand-off commit against the default branch; otherwise the existing task merge as `GitActor::System`; then one tracker mutation that re-checks state and current hand-off and moves to the first terminal state (comment `Merged <commit> into <branch>.`) or, on conflict, to `conflict_state` with the paths — through the round-limit send-back helper from `cyuj4`, so a conflict at the limit escalates. A task that left the state meanwhile gets the "merged, but the task had left" comment and no move.
- `commit_identity` / trailer: add the `system` actor form.
- Non-conflict merge failures: log at `error`, leave the task, retry next run.

## Tests
Integration tests with real bare repositories (`CLAUDE.md`, git is never mocked) and the job invoked directly: approved hand-off merges and closes (parent closure and dependant unblocking follow); conflict sends back with paths and leaves the target unchanged; conflict at the round limit escalates; unapproved or missing hand-off escalates; held and blocked tasks are skipped; paused project skipped; already-merged commit (simulated crash) only moves the task; a task moved out between merge and tracker commit gets the comment and keeps its state. Add a test-only route to run one pass only if the E2E task needs it (the wake-up should make it unnecessary).

## Done when
Backend quality chain passes; `ARCHITECTURE.md` matches what was built.