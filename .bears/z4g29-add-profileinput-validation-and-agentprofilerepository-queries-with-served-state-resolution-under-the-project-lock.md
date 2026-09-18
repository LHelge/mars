---
id: z4g29
title: Add ProfileInput validation and AgentProfileRepository queries with served-state resolution under the project lock
status: done
priority: P1
created: "2026-09-16T20:29:07.295569536Z"
updated: "2026-09-18T09:56:18.014288036Z"
tags:
  - orchestrator
  - projects
  - tracker
depends_on:
  - z4u4e
parent: pkaee
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Deliver the agent-profile domain layer: the `ProfileInput` type with every documented validation rule (`permission_mode` only `bypass`, known `mcp_tools`, secret-name pattern, `partial_messages` default by kind), and `AgentProfileRepository` with list/get/insert/update/delete plus the `profile_states` resolution that maps `serves_states` names to the project's `queue` states inside a project-locked transaction. The default-profile invariant (exactly one per project) is enforced here so both the routes and the project-creation transaction reuse it.

## Documents
- `SPEC.md` "Agent profiles (`/api/projects/{pid}/profiles`)": `Profile` and `ProfileInput` shapes and the three validation rules; "User-facing features" → "Agent profiles" (partial-message default on for conversational, off for ephemeral); "MCP tool contracts" (the tool names: `ready`, `claim`, `get_task`, `update`, `release`, `comment`, `needs_human`, `create_task`, `list_session_branches`, `merge`, `rebase`, `push`).
- `docs/data-model.md` `agent_profiles` (columns, defaults, `UNIQUE (project_id, name)`, `agent_profiles_one_default_idx`, `ON DELETE RESTRICT` from sessions), `profile_states` (same project, only `queue` states, PK), `task_states`, `profile_kind`, `agent_backend` enums, "Tracker mutation transactions" (profile served states use the project lock), `secrets.name` pattern.
- `ARCHITECTURE.md` "Task tracker" → "One mutation at a time per project" (profile served states listed), "MCP design" → "Tool exposure".
- `README.md` "Configuration" `SESSION_IMAGE_DEFAULT`.

## Acceptance criteria
- [ ] `models/agent_profile.rs` defines `AgentProfile` (all columns plus `serves_states: Vec<String>`), `ProfileKind` (`conversational`, `ephemeral`), `AgentBackend` (`claude`), `ProfileInput` and `ProfileError`.
- [ ] `ProfileInput` fields and defaults: `name` (required, trimmed, 1–64 chars), `kind` (default `conversational`), `backend` (default `claude`), `model: Option<String>` (non-empty when present, ≤ 100 chars), `system_prompt: Option<String>` (≤ 64 KiB), `permission_mode` (default `bypass`; any other value → 400 "permission_mode must be \"bypass\""), `image: Option<String>` (default `Config.session_image_default`; non-empty, ≤ 255 chars), `runtime: Option<String>` (non-empty when present), `mcp_tools: Vec<String>` (default empty; each must be in `KNOWN_MCP_TOOLS`, duplicates removed, 400 "unknown MCP tool \"x\"" otherwise), `secrets: Vec<String>` (default empty; each `^[A-Z][A-Z0-9_]{0,127}$`, duplicates removed), `serves_states: Option<Vec<String>>` (default `["ready"]`), `partial_messages: Option<bool>` (default `true` for `conversational`, `false` for `ephemeral`), `idle_timeout_secs: Option<i32>` (default 1800, must be ≥ 1), `is_default: Option<bool>` (default `false` on create; `None` on update means unchanged).
- [ ] `pub const KNOWN_MCP_TOOLS: &[&str]` lives in `models/agent_profile.rs` with the twelve names above; the MCP epic must reuse it (a doc comment says so).
- [ ] `ProfileInput::resolve(self, config: &Config) -> Result<ResolvedProfile, ProfileError>` applies defaults and validation and yields a fully populated struct the repository inserts without further defaulting.
- [ ] `AgentProfileRepository<'a>` provides: `list(project_id)`, `get(project_id, id)`, `insert(tx, project_id, &ResolvedProfile) -> AgentProfile`, `update(tx, project_id, id, &ResolvedProfile, is_default: Option<bool>) -> Option<AgentProfile>`, `delete(tx, project_id, id) -> Result<bool>` (returns `Error::Conflict("the default profile cannot be deleted")` when `is_default`, `Error::Conflict("profile has sessions")` when any `sessions.profile_id` references it, checked explicitly before the `DELETE`), `session_count(tx, id)`, and `set_served_states(tx, project_id, profile_id, &[String])`.
- [ ] `set_served_states` runs after `ProjectRepository::lock_for_update` in the caller's transaction: it selects `id, name, kind FROM task_states WHERE project_id=$1 AND name = ANY($2)`; if any requested name is missing or has `kind <> 'queue'` it returns `Error::BadRequest` with the message `serves_states: "<name>" is not a queue state of this project; queue states are: <comma-separated names in position order>`; otherwise it deletes the profile's `profile_states` rows and inserts the resolved ids.
- [ ] `list`/`get` return `serves_states` as state names ordered by `task_states.position` (`array_remove(array_agg(ts.name ORDER BY ts.position), NULL)` via `LEFT JOIN profile_states ps ... LEFT JOIN task_states ts ...`).
- [ ] Default-profile invariant: `insert`/`update` with `is_default = true` first runs `UPDATE agent_profiles SET is_default = FALSE, updated_at = NOW() WHERE project_id=$1 AND is_default AND id <> $2` in the same transaction so the partial unique index never fires; `update` with `Some(false)` on the current default returns `Error::Conflict("project must keep a default profile")`.
- [ ] A unique violation on `(project_id, name)` maps to `Error::Conflict("profile name already exists")`.
- [ ] `.sqlx/` refreshed and committed.

## Implementation notes
- Files: `orchestrator/src/models/agent_profile.rs`, `orchestrator/src/repositories/agent_profiles.rs`, `orchestrator/src/prelude/error.rs` (`#[from] ProfileError`).
- `ProfileKind` and `AgentBackend` derive `sqlx::Type` with `type_name = "profile_kind"` / `"agent_backend"`, `rename_all = "lowercase"`, and `serde` lower-case.
- `mcp_tools` and `secrets` are `TEXT[]`; bind as `&[String]`.
- Locking order for any write: the caller opens the transaction and calls `ProjectRepository::lock_for_update(tx, project_id)` before `insert`/`update`/`delete`/`set_served_states` (data-model "Tracker mutation transactions" lists profile served states). No `task_events` row is written: `states_changed` is reserved for `task_states` changes.
- `session_count` uses `SELECT COUNT(*) FROM sessions WHERE profile_id = $1`.
- Sessions copy `kind` at launch, so changing a profile's `kind` or `image` is allowed even when it has sessions.

## Edge cases
- `serves_states: []` is valid: the profile serves nothing and `ready` returns an empty list.
- Default `["ready"]` when the project no longer has a `queue` state named `ready` (renamed or deleted) → the same 400 as an explicit bad name; the message lists the valid names.
- `is_default: true` on a profile that is already the default is a no-op.
- The seeded `default` profile's name is not reserved; renaming it is allowed.
- `kind = ephemeral` with `partial_messages` omitted stores `false`; explicit `true` is stored as given.
- Deleting a profile with `sessions` must return 409, never surface the FK violation as 500.

## Testing
- Unit tests for every validation rule and default (both `partial_messages` defaults, `KNOWN_MCP_TOOLS` membership, secret-name pattern, `permission_mode`, `idle_timeout_secs` ≥ 1, duplicate removal).
- Integration tests through `TestApp` at repository level: insert with default served states returns `["ready"]`; `set_served_states` with `["backlog","review"]` works and with `["needs_human"]` or `["nope"]` returns the documented 400 message; transferring `is_default` leaves exactly one default; clearing it on the default returns 409; delete of a profile with a session row returns 409; name uniqueness 409.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- none: implements the documented contract as written. (`name`/`image`/`model` length caps and the `image` default from `SESSION_IMAGE_DEFAULT` are implementation choices within the contract; if the implementer prefers to make them explicit, add one sentence to `SPEC.md` "Agent profiles" in the same commit.)

## Assumes from other epics
- "Database schema, models, repositories and test harness": `agent_profiles`, `profile_states`, `task_states` migrations, `TaskStateRepository` basic CRUD, `Config.session_image_default`, `TestApp`.
- "Task tracker": the `queue`/`human`/`terminal` semantics and state editing; this task only reads `task_states`.