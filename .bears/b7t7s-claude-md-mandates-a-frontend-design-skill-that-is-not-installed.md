---
id: b7t7s
title: CLAUDE.md mandates a /frontend-design skill that is not installed
status: open
priority: P3
created: "2026-09-21T09:37:47.248791061Z"
updated: "2026-09-21T09:37:47.248791061Z"
tags:
  - docs
  - infra
  - frontend
---

## Summary
`CLAUDE.md`, "Frontend conventions" says: "Invoke the `/frontend-design` skill before creating or reshaping UI." No such skill exists in `.claude/skills/` (only `bears-planning` and `implement-epic`), and the Skill tool answers `Unknown skill: frontend-design` in both the coordinating session and a `task-implementer` worktree. Discovered while implementing br9h4 (epic pekcb), whose agent fell back to the tone sentence of the same bullet.

## Documents
- `CLAUDE.md`, "Frontend conventions"

## Acceptance criteria
- [ ] Either the skill is added under `.claude/skills/frontend-design/` (or the plugin that provides it is recorded in the project settings so every clone and worktree has it), or the line in `CLAUDE.md` is rewritten to carry the design guidance itself.
- [ ] A `task-implementer` dispatched for a frontend task can follow the instruction as written.

## Notes
- Decision is the user's: the skill may be a user-level plugin that was installed on another machine.
