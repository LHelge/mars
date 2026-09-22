---
id: cyuj4
title: "Round limit: tasks.rounds and projects.max_rounds, counted on revision and enforced on send-back"
status: open
priority: P1
created: "2026-09-22T20:20:12.083051813Z"
updated: "2026-09-22T20:20:12.083051813Z"
tags:
  - orchestrator
  - tracker
parent: tykeu
---

Implements ADR 0046; the rules are `ARCHITECTURE.md`, "Task tracker" → "Rounds", `docs/data-model.md` (`projects.max_rounds`, `tasks.rounds`, the hand-off and send-back paragraphs under `tasks`, migration 11 `round_limit`) and `SPEC.md`, "Projects", "Tasks", "Code hand-offs and review", "TaskEvent". The documents are already written; change them only if implementing finds them wrong.

## What to do
- Migration `round_limit` (`sqlx migrate add -r round_limit`): `projects.max_rounds SMALLINT NOT NULL DEFAULT 5 CHECK 1–50`, `tasks.rounds SMALLINT NOT NULL DEFAULT 0`; the down migration drops both.
- `Project` gains `max_rounds`; `PUT /projects/{id}` accepts it with the exact 400 `max_rounds must be between 1 and 50`. `Task` gains `rounds` (REST, MCP task output, `TaskEvent.task`).
- In the tracker (`tracker/`, the crate-private state/lease writer and the hand-off path): a revision publication increments `rounds` in its own transaction; any state change whose *from* state is the human state resets it to 0.
- The send-back redirect: a forward recording `changes_requested`, by a **session or the system** (never a REST user), on a task whose `rounds >= max_rounds`, moves the task to the human state instead of the requested state. The hand-off row still records `changes_requested`; the forward's comment is written; `needs_human_reason` is `round limit reached (<rounds>/<max_rounds>)` followed by that comment; the event is `escalated` (not `state_changed`); the escalation email goes out through `tracker::commit_and_notify` as for every escalation.
- Expose the redirect as a tracker-level helper the auto-merge job can call for its conflict send-back (sibling task), so there is one place that decides it.

## Tests
Through the verbs, per `CLAUDE.md`, "Testing expectations" → tracker tests; arrange `rounds` through real revision publications in `tests/common/tracker.rs`. Cover: increment per revision and not per forward; reset on leaving the human state and not on other moves; the redirect at the limit for an MCP forward (state, reason, `escalated` event, email captured by the mock client, review status still recorded); no redirect below the limit; no redirect for a REST user's forward; `max_rounds` validation on `PUT`.

## Done when
The backend quality chain in `CLAUDE.md` passes and `.sqlx/` is regenerated.