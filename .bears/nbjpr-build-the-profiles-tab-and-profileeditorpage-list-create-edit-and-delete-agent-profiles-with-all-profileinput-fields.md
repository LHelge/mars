---
id: nbjpr
title: "Build the profiles tab and ProfileEditorPage: list, create, edit and delete agent profiles with all ProfileInput fields"
status: open
priority: P1
created: "2026-09-16T20:44:51.816509270Z"
updated: "2026-09-16T20:44:51.816509270Z"
tags:
  - frontend
  - projects
  - agent
depends_on:
  - dxunp
parent: cgdc2
---

## Summary
Fill the `profiles` tab of `ProjectPage` with the profile list and the `ProfileEditorPage` form that creates and edits a profile: name, kind, model, system prompt, image, runtime, MCP tool allow-list, declared secrets, served task states, partial-message flag (defaulted per kind) and idle timeout. Deletion surfaces the API's refusals for the default profile and profiles with sessions.

## Documents
- `SPEC.md` "User-facing features", Agent profiles paragraph (default conversational profile with the built-in Claude image serving `ready`; editable fields; served states plus system prompt define the role; ephemeral profiles run one prompt).
- `SPEC.md` "Agent profiles" table: `GET/POST /projects/{pid}/profiles`, `GET/PUT/DELETE /projects/{pid}/profiles/{id}` (DELETE 204; 409 if default or has sessions); `Profile` and `ProfileInput` shapes; `permission_mode` must be `bypass`; `mcp_tools` entries must be known tool names; `serves_states` entries must be names of the project's `queue` states (400 otherwise) and default to `["ready"]`.
- `SPEC.md` "MCP tool contracts": task tools are always allowed; the profile-gated tools are `list_session_branches`, `merge`, `rebase`, `push`.
- `SPEC.md` "Task states": `GET /projects/{pid}/task-states` → `TaskState[]` with `kind`.
- `SPEC.md` "Secrets": `GET /secrets?scope=&scope_id=` → `SecretMeta[]` (names only; orchestrator-only secrets are never injected).
- `docs/data-model.md` `agent_profiles` (column notes: `model` null means CLI default; `runtime` null means engine default; `partial_messages` has no column default, the model sets true for conversational and false for ephemeral; `idle_timeout_secs` default 1800; `is_default` exactly one per project; `secrets` are names, orchestrator-only ones never injected even if listed).
- `SPEC.md` "Frontend" pages list (`ProfileEditorPage`).

## Acceptance criteria
- [ ] `frontend/src/pages/project/ProfilesTab.tsx` lists `listProfiles(pid)` (key `["projects", pid, "profiles"]`): name (with `default` badge when `is_default`), kind, model or `CLI default`, image (monospace), `serves_states` chips, `idle_timeout_secs`, and actions `Edit`, `Delete`.
- [ ] `New profile` opens `frontend/src/pages/ProfileEditorPage.tsx` in create mode; `Edit` opens it in edit mode. The editor is mounted inside the tab (no new route) and is driven by `?tab=profiles&profile=new|<id>` so it is linkable.
- [ ] Fields: `name` (required, text); `kind` (`conversational` | `ephemeral`; switching kind before the user has touched `partial_messages` resets that checkbox to the kind default); `backend` fixed `claude` (read-only); `model` (optional text, placeholder `CLI default`); `system_prompt` (monospace textarea); `permission_mode` fixed `bypass` (read-only, sent as `"bypass"`); `image` (text, required; prefilled from the project's default profile image in create mode); `runtime` (optional, placeholder `engine default`, hint `runsc`, `kata`); `mcp_tools` checkboxes for `list_session_branches`, `merge`, `rebase`, `push` with the note that task tools are always available; `secrets` multi-select of names from `GET /secrets` at global, project (`scope_id=pid`) and the caller's user scope, deduplicated, with orchestrator-only ones shown disabled and annotated `never injected`, plus a free-text add for names not yet created; `serves_states` checkboxes over the project's `queue` states from `listTaskStates` (default `ready` checked in create mode); `partial_messages` checkbox; `idle_timeout_secs` number (min 60, default 1800).
- [ ] Submit sends the full `ProfileInput` (PUT is full replacement); `partial_messages` is always sent explicitly from the checkbox state; success invalidates the profile list and returns to the list; 400 (unknown tool, non-queue state, bad permission mode) and 409 (duplicate name) messages are shown via `Alert`.
- [ ] `Delete` is disabled with tooltip `default profile` for `is_default`; otherwise confirms and calls `deleteProfile`; a 409 shows `profile has sessions` (server message) inline.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: `frontend/src/pages/project/ProfilesTab.tsx`, `frontend/src/pages/ProfileEditorPage.tsx`, `frontend/src/pages/project/profileForm.ts` (pure helpers: `defaultInputForKind(kind, image)`, `toProfileInput(profile)`, unit-tested), `frontend/src/pages/index.ts`.
- Known MCP tool names are a constant `PROFILE_GATED_TOOLS = ["list_session_branches", "merge", "rebase", "push"] as const` in `frontend/src/types/profile.ts` (added here, exported).
- Secret name sources: `listSecrets({scope: "global"})`, `listSecrets({scope: "project", scope_id: pid})`, `listSecrets({scope: "user"})` from the foundation's `services/secrets.ts`.
- Use `FormField` for every input; `useFormSubmit` for pending/error state; the `/frontend-design` skill for the two-column dense layout (settings left, prompt right).

## Edge cases
- Editing the default profile: `name` is editable (uniqueness enforced server-side); `is_default` is not part of `ProfileInput` and never sent.
- `serves_states` referencing a state that was since deleted or renamed: the API rejects with 400; show the message and let the user fix the checkboxes.
- A project with no `queue` states cannot occur (last queue state cannot be deleted) but render defensively.
- `idle_timeout_secs` must be a positive integer; block submit otherwise.
- Ephemeral profiles with `partial_messages: true` are allowed; just the default differs.

## Testing
- Vitest: `profileForm.test.ts` covering `defaultInputForKind` (conversational → `partial_messages: true`, ephemeral → `false`, `serves_states: ["ready"]`, `idle_timeout_secs: 1800`, `permission_mode: "bypass"`) and `toProfileInput` stripping ids/timestamps/`is_default`.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend foundation epic: `services/secrets.ts#listSecrets({scope, scope_id?})`, `useAuth()` (for the caller's user id if needed), form components.
- Projects epic (backend): profile endpoints with the documented statuses.