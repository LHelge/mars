---
id: pekcb
title: "Role profiles: seed every new project with planner, implementer, reviewer and merger, and make the MCP server say it is the tracker"
type: epic
status: open
priority: P2
created: "2026-09-20T22:39:57.915346359Z"
updated: "2026-09-20T22:39:57.915346359Z"
tags:
  - profiles
  - mcp
  - orchestrator
  - frontend
---

## Scope
A new project starts with one `default` profile and no system prompt, and the MCP server's instructions are one sentence that assumes the agent already knows it is in Mars. Observed on 2026-09-21 on the live stack: a hand-made planner profile, in a repository whose `CLAUDE.md` mandates another task tracker over MCP, reported that tracker as missing instead of planning into Mars. Nothing had told it that Mars's tracker replaces the one the repository names.

This epic (1) makes the MCP server instructions say that, for every session whatever its profile, and (2) seeds each new project with four role profiles — `planner`, `implementer` (the default), `reviewer`, `merger` — matching the seeded states `backlog → ready → review → merge`, each with a system prompt that describes the job. The same four texts are offered as templates when creating a profile, which covers existing projects and restoring a deleted role.

## Constraints decided with the user
- The prompts are **generic**: they never name a specific third-party tracker or tool. The wording is "where the repository's instructions name a task tracker, use the Mars task tools instead".
- The repository's `CLAUDE.md` keeps governing *how* code is written and checked; the role prompt governs *what this agent's job is* (`ARCHITECTURE.md`, "Claude Code invocation", the non-bare split).
- Templates are **copied at project creation** and then belong to the project: editable, deletable, never rewritten by a later release. Existing projects are not migrated.
- No seeded ephemeral ("run once") profiles yet.
- The prompts are product behaviour: reproduced verbatim in `SPEC.md` like the tool descriptions, so a change to them is reviewed.

## Documents (written by the tasks, rule 1)
- `SPEC.md`: product overview "Agent profiles"; "Agent profiles" API section (seeding, template endpoint); "MCP tool contracts" (server instructions verbatim); a new "Role profile templates" section with the four prompts verbatim; "Frontend".
- `ARCHITECTURE.md`: "Task tracker" (the paragraph that already describes planner/implementer/reviewer/merger as roles), "MCP design", "Claude Code invocation".
- `docs/data-model.md`: `agent_profiles` (what a new project is seeded with).
- `README.md`: "Start" walkthrough (launch from the planner, not "its default profile" only).
- A new ADR: seed role profiles at creation and copy rather than reference (rejected: template picker only; rejected: prompts referenced live from the binary).

## Acceptance criteria
- [ ] A session of any profile is told by the MCP server that it is the session's task tracker and replaces the one the repository names.
- [ ] A newly created project has the four role profiles with prompts, served states and tool allow-lists; `implementer` is the default.
- [ ] A profile can be created from a template in the UI, in new and existing projects.
- [ ] No prompt or instruction text names a specific external tracker.
- [ ] Both quality chains pass.
