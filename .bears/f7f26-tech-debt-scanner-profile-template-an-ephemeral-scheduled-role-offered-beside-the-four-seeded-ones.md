---
id: f7f26
title: "Tech-debt scanner profile template: an ephemeral scheduled role offered beside the four seeded ones"
status: open
priority: P3
created: "2026-09-21T20:25:49.180976570Z"
updated: "2026-09-21T20:25:49.180976570Z"
tags:
  - orchestrator
  - frontend
  - scheduler
  - profiles
  - spec
depends_on:
  - "6f9uu"
parent: tup8z
---

## Summary
The first instance of a scheduled agent that `ARCHITECTURE.md`, "Scheduled agents" names: a daily scan that files `ready` tasks and needs only `create_task`. Offer it as a fifth entry of `GET /profile-templates` (`orchestrator/src/projects/` `profile_templates`, `routes/profile_templates.rs`; `SPEC.md`, "Role profile templates") so a user gets a working scheduled profile from the editor's template picker. It is **not** seeded at project creation (ADR 0038 seeds the four queue roles only): a schedule that spends money should be something a person turned on.

## Acceptance criteria
- [ ] `ProfileTemplate` gains what a scheduled template needs (`schedule_cron`, `schedule_prompt`; `kind` is already reported) and `SPEC.md` documents the shape change and the template's full text — system prompt, schedule prompt, a once-a-day expression, tool list limited to what filing tasks needs, no served states if the model allows an empty list for a profile that serves no queue (otherwise say what it serves and why).
- [ ] The prompt tells the agent to search existing tasks before filing, to cap how many tasks one run files, to cite file and line, and to file into the state the project uses for unplanned work; it follows the conventions of the four role prompts (tracker named as the MCP server, no hard-coded state names beyond the template's defaults).
- [ ] The template picker pre-fills the schedule fields; the frontend's check that template states exist in the project still holds.
- [ ] Project creation still seeds exactly four profiles — assert it.

## Testing
Route test for the fifth template's shape; the seeding test unchanged and passing; Vitest for the pre-fill if it has logic.