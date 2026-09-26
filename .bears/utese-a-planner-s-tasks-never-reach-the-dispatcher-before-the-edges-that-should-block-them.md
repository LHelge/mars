---
id: utese
title: A planner's tasks never reach the dispatcher before the edges that should block them
type: epic
status: open
priority: P1
created: "2026-09-26T20:45:39.079911150Z"
updated: "2026-09-26T20:45:39.079911150Z"
tags:
  - mcp
  - tracker
  - dispatcher
  - profiles
  - orchestrator
---

## Problem

A planner session breaking a task down files its sub-tasks one `create_task` call at a time. The dispatcher (`ARCHITECTURE.md`, "Dispatcher") wakes on every `task_events` notification after a 250 ms debounce, so a sub-task filed in `ready` is claimable and usually claimed within a second, while the planner's next MCP call is seconds away. If that next call files the prerequisite, or adds a `blocks` edge with `update`'s `add_depends_on`, the dependant has already been dispatched from a graph that was not finished.

Nothing else holds it back:

- ADR 0052's `author_work_unlanded` gate only waits for a planner's *commits*. The `planner` template (`SPEC.md`, "Role profile templates" → `planner`; `orchestrator/src/projects/templates/planner.md`) says "You do not write or change code", so a pure-tracker planner's tip is its `base_commit`, which counts as landed.
- A planner cannot file in `backlog` and promote later: `update`'s creator exception (`SPEC.md`, "MCP tool contracts" → `update`) covers `title`, `description`, `labels` and dependency edits, not `state`.
- The template says "ordered with `depends_on`" but not that prerequisites must be filed first.

What already works: `create_task` with `depends_on` creates the task and its edges in one `TrackerMutation`, nothing broadcast before commit (`ARCHITECTURE.md`, "Task tracker"). The race opens only when a dependant is filed before its prerequisite, or an edge is added later to a task already in `ready`.

## Plan

1. Prompt fix now: the planner files in dependency order with `depends_on` at creation.
2. Real fix: an atomic `create_plan` MCP tool that files a parent, its sub-tasks and their edges as one mutation, recorded in a new ADR.
3. The planner template switches to `create_plan`.

## Rejected

Widening ADR 0052's gate to "author session still live": planners are conversational and park rather than end, so their tasks would wait indefinitely. Record this in the ADR of the `create_plan` task.