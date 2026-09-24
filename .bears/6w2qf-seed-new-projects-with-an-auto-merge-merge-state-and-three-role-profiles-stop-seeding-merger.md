---
id: "6w2qf"
title: Seed new projects with an auto-merge `merge` state and three role profiles; stop seeding `merger`
status: done
priority: P1
created: "2026-09-22T20:20:39.062025677Z"
updated: "2026-09-24T15:41:05.151521667Z"
tags:
  - orchestrator
  - profiles
  - docs
depends_on:
  - ymav9
parent: tykeu
attempts: 1
---

Implements the seeding half of ADR 0045 (which supersedes ADR 0038 in this one respect). Documents already describe the result: `docs/data-model.md`, `task_states` default set (`merge` with `auto_merge` and conflict state `ready`) and `agent_profiles` (three seeded profiles); `SPEC.md`, "User-facing features" → "Agent profiles" and "Agent profiles" (three seeded); `ARCHITECTURE.md`, "Task tracker" → "State is a queue"; `README.md`, "Operation".

## What to do
- `projects::create::create_project`: the default `merge` state is inserted with `auto_merge = true` and `conflict_state_id` = the `ready` row's id (insert order matters for the FK).
- `projects/profile_templates.rs`: `merger` gets `seeded: false`; it stays in `GET /profile-templates`.
- **This task owns `SPEC.md`, "Role profile templates"**, which `tests/profile_templates.rs` compares with the code, so it was deliberately left unchanged when the other documents were written: change the intro ("seeds three conversational profiles" plus the `merge` state being the orchestrator's, ADR 0045), the `seeded` cell of `merger` to `no`, the sentence about `GET /projects/{pid}/profiles` returning the seeded ones, and the paragraph explaining what is not seeded (now `merger` and `tech-debt-scanner`, for two different reasons).
- The implementer prompt (`templates/implementer.md` and its verbatim copy in `SPEC.md`, "Role profile templates" → `implementer`): after "If the task came back from review, the reviewer's comment says why: address it and hand off a new commit." add a sentence covering a task sent back by a conflicting merge, along the lines of: "If it came back because its merge conflicted, the orchestrator's comment lists the conflicting paths: bring your branch up to date with the default branch, resolve them, and hand off a new commit for review." Keep the deny-list of `tests/common/tracker_products.rs` in mind.
- Fix the tests that assert four seeded profiles (project creation suite, any E2E that counts profiles).

## Done when
`tests/profile_templates.rs` and the project-creation tests pass against the updated section; backend quality chain passes; `frontend` e2e suites that count profiles are updated if any.