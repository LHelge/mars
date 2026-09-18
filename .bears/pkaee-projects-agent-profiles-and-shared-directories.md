---
id: pkaee
title: Projects, agent profiles and shared directories
type: epic
status: done
priority: P1
created: "2026-09-16T20:12:51.364909831Z"
updated: "2026-09-18T12:22:27.905130673Z"
tags:
  - orchestrator
  - projects
depends_on:
  - z4u4e
---

## Scope

The project aggregate and its configuration.

- `POST /api/projects` returning `status: cloning`, the background clone job moving the project to `ready` or `error` (with `status_message`), `retry-clone`, `fetch`, `branches`, `PUT` (name, default branch, `max_attempts`), `DELETE` refused while sessions run and otherwise removing sessions, tasks, secrets, shared dirs, CLI state dir and mirror under the project git lock. `credential` on create stored as the orchestrator-only `GIT_CREDENTIAL` secret; `has_credential` in the DTO.
- Default `task_states` set and the `default` conversational profile serving `ready` created with the project (the tracker semantics of states are in the Task tracker epic; this epic seeds the rows).
- Agent profiles CRUD: `ProfileInput` validation (`permission_mode` only `bypass`, known `mcp_tools`, `serves_states` limited to the project's queue states, `partial_messages` default by kind), one default per project, delete refused for the default or a profile with sessions.
- Shared directories CRUD: name and path validation rules, `clear` and delete refused while a session is `running` or `creating`, directory removal with the row.
- Data-directory layout creation under `DATA_DIR/projects/<id>/` (`repo.git`, `claude/`, `shared/`).

## Documents

`SPEC.md` "Projects", "Shared directories", "Agent profiles"; `ARCHITECTURE.md` "Storage"; `docs/data-model.md` "Projects and profiles", `task_states` default set, `profile_states`; ADR 0015.

## Acceptance criteria

- [ ] Every endpoint in the three tables has happy-path and error-path tests (401, 400 validation, 404, 409 conflicts including running-session refusals).
- [ ] Project creation against a real local bare repository reaches `ready` with discovered `default_branch`; an unreachable remote reaches `error` and `retry-clone` works.
- [ ] Deleting a project removes every on-disk artefact and cascades rows.

## Out of scope

Task state editing semantics and events (Task tracker epic); mirror fetch scheduling (Background jobs epic).