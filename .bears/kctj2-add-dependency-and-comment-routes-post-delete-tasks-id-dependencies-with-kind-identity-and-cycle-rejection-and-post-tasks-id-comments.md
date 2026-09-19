---
id: kctj2
title: "Add dependency and comment routes: POST/DELETE /tasks/{id}/dependencies with kind identity and cycle rejection, and POST /tasks/{id}/comments"
status: in_progress
priority: P1
created: "2026-09-16T20:44:08.521746833Z"
updated: "2026-09-19T11:22:42.583959233Z"
tags:
  - orchestrator
  - tracker
depends_on:
  - h8qkt
  - "6jycg"
parent: "5h3y4"
attempts: 1
---

## Summary
Expose dependency edges and comments over REST and as domain functions the MCP `update`, `comment` and `create_task` tools reuse. Adding a dependency is identified by `(task, depends_on, kind)`, checks cycles for `blocks` only, recomputes the dependant's `blocked` flag and emits `dependency_added` plus any flip; removing one kind leaves the others intact and emits `dependency_removed`. A comment emits `commented` with the comment payload and links the authoring session to the task.

## Documents
- `SPEC.md` "Tasks": `POST /projects/{pid}/tasks/{id}/dependencies {depends_on: id, kind?: "blocks" | "discovered_from" | "related"}` → 200 `Task` (`kind` defaults to `blocks`; 409 on cycle); `DELETE /projects/{pid}/tasks/{id}/dependencies/{dep}?kind=blocks` → 200 `Task` (removes only that kind; accepts the three kinds); `POST /projects/{pid}/tasks/{id}/comments {body}` → `Comment`; `{id}` and `{dep}` accept UUID or number; only `blocks` participates in cycle checks and affects `blocked`; the same pair may carry `blocks` and `discovered_from`.
- `SPEC.md` "TaskEvent" (`dependency_added`, `dependency_removed`, `blocked`, `unblocked`, `commented` with `comment`), "MCP tool contracts" → `comment` ("Any session in the project may comment on any task"), `update` (`add_depends_on` creates `blocks`, cycle → `invalid_argument`; `remove_depends_on` removes only `blocks`).
- `docs/data-model.md` `task_dependencies` (PK with kind, same-project repository check, cycle CTE under the lock), `task_comments` (author rule; `system` comments have no author), `task_sessions` (upsert on comment).
- `ARCHITECTURE.md` "Task tracker" → "Blocked is stored" (kinds coexist and are removed independently).
- ADRs 0021, 0023, 0030.

## Acceptance criteria
- [ ] `tracker::dependencies::add_dependency(m, task: &Task, depends_on: &Task, kind) -> Result<TaskDto>`: same-project check (400 `dependency must reference tasks of the same project`), self-edge → 400, `kind == Blocks` → `check_no_cycle` (409 `dependency would create a cycle`), `insert_dependency` (PK violation → 409 `dependency already exists`), emit `dependency_added` with the dependant task after the insert, then `recompute_blocked(&[task.id])` for `blocks` only; `touch_actor(task.id)`.
- [ ] `tracker::dependencies::remove_dependency(m, task, depends_on, kind) -> Result<TaskDto>`: `delete_dependency` returning `false` → 404 `dependency not found`; emit `dependency_removed`; `recompute_blocked(&[task.id])` for `blocks` only; `touch_actor`.
- [ ] `tracker::comments::add_comment(m, task: &Task, author: CommentAuthor::User(Uuid) | CommentAuthor::Session(Uuid) | CommentAuthor::System, body) -> Result<CommentDto>`: builds `NewTaskComment` (model validation: non-empty body → 400 `comment body must not be empty`; author rule), inserts, emits `commented` with `{actor, task, comment}` (the task payload is the unchanged task), `touch_actor` for session authors. System comments carry actor `system`.
- [ ] Routes in `orchestrator/src/routes/tasks.rs`: `POST .../dependencies` → 200 `Task` (body `{depends_on: String (UUID or number), kind?: TaskDependencyKind}`; unknown `kind` string → 400 `invalid dependency kind`; unknown `depends_on` → 404); `DELETE .../dependencies/{dep}?kind=` → 200 `Task` (`kind` query required; missing or unknown → 400); `POST .../comments {body}` → 201 `Comment` (the general create rule; `SPEC.md` shows no explicit status).
- [ ] `cargo sqlx prepare` run if queries were added.

## Implementation notes
- Files: `orchestrator/src/tracker/dependencies.rs`, `orchestrator/src/tracker/comments.rs`, `orchestrator/src/routes/tasks.rs` (extend).
- Resolve `{id}`, `{dep}` and `depends_on` with `TaskRef` inside the mutation (numbers are looked up under the lock so a concurrent delete cannot swap identities).
- The returned `Task` DTO is loaded after the flag recompute so `blocked` and `depends_on` are current.
- `TaskDependencyKind` deserialises from `blocks|discovered_from|related` via the model's serde derive; the query parameter uses the same enum with `serde_urlencoded`.

## Edge cases
- Adding `related` between tasks that already have `blocks` succeeds; removing `blocks` afterwards leaves `related`; the `blocks` removal unblocks if no other open prerequisite remains.
- Adding `blocks` on a terminal prerequisite: edge inserted, no flip, `dependency_added` only.
- Removing a `blocks` edge whose prerequisite is terminal: no flip.
- Cross-project `depends_on` given by number resolves within `pid` only; a number that exists in another project is 404 here.
- Comment on a task in another project's path → 404 (scope in the `WHERE`).
- A comment by a session that is not the holder is allowed (comments are open to any session in the project); the lease is irrelevant.

## Testing
- Integration tests in `orchestrator/tests/task_dependencies_api.rs` and `task_comments_api.rs` through `TestApp`: add `blocks` on an open task → 200 with `blocked: true`, `depends_on` containing `{task_id, kind: "blocks"}`, the prerequisite's `blocks` list containing the dependant, events `dependency_added` then `blocked`; add `discovered_from` on the same pair → 200, no flip event; A→B→C then C→A `blocks` → 409, `related` C→A → 200; duplicate → 409; self → 400; other project → 400 (by UUID) or 404 (by number); remove `blocks` with `?kind=blocks` → `dependency_removed` then `unblocked`, `discovered_from` still listed; remove with no `kind` → 400; remove a non-existent kind → 404; two concurrent reciprocal `blocks` requests (spawned together) → exactly one 200 and one 409; comment → 201 `Comment` with `author_user_id` = caller, `system: false`, one `commented` event whose `comment.body` matches; empty body → 400; unauthenticated → 401.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `SPEC.md` "Tasks": add the explicit `201` to the `POST .../comments` row (the table currently omits the status; the default create rule gives 201). One-line change in the same commit.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `insert_dependency`, `delete_dependency`, `insert_comment`, `NewTaskComment::validate`, `TaskRef`.
- "Authentication, users, invites and email": `CurrentUser`, `TestApp` login helpers.