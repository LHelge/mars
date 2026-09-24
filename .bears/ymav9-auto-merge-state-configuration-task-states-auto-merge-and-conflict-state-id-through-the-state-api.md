---
id: ymav9
title: "Auto-merge state configuration: task_states.auto_merge and conflict_state_id through the state API"
status: done
priority: P1
created: "2026-09-22T20:20:12.101563199Z"
updated: "2026-09-24T14:13:07.079074574Z"
tags:
  - orchestrator
  - tracker
parent: tykeu
attempts: 1
---

Implements the configuration half of ADR 0045: `docs/data-model.md`, `task_states` (the two columns, both `CHECK`s, the `ON DELETE RESTRICT` and the deletion rule; migration 12 `auto_merge_states`) and `SPEC.md`, "Task states" (fields, the `PUT` pairing rule, the exact 400/409 messages). Documents already written.

## What to do
- Migration `auto_merge_states`: `task_states.auto_merge BOOLEAN NOT NULL DEFAULT FALSE`, `conflict_state_id UUID NULL REFERENCES task_states(id) ON DELETE RESTRICT`, `CHECK (auto_merge = (conflict_state_id IS NOT NULL))`, `CHECK (NOT auto_merge OR kind = 'queue')`. Existing rows stay `false`; the down migration reverses it fully.
- Model validation with the per-model error enum: queue-only, pair required together, conflict state a `queue` state of the same project and not the state itself — with the exact messages of `SPEC.md`, "Task states".
- `TaskState` gains `auto_merge` and `conflict_state` (the *name*, or null). `POST` and `PUT` accept both; on `PUT` either field replaces both, `auto_merge: false` clears the conflict state, neither leaves them alone. Every change emits `states_changed` as today, through the tracker mutation.
- Deletion refuses a state that another state names as its conflict state with 409 `state is the conflict state of <name>`, checked by the repository before the `DELETE`. Renaming the conflict state needs nothing (it is by id).
- Do **not** change project seeding here; that is its own task.

## Tests
Integration tests per endpoint path: happy path, each 400 message, the 409 on deletion, `states_changed` emitted, unauthenticated/forbidden as for the other state endpoints. A migration down/up round trip if the suite has a pattern for it.

## Done when
Backend quality chain passes, `.sqlx/` regenerated.