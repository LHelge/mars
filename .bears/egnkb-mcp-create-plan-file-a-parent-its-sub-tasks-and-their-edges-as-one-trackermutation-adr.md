---
id: egnkb
title: "MCP create_plan: file a parent, its sub-tasks and their edges as one TrackerMutation (ADR)"
status: open
priority: P1
created: "2026-09-26T20:45:58.896842248Z"
updated: "2026-09-26T20:45:58.896842248Z"
tags:
  - mcp
  - tracker
  - orchestrator
  - docs
parent: utese
---

The real fix of epic `utese`. Cites `SPEC.md`, "MCP tool contracts" (`create_task`, which this mirrors), and `ARCHITECTURE.md`, "Task tracker" (one project row lock per mutation, events broadcast only after commit, ADR 0021/0028) and "Dispatcher".

## What

A new MCP tool `create_plan` in `orchestrator/src/mcp/tools/`, running as one `TrackerMutation` in `tracker/`. The input is a tree:

- an optional parent (the same fields as `create_task`);
- a list of sub-tasks, each with a caller-chosen local `ref` string;
- dependencies that name either a local `ref` or an existing task reference (UUID, number or `#<number>`).

Everything is created under one project lock, in one transaction, with one batch of `TaskEvent` rows. The dispatcher's candidate read can therefore only see the finished graph. The output is `{ tasks: Task[] }` in input order, plus the `ref` → id mapping.

The rules match `create_task`: `state` defaults to the project's default state, `created_by_session_id` is set, the one-level `parent` rules and `discovered_from` provenance resolution apply, `blocks` edges are cycle-checked across the whole batch, and any validation failure creates nothing. Decide and specify the maximum batch size, local refs that are unknown or duplicated, and whether the parent may itself be an existing task, so a planner can add sub-tasks to the task it was launched for.

## Docs in the same commit

- `SPEC.md`, "MCP tool contracts": a `### create_plan` section with the description string, input, output and error messages, written in the style of `create_task`.
- A new ADR, `docs/decisions/0055-...` (check the next free number). It records the atomic plan tool and rejects (a) prompt ordering alone, which relies on the model following it; (b) widening ADR 0052's gate to "author session still live", because planners park; and (c) letting the creator promote its own task from `backlog`, which reopens the same race whenever the promotion order is wrong.
- `ARCHITECTURE.md`, "Dispatcher", if the rationale for coalescing wake-ups names the planner filing tasks one by one.
- Check whether the tool list or any profile tool gating (`SPEC.md` / `docs/data-model.md`) needs the new name.

## Tests

- MCP tests drive the handler through the in-process `rmcp` server with a session token from `TestApp`.
- Happy path; cycle inside the batch; unknown or duplicate local ref; a dependency on a task of another project; a failure part-way through leaves no tasks, edges or events.
- Assert at the `TrackerMutation` seam (`tests/tracker_mutation.rs`) that the batch is one transaction with events broadcast after commit.
- A dependant whose prerequisite is in the same batch is `blocked` in the first event the dispatcher could read.
- If the tool list the CLI sees changes, check whether `tests/mcp_conformance.rs` still passes.