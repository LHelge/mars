---
id: tykeu
title: Automatic task merges in the orchestrator, and a per-task limit on implementation rounds
type: epic
status: done
priority: P1
created: "2026-09-22T20:16:49.503009094Z"
updated: "2026-09-24T16:23:50.791314946Z"
tags:
  - orchestrator
  - frontend
  - tracker
  - git
  - automation
---

## Scope
Two related tracker features, designed with the user on 2026-09-22 and written into the documents before any code (ADR 0045, ADR 0046):

- **Auto-merge** (`ARCHITECTURE.md`, "Task tracker" → "Automatic merges"; `SPEC.md`, "Task states"; `docs/data-model.md`, `task_states`). A `queue` state can be marked `auto_merge` with a `conflict_state`. An orchestrator job, not an agent, merges every unheld, unblocked task in such a state whose current hand-off is approved into the project's default branch and moves it to the first terminal state; a conflict moves it to the conflict state with the conflicting paths. New projects seed `merge` with auto-merge on (conflict state `ready`) and three agent profiles; the `merger` template stays available but is no longer seeded. Existing projects are not migrated.
- **Round limit** (`ARCHITECTURE.md`, "Task tracker" → "Rounds"; `docs/data-model.md`, `tasks.rounds`, `projects.max_rounds`). `tasks.rounds` counts revision hand-offs since the task last left the human state. A send-back — a forwarded hand-off recording `changes_requested`, or an auto-merge conflict — of a task at `projects.max_rounds` (default 5) by a session or the system goes to the human state instead. `attempts` keeps its meaning.

## Decisions (settled with the user, 2026-09-22)
- Merge commits whenever git can make one; only a conflict sends a task back. Rejected: fast-forward only (every merge would invalidate every other approval).
- The merge is deterministic code, not an LLM session. Rejected: the seeded `merger` agent (ADR 0038's fourth role).
- Configuration lives on the state; the target is always the default branch.
- Push stays a manual action.
- The loop bound is a separate counter; `attempts` still resets on every state change. Rejected: not resetting `attempts` (escalation only fires on release, so it would not bound a hand-off loop, and it would weaken the failure cap).

## Acceptance criteria
- [ ] Every child task is done and the full quality chains pass.
- [ ] The documents written ahead of the code (ADRs 0045 and 0046, `ARCHITECTURE.md`, `SPEC.md`, `docs/data-model.md`, `README.md`) match what shipped; any deviation found while implementing updates them in the same commit.