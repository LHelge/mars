---
id: qgw4z
title: "Planner template: file prerequisites first with depends_on at creation, never add a blocking edge to a ready task"
status: open
priority: P1
created: "2026-09-26T20:45:58.877451660Z"
updated: "2026-09-26T20:45:58.877451660Z"
tags:
  - profiles
  - docs
  - orchestrator
parent: utese
---

Implements the stopgap of epic `utese`. Cites `SPEC.md`, "Role profile templates" → `planner`, and `ARCHITECTURE.md`, "Dispatcher".

## What

Change the `planner` prompt in `orchestrator/src/projects/templates/planner.md` and the copy quoted in `SPEC.md`, "Role profile templates" → `planner`, in the same commit (CLAUDE.md rule 1). Add to the paragraph that starts "Then write the plan into the task tracker", in the template's own voice:

- File prerequisites before the tasks that depend on them, and give each task its `depends_on` in the `create_task` call that creates it. A task in `ready` may be picked up by an implementer as soon as it is filed.
- Never add a blocking dependency with `update` to a task that is already in `ready`. If the order is still unclear, file the task in `backlog`.

## Why

The dispatcher claims a `ready` task within about a second of the `task_events` notification, and ADR 0052's gate does not hold a planner that makes no commits. See the epic body.

## Done when

- The template file and `SPEC.md` quote the same text. Any test that compares the seeded prompt against the file or a snapshot still passes (grep `tests/` for `planner.md` or the template-loading function).
- Existing projects keep the prompt they were seeded with (ADR 0051 seeds at project creation). Say so in the commit message. Don't write a migration to rewrite stored profiles unless `ARCHITECTURE.md` already specifies one for template changes.

## Edge cases

- `backlog` is not a dispatch state, but the planner cannot promote its own task later (the creator exception on `update` excludes `state`). The wording must not suggest it can.