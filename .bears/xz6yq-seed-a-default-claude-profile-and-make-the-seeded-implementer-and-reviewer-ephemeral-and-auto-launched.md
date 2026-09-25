---
id: xz6yq
title: Seed a default `claude` profile, and make the seeded implementer and reviewer ephemeral and auto-launched
type: epic
status: open
priority: P2
created: "2026-09-25T09:17:00.794553Z"
updated: "2026-09-25T09:17:00.794553Z"
tags:
  - orchestrator
  - frontend
  - profiles
  - automation
---

## Scope
Designed with the user on 2026-09-25. They wanted to launch a plain Claude Code session from the project page's *Launch a session* form without picking a role. Instead of a nullable `sessions.profile_id` ("no profile"), the seeded profiles change:

- A new seeded template **`claude`**: `conversational`, serves **no** queue state, no git tools, a short system prompt saying the session runs inside Mars and the task tools are available. It becomes the project's **default** profile, so the launch form preselects it.
- **`implementer`** and **`reviewer`** become `ephemeral` with `auto_launch: true`. With the seeded auto-merge `merge` state, a task in `ready` then travels to `done` without anyone launching anything.
- `planner` stays conversational over `backlog`. `merger` and `tech-debt-scanner` stay offered and unseeded.
- `auto_launch` is seeded on unconditionally, whether or not a `global`- or `project`-scope agent credential exists. The dispatcher already rechecks at launch (`has_unattended_credential`, skip reason `no_credential`), so the profiles start working once one is stored. Saving an edit to either profile without such a credential gets the existing 400 at the auto-launch toggle, and that is accepted.
- Existing projects are **not** migrated: templates are copied, not referenced (ADR 0038). An existing project gets `claude` by creating a profile from the template.

## Decisions (settled with the user, 2026-09-25)
- Seeded default profile instead of a "no profile" launch. Rejected: a nullable `sessions.profile_id` with built-in defaults in code, which touches the launcher, recovery and MCP auth, and exists only to model the absence of a profile.
- Auto-launch seeded on and skipped without a credential. Rejected: seeding it only when a global credential exists at creation (a credential added later would not switch it on), and relaxing the save-time credential rule for every profile (ADR 0036, 0042).
- `claude` serves no queue. Rejected: `ready` (it would see the implementer's queue) and every queue state.

This reverses ADR 0038's "nothing seeded spends money by itself" in one respect, so a new ADR (0051) records it and marks 0038 as superseded in that respect.

## Acceptance criteria
- [ ] Every child task is done and the full quality chains pass.
- [ ] ADR 0051, `SPEC.md` ("User-facing features" → Agent profiles and Automatic dispatch; "Agent profiles"; "Role profile templates") and the in-app help describe what shipped.