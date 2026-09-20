---
id: gn4y2
title: Frontend task board, task detail and hand-off controls
type: epic
status: done
priority: P1
created: "2026-09-16T20:14:48.263481977Z"
updated: "2026-09-20T11:38:48.923490279Z"
tags:
  - frontend
  - tracker
depends_on:
  - "2f5u2"
  - h8kw9
  - xjaah
---

## Scope

The per-project board and everything in the task drawer.

- `taskStore` (Zustand) holding the authoritative REST snapshot of states and tasks; `useTaskStream` opening SSE with `?after=lastSeq`, waiting for `open` before the initial load, explicit reconnect with a refreshed token, and the board refresh ordering rules (single in-flight refresh, view and event generations, coalesced follow-up, stale responses discarded, failures keep the previous snapshot) per ADR 0022.
- `TaskBoard` with columns in `position` order, `TaskCard` (priority, labels, holder link, attempts above one, assignee, blocked and dependency indicators, parent badge), local search by title substring or exact `#number` with `No matching tasks` (ADR 0031), create-task form.
- `TaskDetail` drawer at `/projects/:id/tasks/:number`: description, comments with system comments styled apart, dependencies by kind with add/remove, children, sessions that touched the task, actions: move to state, release, "open in session" and "run once" with profile choice and base-override disclosure, delete, `Copy link`.
- Hand-off controls: current hand-off summary (source session, branch, commit, comment, review status), history, revision form (source session, exact commit, comment, target state), review actions forwarding the current id with `approved`/`changes_requested`, task merge enabled only for an approved current hand-off, revision diff via `handoff_id`.
- `TaskStatesEditor`: list, add, rename, reorder, delete with API refusals shown as disabled actions.

## Documents

`SPEC.md` "Frontend" (task board, search, board refresh ordering, hand-off controls, copy links), "User-facing features" (Task board); `ARCHITECTURE.md` "Frontend architecture"; ADRs 0022, 0031.

## Acceptance criteria

- [ ] Refresh ordering is unit-tested: an event arriving mid-load triggers exactly one coalesced refresh; a failed load keeps the previous snapshot.
- [ ] Search matches `42` and `#42` to task 42 only, is case-insensitive on titles, survives refreshes and resets on project change.
- [ ] Every drawer action calls its endpoint through `services/` and the board refreshes on the resulting event.
- [ ] Lint, typecheck and build pass.

## Out of scope

Backend semantics (Task tracker and Code hand-offs epics).