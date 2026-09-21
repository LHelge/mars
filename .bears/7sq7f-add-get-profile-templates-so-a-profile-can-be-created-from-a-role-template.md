---
id: "7sq7f"
title: Add GET /profile-templates so a profile can be created from a role template
status: done
priority: P2
created: "2026-09-20T22:41:04.816655620Z"
updated: "2026-09-21T09:25:18.923803857Z"
tags:
  - orchestrator
  - profiles
  - api
depends_on:
  - pd6zy
parent: pekcb
attempts: 1
---

## Summary
Expose the role templates read-only so the frontend can pre-fill a new profile from one. This is what reaches existing projects (which are not migrated) and restores a role someone deleted. Creating the profile stays the ordinary `POST /projects/{pid}/profiles`; there is no "instantiate template" endpoint.

## Documents
- `SPEC.md` "Agent profiles": new row `GET /profile-templates` and the `ProfileTemplate` shape; cross-reference "Role profile templates"
- The ADR written by the seeding task (one sentence: templates are also offered at profile creation)

## Acceptance criteria
- [ ] `GET /api/profile-templates` (JWT) → `ProfileTemplate[]` in template order: `{ name, kind, backend, serves_states, mcp_tools, system_prompt, is_default }`. `is_default` is informational (which one seeding makes the default); the frontend does not send it on.
- [ ] Served from `profile_templates()`; no database access.
- [ ] 401 unauthenticated; the usual 403 while a password change is required.
- [ ] Documented in `SPEC.md` in the same commit; both clippy invocations and the test suite pass.

## Implementation notes
- Files: `orchestrator/src/routes/profiles.rs` (or a small `routes/profile_templates.rs` if the profiles router is project-nested and a top-level path does not fit it), `routes/mod.rs`, `SPEC.md`.
- Top-level path rather than `/projects/{pid}/…`: the templates do not depend on the project. The *frontend* is what checks that a template's `serves_states` exist in the target project.

## Edge cases
- A project that renamed or removed a state a template serves: `POST …/profiles` answers its existing 400 for an unknown queue state; nothing new on the server.

## Testing
- `tests/profiles.rs` (or a new `tests/profile_templates_api.rs`): happy path returns four in order with non-empty prompts; unauthenticated 401.
