---
id: arym6
title: Add "open in session" and "run once" launch controls with profile choice and base-override disclosure to the drawer
status: in_progress
priority: P1
created: "2026-09-16T20:44:10.240271289Z"
updated: "2026-09-20T10:51:51.783400401Z"
tags:
  - frontend
  - tracker
  - sessions
depends_on:
  - k4esd
parent: gn4y2
attempts: 1
---

## Summary
Add the two one-click launch actions to the task drawer: "Open in session" picks a conversational profile (default: the first that serves the task's state) and "Run once" picks an ephemeral profile; both launch `POST /projects/{pid}/sessions` with `task_id`, default the base to the task's current hand-off commit, visibly disclose any base override, and navigate to the new session. Disabled states mirror the API's 409 rules (held, blocked, terminal, project not ready).

## Documents
- `SPEC.md` "Sessions" (`POST /projects/{pid}/sessions` body `{profile_id, base_ref?, title?, message?, task_id?}` → 201 `Session`; 400 unresolvable `base_ref`; 400 ephemeral without task or message; 409 project not ready; 409 task held, blocked or terminal; hand-off defaulting paragraph)
- `SPEC.md` "Frontend", "Task board" ("open in session" picks a conversational profile, default the first that serves the task's state; "run once" with an ephemeral profile) and "Hand-off controls" (default to the hand-off commit and visibly disclose any base override)
- `SPEC.md` "Agent profiles" (`Profile.kind`, `serves_states`), "User-facing features", "Sessions" and "Agent profiles"
- `ARCHITECTURE.md` "Task tracker", "Launching a session for a task" (user launch ignores served states; task must be unheld, unblocked, non-terminal; a task in the human state qualifies; explicit base overrides and confers no approval)

## Acceptance criteria
- [ ] `frontend/src/tasks/LaunchForTask.tsx` renders two buttons in the drawer action bar: `Open in session` and `Run once`. Both are disabled with a `title` reason when `lease_holder_session_id` is set (`Held by a session; release it first`), `blocked` is true (`Blocked by open dependencies or children`), the task's state kind is `terminal` (`Closed tasks cannot be launched`), or the project status is not `ready` (`Project is not ready`).
- [ ] Clicking opens a small form: profile select filtered by `kind === "conversational"` (open) or `kind === "ephemeral"` (run once), loaded with `useQuery(["projects", pid, "profiles"], () => listProfiles(pid))`; default selection is the first profile of that kind whose `serves_states` includes the task's current state name, else the first of that kind; when no profile of the kind exists the form says `No <kind> profile in this project` with a link to the profiles tab.
- [ ] Base section: when `task.handoff` is set, shows `Base: hand-off <short commit> from <source_branch> (<review_status>)` as the default and a disclosure `Override base ref` revealing a text input; when overridden, a visible warning reads `Base overridden: the session will not start from the hand-off commit and this grants no review approval`. Without a hand-off it shows `Base: <default_branch>` with the same override disclosure.
- [ ] Optional `First message` textarea (sent as `message`); `title` is left to the server default (the task's title).
- [ ] Submit calls `createSession(pid, { profile_id, task_id: task.id, base_ref?: override || undefined, message?: text || undefined })`; on 201 it calls `useTaskStore.getState().invalidate()`, invalidates `taskKeys.detail`, and navigates to `/sessions/{id}`. 400 and 409 bodies are shown inline verbatim; a 409 refetches the task (someone claimed it).
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build` pass.

## Implementation notes
- Files: `frontend/src/tasks/LaunchForTask.tsx`, edit `frontend/src/tasks/TaskDetail.tsx`.
- `createSession` and `listProfiles` come from `services/sessions.ts` and `services/profiles.ts` (Frontend project and session views epic); if that epic has not landed, add the two functions with the documented signatures in those files and note it in the commit.
- Project status: read the project through `useQuery(["projects", pid], () => getProject(pid))` (`services/projects.ts`).
- State kind lookup: `useTaskStore` states by name.
- `short commit` = first 10 characters; show the full id in `title`.

## Edge cases
- Ephemeral launch requires `task_id` or `message`; since `task_id` is always sent the message stays optional.
- An override that does not resolve returns 400 from the server; do not validate ref syntax client-side beyond trimming.
- The hand-off may change between opening the form and submitting (new revision published); the server selects the current hand-off atomically, so the UI just refetches after 201/409.
- Human-state tasks are launchable (that is the documented way out of `needs_human`).

## Testing
- Vitest: `defaultProfile(profiles, kind, stateName)` selection helper and `launchDisabledReason(task, stateKind, projectStatus)`.
- Manual: launch from a task with and without a hand-off; confirm `Session.handoff_id` and `base_ref` in the session header; override the base and see the warning.
- `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- Frontend project and session views epic: `services/sessions.ts` `createSession`, `services/profiles.ts` `listProfiles`, `services/projects.ts` `getProject`, the `/sessions/:id` route.
- Session lifecycle epic and Code hand-offs epic: launch-for-task claim and hand-off defaulting as documented.