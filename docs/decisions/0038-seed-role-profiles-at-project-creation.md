# 0038. Seed the four role profiles at project creation, copied not referenced

Status: accepted. Superseded in one respect by ADR 0045: `merger` is no longer seeded, because an orchestrator job merges approved hand-offs; the other three roles and the copy-not-reference rule stand. Superseded in one respect by ADR 0051: a default `claude` profile is seeded first, and `implementer` and `reviewer` are seeded ephemeral with `auto_launch`, so a seeded profile may spend money without a person launching it.

## Context

A role in Mars is a profile's served states plus its system prompt; nothing about `planner`, `implementer`, `reviewer` or `merger` is code (`ARCHITECTURE.md`, "Task tracker"). Until now creation seeded one profile named `default`, serving `ready`, with no prompt at all. The board arrived with four queue columns and one agent that only knew how to pick work out of one of them, so every new project began with the same unwritten homework: four prompts that have to agree with the tool contracts, the hand-off rules and the approval model before anything flows.

Two alternatives were considered.

A **template picker** in the profile editor — a list of role prompts a user can apply to a profile they create — keeps the seeding as it is and still leaves a new project starting with a blank agent and an empty board. The work is not choosing a prompt; it is knowing that four profiles over four queues is how the product is meant to be used. A picker teaches that only to someone who already went looking.

**Resolving the prompts live from the binary** — a profile with no `system_prompt` falling back to the template for its name — would keep every project's agents current with the shipped text. It also means a release silently changes how an existing project's agents behave, in the middle of work those agents are doing, and that an edit has nowhere to live: changing one word turns the profile into an ordinary one and severs it from later improvements anyway.

## Decision

`create_project` seeds four conversational profiles in the same transaction as the task states: `planner` over `backlog`, `implementer` over `ready` and the project's default, `reviewer` over `review` with `list_session_branches`, and `merger` over `merge` with `list_session_branches` and `merge`. The prompt texts live as files under `orchestrator/src/projects/templates/`, are embedded with `include_str!`, and are **copied** into each project's rows. Nothing reads them again.

`push` goes to nobody (ADR 0007) and `rebase` to nobody: an approval is bound to the commit that was reviewed (ADR 0018), so the merger answers a conflict by sending the task back to `ready` rather than by rewriting it, and the implementer rebases inside its own clone, whose `origin` is the project repository.

## Consequences

A new project is usable as a four-role board immediately, and the prompts are reviewable prose in a diff, reproduced verbatim in `SPEC.md`, "Role profile templates" with a test comparing the two so they cannot drift.

The templates are also offered when a profile is created, read-only over `GET /profile-templates` (`SPEC.md`, "Agent profiles"): the rejected picker was rejected as a *replacement* for seeding, and beside it is what reaches a project created before this decision and what brings a deleted role back.

The seeded profiles are ordinary rows: editable, renameable, deletable except for the default, and untouched by upgrades. Improving a template therefore reaches new projects only; existing ones keep what they were given, which is the point.

The prompts name the seeded state names. A project that renames or deletes `ready`, `review`, `merge`, `backlog` or `done` keeps the link (`profile_states` is by id) but leaves the prompt naming a state that no longer exists, and the text then needs a manual edit. That is accepted: the alternative is a prompt that rewrites itself, which is a template resolved live by another name.
