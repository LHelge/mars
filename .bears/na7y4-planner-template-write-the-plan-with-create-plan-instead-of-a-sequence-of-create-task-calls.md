---
id: na7y4
title: "Planner template: write the plan with create_plan instead of a sequence of create_task calls"
status: in_progress
priority: P2
created: "2026-09-26T20:46:05.970625615Z"
updated: "2026-09-27T08:31:26.128604448Z"
tags:
  - profiles
  - docs
  - orchestrator
depends_on:
  - qgw4z
  - egnkb
parent: utese
attempts: 1
---

The closing step of epic `utese`. Cites `SPEC.md`, "Role profile templates" → `planner`, and the `create_plan` section added by `egnkb`.

## What

Rewrite the "Then write the plan into the task tracker" paragraph of `orchestrator/src/projects/templates/planner.md`, and its quote in `SPEC.md`, so the planner files the whole plan with one `create_plan` call. Keep `create_task` for single follow-ups discovered later.

Keep the ordering rule from `qgw4z` for any task filed on its own. The rule about never adding a blocking edge to a `ready` task still applies.

## Done when

- The template and `SPEC.md` quote the same text.
- If the `planner` profile's tool gating limits which MCP tools it may call, `create_plan` is allowed.
- A seeded planner in a new project has the new prompt. As in `qgw4z`, existing projects keep theirs unless `ARCHITECTURE.md` says otherwise.
- Consider whether the live verification task `scfnj` should also exercise `create_plan`, and add a comment there if so.