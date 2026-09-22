---
id: wpju2
title: "Tighten types at the edges: noUncheckedIndexedAccess, unions for wire strings, no casts from DOM values, encoded URL building in services"
status: done
priority: P3
created: "2026-09-21T10:56:30.956675752Z"
updated: "2026-09-22T00:13:31.144223389Z"
tags:
  - frontend
  - technical-review
  - typescript
  - refactor
parent: "579dz"
attempts: 1
---

Problem: type discipline is good overall (no `any`, no non-null `!`, ApiError narrowed by instanceof at all 44 sites, types/ matches SPEC.md). What remains:
- A real bug from a wide type: tasks/taskStateRules.ts:37-43 (countTasksByState) and :67 use a plain object as Record<string, number>. `constructor` is a valid state name under [a-z0-9][a-z0-9_-]*; `counts["constructor"] ?? 0` yields Object, the count becomes a string, `inState > 0` compares NaN, so the delete button is enabled for a non-empty state and the count cell shows garbage (the server still answers 409). Use Map or Object.create(null).
- tsconfig: enable noUncheckedIndexedAccess (record[key] becomes T | undefined). In services/utils it flags only one-token fixes (apiClient.ts:55, utils/returnTo.ts:39, github.ts:39, taskRef.ts:31, sharedDir.ts:124/:138/:203); its value is the Record<string, Message> lookups in session/, where the code already hand-writes `=== undefined` checks against types that claim the value cannot be undefined (SessionHeader.tsx:227-229). exactOptionalPropertyTypes: the code already follows the discipline; expect friction from React props for little gain — decide and note it.
- Casts from DOM strings into unions, safe only because options are generated from the same constants: `Number(x) as TaskPriority` (TaskEditForm.tsx:194/:202, CreateTaskForm.tsx:173/:181), `as TaskStateKind` (TaskStatesEditor.tsx:282/:291), `as TaskDependencyKind` (DependencyEditor.tsx:136), `as ProfileKind` (ProfileEditorPage.tsx). A `parseX(s): X | undefined` over the const array removes all of them.
- Wide strings where a union exists: types/profiles.ts backend and permission_mode, profileForm.ts constants typed string and `mcp_tools: string[]` although PROFILE_GATED_TOOLS is a const tuple (a typo in a tool name type-checks); types/git.ts DiffFile.status is string with Record<string, string> lookup tables in ChangesFileList.tsx:13-29 whose R and C entries are dead under --no-renames.
- Shapes that should be discriminated: `{ kind: "git"; op; detail: GitDetail }` does not tie op to the detail shape, so SessionHeader.tsx:231 casts `message.detail as { work_tree?: string }` (toolInput.ts already has record()/inputString() guards to use meanwhile); JsonTree Branch takes `value: unknown` and casts (JsonTree.tsx:81-83) although Node has already narrowed it; request types looser than their own doc comments — types/tasks.ts source_session_id optional but "Required over REST" and the frontend is only ever the REST client, UpdateTaskInput.handoff "requires a different target state" untied to `state`.
- services build query strings five ways (sessions.ts:20-26 and :76-86, tasks.ts:30-33 and a hard-coded ?state_kind=human at :41, git.ts:30-36, secrets.ts:27-31 and :65) and encode path ids inconsistently: users, secrets and taskStates encodeURIComponent, sessions, projects, tasks, profiles and git do not — safe only because pages UUID-gate route params, an invariant that lives far from the service. listSessions(params: { state }) and listProjectSessions(pid, state) differ in shape for the same filter.

Acceptance: the `constructor` state name counts correctly (unit test). noUncheckedIndexedAccess is on and the build is clean without `!`. The casts above are replaced by parse helpers; the wide strings become unions derived from their const arrays; git events narrow detail by op; request types match their contracts. apiGet and friends take an optional `query` that serialises defined values through URLSearchParams, a `seg()` helper encodes path parts, and every service uses them. types/ keeps mirroring SPEC.md field for field.

References: files above; frontend/tsconfig.app.json; frontend/eslint.config.js. Contract: CLAUDE.md, "Frontend conventions" (types mirror SPEC.md exactly; all API calls through services/).