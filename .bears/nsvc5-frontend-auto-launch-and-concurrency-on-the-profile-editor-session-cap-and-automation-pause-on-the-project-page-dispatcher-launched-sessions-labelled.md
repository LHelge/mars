---
id: nsvc5
title: "Frontend: auto-launch and concurrency on the profile editor, session cap and automation pause on the project page, dispatcher-launched sessions labelled"
status: open
priority: P1
created: "2026-09-21T20:24:30.914974391Z"
updated: "2026-09-21T20:24:30.914974391Z"
tags:
  - frontend
  - dispatcher
  - profiles
  - projects
depends_on:
  - vx7sq
parent: qabvt
---

## Summary
The controls for the fields `vx7sq` adds (`SPEC.md`, profile and project endpoints; "Frontend"). Invoke the `/frontend-design` skill first (CLAUDE.md, "Frontend conventions"); the tone is the operator console, and a paused project is state worth its quiet colour.

## Acceptance criteria
- [ ] `src/types/` mirrors the new fields exactly (`auto_launch`, `max_concurrent`, `max_concurrent_sessions`, `automation_paused`, and `launch_source` on `Session`), `snake_case`.
- [ ] Profile editor: an auto-launch toggle and a max-concurrent number field, shown only for `ephemeral` profiles (switching kind to conversational clears auto-launch before submit). The server's 400 for a missing agent credential is shown in its own words with a pointer to where a project or global credential is set up; reuse the existing credential visibility from the agent-credentials work rather than a second lookup if it already tells the editor the answer.
- [ ] Project page: a nullable session-cap field and an "automation paused" toggle; while paused, the project header (or wherever project state is shown) says so.
- [ ] A session with `launch_source: "dispatcher"` is labelled as launched by the dispatcher wherever the launching user is otherwise shown (session list, session header); `"schedule"` gets its label here too, as a union type handled exhaustively, so `k73td` has nothing to add for it. Never infer automation from `created_by: null` — a deleted user produces the same null and keeps whatever it renders as today.
- [ ] All calls through `src/services/`; forms through `useFormSubmit()`; new `data-testid`s are constants in `src/utils/testIds.ts`.

## Testing
Vitest beside any new pure logic (kind/auto-launch coupling, payload building). The Playwright scenario is the epic's last task.