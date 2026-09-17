---
id: kg8cx
title: "Architecture: the tracker mutation envelope is carried by the type, not by prose"
type: epic
status: open
priority: P1
created: "2026-09-17T19:59:12.787736980Z"
updated: "2026-09-17T19:59:12.787736980Z"
tags:
  - orchestrator
  - tracker
  - architecture
depends_on:
  - naqhy
  - thes7
---

## Why

The architecture survey (2026-09-17) found that the project row lock and `pg_notify` primitive are concentrated in one place (`TaskRepository::begin_mutation`, `append_task_events`) while the *obligation* to use them is stated 22 times as doc-comment warnings across `repositories/tasks/*.rs`. Every helper takes a bare `&mut PgConnection`, which a pool connection satisfies, so nothing stops a caller from mutating a task outside the project lock. `StateFields` is a seven-field bag that "decides nothing", so a terminal move that forgets `closed_at` or leaves the lease held compiles. The only executable specification of the tracker's verbs today is test helpers (`in_mutation`, `publish_handoff`) that re-implement the envelope.

The tracker epic (`5h3y4`) already plans the deep module: `tracker/` with `TrackerMutation` (`thes7`), `change_state` (`cuw5s`) and the lease verbs (`cws3a`). This epic makes the shallow path unreachable once the deep one exists, so that routes, MCP tools and cron jobs cannot bypass it, and moves the tests onto the verb interface.

## Scope

- Repository helpers that must run under the project lock take the mutation (or a connection token only `TrackerMutation` can produce) instead of `&mut PgConnection`; the prose warnings go.
- `StateFields` and `set_task_state_fields` are reachable only from `tracker/`.
- Tracker tests exercise the verbs through `TrackerMutation`; the test-side envelope helpers are deleted.
- Shallow file boundaries in `repositories/tasks/` and `models/` that fail the deletion test are folded.

## Documents

`ARCHITECTURE.md` "Task tracker" → "One mutation at a time per project"; `CLAUDE.md` "Backend conventions" (repository helpers and the caller's transaction) and "Testing expectations"; `docs/data-model.md` "Tracker mutation transactions"; ADRs 0021, 0028, 0030.

## Acceptance criteria

- [ ] No public function outside `tracker/` can change a task row's state, lease, attempts, closure or blocked flag.
- [ ] `grep -rn "inside a .*begin_mutation" orchestrator/src` finds nothing.
- [ ] No integration test opens `begin_mutation` itself; the lock and notify behaviour is asserted at the `TrackerMutation` seam only.

## Out of scope

The verbs themselves (tracker epic), hand-off publication (code hand-offs epic), MCP tools.