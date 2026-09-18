---
id: cpmj2
title: "Add the agent-profiles route module: list, create, get, update and delete under /projects/{pid}/profiles"
status: in_progress
priority: P1
created: "2026-09-16T20:32:51.067210122Z"
updated: "2026-09-18T11:05:32.692869439Z"
tags:
  - orchestrator
  - projects
depends_on:
  - z4g29
  - "5ywhm"
  - cgj5v
parent: pkaee
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create `routes/profiles.rs` exporting `routes() -> Router<AppState>` with the five endpoints of the Agent profiles table. Handlers validate `ProfileInput` through the model, open one transaction per mutation that locks the project row before touching `agent_profiles` and `profile_states`, and map the repository's conflicts to 409. This is the surface that lets a user turn the seeded `default` profile into a planner, reviewer or ephemeral implementer.

## Documents
- `SPEC.md` "Agent profiles (`/api/projects/{pid}/profiles`)": all five rows, `Profile` and `ProfileInput` shapes, `permission_mode` must be `bypass`, `mcp_tools` known names, `serves_states` limited to the project's `queue` states (400 otherwise) defaulting to `["ready"]`, delete 409 if default or has sessions.
- `SPEC.md` "User-facing features" → "Agent profiles" (editable fields; partial-message default; further conversational profiles; ephemeral profiles).
- `docs/data-model.md` `agent_profiles`, `profile_states`, "Tracker mutation transactions" (profile served states under the project lock).
- `CLAUDE.md` "API conventions" (201 for creates, 204 for deletes, plural nouns, nested sub-resources).

## Acceptance criteria
- [ ] `GET /api/projects/{pid}/profiles` → 200 `Profile[]` ordered by `created_at`; the seeded `default` profile appears with `is_default: true` and `serves_states: ["ready"]`; 404 for an unknown project.
- [ ] `POST /api/projects/{pid}/profiles` with `ProfileInput` → 201 `Profile`; 400 for each validation failure (bad `permission_mode`, unknown `mcp_tools` entry, bad secret name, `idle_timeout_secs < 1`, empty name, non-queue or unknown `serves_states` with the message listing the valid names); 409 "profile name already exists"; 404 unknown project.
- [ ] `GET /api/projects/{pid}/profiles/{id}` → 200 or 404 (also 404 when the profile belongs to another project: scope in the `WHERE` clause).
- [ ] `PUT /api/projects/{pid}/profiles/{id}` with `ProfileInput` → 200 `Profile` with full-replacement semantics: omitted optional fields take their documented defaults again (`serves_states` → `["ready"]`, `partial_messages` → by kind, `idle_timeout_secs` → 1800, `mcp_tools`/`secrets` → `[]`, `image` → `SESSION_IMAGE_DEFAULT`), except `is_default`, where omitted means unchanged; `is_default: true` transfers the default from the previous holder in the same transaction; `is_default: false` on the current default → 409 "project must keep a default profile"; 404 unknown; 409 duplicate name.
- [ ] `DELETE /api/projects/{pid}/profiles/{id}` → 204; 409 "the default profile cannot be deleted"; 409 "profile has sessions"; 404 unknown.
- [ ] Every mutation runs `BEGIN` → `ProjectRepository::lock_for_update(pid)` (false → 404) → repository call(s) → `COMMIT`; no `task_events` row is written.
- [ ] 401 without a token and 403 under `must_change_password` for every endpoint.
- [ ] The `Profile` response is exactly `{ id, project_id, name, kind, backend, model, system_prompt, permission_mode, image, runtime, mcp_tools, secrets, serves_states, partial_messages, idle_timeout_secs, is_default, created_at, updated_at }`.

## Implementation notes
- Files: `orchestrator/src/routes/profiles.rs` (new), `orchestrator/src/routes/mod.rs` (mount under `/api/projects/{pid}/profiles`).
- `ProfileInput` is the request body for both `POST` and `PUT`; resolve it with `ProfileInput::resolve(config)` before opening the transaction so pure validation failures never take the project lock; `serves_states` resolution happens inside the transaction through `AgentProfileRepository::set_served_states`.
- For `POST`: `insert` then `set_served_states`; for `PUT`: `update` then `set_served_states` (always replace the link rows, even when unchanged, to keep the handler simple).
- The `mcp_tools` list is stored as given (deduplicated); the MCP epic decides exposure.

## Edge cases
- `PUT` on the default profile that omits `is_default` keeps it default; a client that sends the full `Profile` object back (including `is_default: true`) is also fine.
- Renaming a profile to its own current name is a no-op, not a conflict.
- Changing `kind` from `conversational` to `ephemeral` without `partial_messages` flips the stored flag to `false` (documented default by kind); with an explicit value the value wins.
- `serves_states` naming a `terminal` or `human` state → 400 with the message listing only `queue` names.
- A profile from project A addressed under project B's path → 404, never 403.

## Testing
- Integration tests in `orchestrator/tests/profiles.rs` via `TestApp` (project created through `POST /projects`; it need not be `ready`):
  - list shows the seeded default; create a planner `{name: "planner", serves_states: ["backlog"], system_prompt: "..."}` → 201 and `partial_messages: true`; create an ephemeral implementer without `partial_messages` → `false`; with `partial_messages: true` → `true`.
  - 400 cases: `permission_mode: "plan"`, `mcp_tools: ["push", "delete_repo"]`, `secrets: ["bad-name"]`, `idle_timeout_secs: 0`, `serves_states: ["needs_human"]` (assert the message contains `backlog, ready, review, merge`), `serves_states: ["nope"]`.
  - 409 duplicate name on create and on update; get/update/delete unknown → 404; cross-project id → 404.
  - update with `is_default: true` on the planner → the default moved (list shows exactly one `is_default`); update the old default with `is_default: false` → 409.
  - delete the default → 409; delete a profile after inserting a `sessions` row for it → 409; delete the planner → 204 and its `profile_states` rows are gone.
  - `PUT` omitting `serves_states` resets to `["ready"]`.
  - 401 for each endpoint without a token.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- `SPEC.md` "Agent profiles": add one sentence stating that `PUT` is a full replacement in which omitted fields take their defaults, except `is_default`, which is unchanged when omitted and cannot be cleared on the current default (409). Same commit.

## Assumes from other epics
- "Authentication, users, invites and email": the current-user extractor and gate.
- "Task tracker": state editing; served states are re-validated by that epic when a state is deleted (`profile_states` cascades on `task_states` deletion, so no action here).