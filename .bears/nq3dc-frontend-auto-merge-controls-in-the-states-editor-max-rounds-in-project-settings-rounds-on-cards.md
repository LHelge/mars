---
id: nq3dc
title: "Frontend: auto-merge controls in the states editor, max_rounds in project settings, rounds on cards"
status: done
priority: P2
created: "2026-09-22T20:20:39.104815081Z"
updated: "2026-09-24T15:41:05.205831152Z"
tags:
  - frontend
depends_on:
  - ymav9
  - cyuj4
parent: tykeu
attempts: 1
---

The UI of ADRs 0045 and 0046, as `SPEC.md`, "Frontend" → "Task board" now describes it; the API shapes are `SPEC.md`, "Projects" (`max_rounds`), "Task states" (`auto_merge`, `conflict_state`) and "Tasks" (`rounds`).

## What to do
- Types in `src/types/`: `TaskState.auto_merge`, `TaskState.conflict_state`, `Project.max_rounds`, `Task.rounds`, mirroring `SPEC.md` exactly.
- `TaskStatesEditor`: on each `queue` state an `Auto-merge` toggle and, while on, a `Conflict state` select (`FieldShell`, `CONTROL`) over the project's other queue states, saved together in one `PUT`; turning it on pre-selects `ready` if present, else the first other queue state. The delete action of a state that is another's conflict state is disabled with that reason. Server refusals through `useFormSubmit`'s `mapError`.
- Board column header: an `auto-merge` chip on auto-merge states (quiet colour, consistent with the other chips).
- Cards: `round n/max` when `rounds > 1`, beside attempts.
- Project settings form: `max_rounds` (1–50) beside `max_attempts`, same bounds handling.
- Help: if the help epic's "task flow and hand-offs" topic (`4qa8f`) has landed, add a short auto-merge and round-limit passage there; otherwise comment on `4qa8f` so it covers them.
- Invoke `/frontend-design` before reshaping the editor row.

## Tests
Vitest beside the helpers (pre-selection, the pairing sent on save, the disabled-delete reason, the card label). E2E is a separate task.

## Done when
Frontend quality chain in `CLAUDE.md` passes.