---
id: p6fwn
title: Update the in-app help and hints for automatic merges and the round limit
status: done
priority: P2
created: "2026-09-22T21:18:35.910369047Z"
updated: "2026-09-24T16:23:50.756469739Z"
tags:
  - frontend
  - docs
depends_on:
  - "6w2qf"
  - vssu2
  - nq3dc
  - "4qa8f"
  - "23pue"
parent: tykeu
attempts: 1
---

The in-app help (epic gtbp5; `SPEC.md` "Frontend" → Help, topics in `frontend/src/help/*.md`, titles in `help/topics.ts`) describes Mars as it was before this epic. Once auto-merge, the new seeding and `max_rounds` are implemented, bring it in line — nothing here documents behaviour ahead of the code. Check each claim against `SPEC.md`/`ARCHITECTURE.md` as this epic left them (ADR 0045, 0046).

- `getting-started.md` "The starter profiles": the seeded set without `merger` (planner, implementer, reviewer) and a `merge` state with auto-merge on; a new project's reviewed work lands on the default branch by itself and waits there to be pushed.
- `task-flow.md`: auto-merge states (what moving an approved task into one does, the conflict state and its conflicting paths, that pausing automation pauses it), `max_rounds` (what counts as a round, the escalation to the human state, default 5) beside `max_attempts`; hand-offs: when the task's manual merge action is still needed.
- `automation.md`: auto-merge as a third unattended mechanism beside the dispatcher and schedules, and what the project's pause stops.
- `branches.md`: merged work waits on Mars's default branch until pushed.
- `profiles.md`: remove or reword anything that assumes a seeded `merger` or that only an agent merges.
- Hints with `help` links added by this epic's UI (states editor auto-merge controls, `max_rounds` in project settings, rounds on cards): give them one-line hints and `help="task-flow"` (or `automation`) in the gtbp5 style — a `help` prop on `FieldShell`/`SectionHeader`/`EmptyState`/profile `Fieldset`, else a `HelpLink` beside, never inside, the text an `aria-describedby` names.
- Keep the gtbp5 hints already on these screens accurate (e.g. `ProjectSettingsForm` max attempts, `tasks/` board and hand-off hints, `TaskMeta` attempts).
- Update `tests/help.spec.ts` and any unit test asserting changed strings; `SPEC.md` "Frontend" if it describes a changed text.