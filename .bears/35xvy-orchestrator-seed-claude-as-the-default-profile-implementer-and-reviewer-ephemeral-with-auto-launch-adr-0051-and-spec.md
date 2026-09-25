---
id: "35xvy"
title: "Orchestrator: seed `claude` as the default profile, implementer and reviewer ephemeral with auto_launch; ADR 0051 and SPEC"
status: open
priority: P2
created: "2026-09-25T09:17:20.534068Z"
updated: "2026-09-25T09:17:20.534068Z"
tags:
  - orchestrator
  - docs
  - profiles
parent: xz6yq
---

Implements the epic's orchestrator half and every document change (`SPEC.md`, "Role profile templates", "Agent profiles", "User-facing features"; ADR 0038, 0042, 0045). Read the epic `xz6yq` first.

## Code (`orchestrator/src/projects/profile_templates.rs`, `templates/`)
- `ProfileTemplate` gains `auto_launch: bool`, and `to_new_profile` sets it. `GET /profile-templates` serves it (route DTO; `SPEC.md` `ProfileTemplate` shape). It is sent on like any other template field, so creating one of these profiles from the template without an unattended credential gets the ordinary 400 of the create endpoint, the same way the scheduled template does.
- New template **`claude`**: `conversational`, `serves_states: []`, `mcp_tools: []`, `is_default: true`, `seeded: true`, `auto_launch: false`, prompt in `templates/claude.md`. It comes **first** in `profile_templates()`, so it is the first seeded row and the first `GET /projects/{pid}/profiles` returns. Keep the prompt short: the session runs inside Mars in a container of its own with a clone of the project repository; the task tools of `SPEC.md`, "MCP tool contracts" are available (name them: `ready`, `claim`, `get_task`, `update`, `release`, `comment`, `needs_human`, `create_task`); if launched for a task it already holds it and reads it with `get_task` first; otherwise it does what the user asks and uses the task tools when asked about the board, or when work it discovers belongs in a task of its own. No container or install paragraph.
- `implementer` and `reviewer`: `kind: Ephemeral`, `auto_launch: true`, `implementer` no longer the default. Add one sentence to both prompts for a run that has no conversation: nobody reads a question left in the reply, so a decision or credential it needs goes through `needs_human`, and the run ends when it stops. Check the rest of each prompt still reads right for a one-shot run.
- Unit tests in the module: the seeded list is `["claude", "planner", "implementer", "reviewer"]`; the default is `claude`; `auto_launch` only on ephemeral templates; `no_seeded_template_carries_a_schedule` stays. The module docs change from "the three conversational roles" and "nothing that would start spending money by itself".
- Check `NewAgentProfile::validate` accepts an explicitly empty `serves_states` (the default is `["ready"]` only when the field is omitted over the API).

## Tests
Fix every orchestrator test that assumed the old seeded set: the default profile being `implementer` and conversational, three seeded profiles, and seeded profiles never auto-launching. In particular `tests/profile_templates.rs` (verbatim SPEC comparison, which needs the new prompt reproduced in SPEC), `tests/profile_templates_api.rs`, `tests/projects_create.rs`, `tests/profiles.rs`, and `tests/cron_dispatcher.rs` / `tests/cron_scheduler.rs` / `tests/cron_auto_merge.rs`, where a project with a stored global or project credential now has two seeded `auto_launch` profiles the dispatcher will pick. Where a scenario needs a conversational profile for tasks in `ready`, arrange one explicitly rather than relying on a seeded one. Integration test: a new project's `claude` is the default, serves nothing and is conversational; `implementer` and `reviewer` are ephemeral with `auto_launch`; and the dispatcher launches the seeded implementer on a `ready` task once a project-scope agent credential is stored, but not before (skip `no_credential`).

## Documents (same commit)
- New `docs/decisions/0051-...md` with the decisions and rejected alternatives from the epic, plus an index line in `docs/decisions/README.md`. Update ADR 0038's status line: superseded in one respect by 0051. Check 0042 and 0045 for claims about seeded profiles that no longer hold.
- `SPEC.md`: the "Agent profiles" feature paragraph (four seeded profiles, `claude` the default, implementer and reviewer ephemeral and auto-launched, a new project's `ready` → `done` flow is unattended once a credential is stored); "Automatic dispatch" if it says only a person's decision turns dispatch on; "Agent profiles" (`ProfileTemplate` shape gains `auto_launch`, "A new project already has three of these"); "Role profile templates" (intro, the table gains an `auto_launch` column and a `claude` row, the "every other field" sentence, "returns exactly the three seeded ones", the verbatim `claude` prompt and the changed implementer and reviewer prompts).
- `ARCHITECTURE.md`, `README.md`, `docs/data-model.md`: grep for "three" / "seeded" / "implementer" claims and fix what no longer holds.
- Run `cargo sqlx prepare` only if a query changed.