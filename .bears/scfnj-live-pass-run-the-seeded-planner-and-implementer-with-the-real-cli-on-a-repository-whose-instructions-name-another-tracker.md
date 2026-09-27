---
id: scfnj
title: "Live pass: run the seeded planner and implementer with the real CLI on a repository whose instructions name another tracker"
status: open
priority: P2
created: "2026-09-20T22:41:31.937031852Z"
updated: "2026-09-27T08:38:42.308381560Z"
tags:
  - verification
  - profiles
  - mcp
depends_on:
  - vxcrn
  - pd6zy
parent: pekcb
---

## Summary
The prompts and the MCP instructions steer a model, which no automated test here exercises (the stub image ignores prompts). This is the manual pass that reproduces the original failure and shows it fixed, on the compose stack with the real Claude image and a real credential supplied by the operator.

## Documents
- `SPEC.md` "Role profile templates", "MCP tool contracts" (server instructions, `create_plan`)
- The epic body (the 2026-09-21 observation)
- ADR 0056 (epic `utese`)

## Acceptance criteria
- [ ] On a freshly created project for a repository whose `CLAUDE.md` mandates a different task tracker over MCP: a `planner` session asked to plan a small feature creates a parent task with sub-tasks in Mars (`parent`, `depends_on`, sensible states) and does **not** report a missing tracker, ask for one, or write task files into the repository.
- [ ] That planner files the multi-task plan with **one `create_plan` call** (task `na7y4`), not a sequence of `create_task` calls; its dependants appear on the board already `blocked`, and the dispatcher never launches an implementer on a dependant before its prerequisite is done.
- [ ] An `implementer` session launched for one of those tasks reads it, works, commits and hands off to `review` with a commit-bound hand-off and a comment.
- [ ] A `reviewer` session on that task either approves to `merge` or returns it to `ready` with concrete comments; a `merger` session merges an approved hand-off and closes the task.
- [ ] A hand-made profile with an **empty** system prompt in the same project still uses the Mars tools for tracking (this isolates the MCP instructions from the role prompts).
- [ ] Findings are recorded as a comment on this task; every prompt change they motivate is made in the template files and `SPEC.md` together, and anything larger becomes a new task linked to this one.

## Implementation notes
- Needs an operator-supplied credential; never commit it or paste it into a task (rule 3). Transcripts may contain it if the agent prints its environment (ADR 0027) — use a throw-away project.
- Rebuild the session image first (`podman build -t mars-session-claude:latest images/claude`); a stale image fails with "no stdin FIFO".
- Watch for the repository's own `.mcp.json` server failing to connect and whether the agent dwells on it; if it does, file the follow-up named in the MCP instructions task.

## Edge cases
- A repository with no tracker instructions at all should behave the same; run the planner once on such a repository too.
