---
id: vnwqh
title: "Write the tasks migration: tracker tables and the deferred session and hand-off columns"
status: done
priority: P0
created: "2026-09-16T20:30:09.653306476Z"
updated: "2026-09-17T07:11:31.591492714Z"
tags:
  - orchestrator
  - core
  - tracker
depends_on:
  - k42gy
parent: p5tsd
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create migration 5, `tasks`: `task_states`, `profile_states`, `tasks`, `task_dependencies`, `task_comments`, `task_handoffs`, `task_sessions`, `task_events`, and then the deferred columns `tasks.current_handoff_id`, `sessions.task_id` and `sessions.handoff_id` with their foreign keys and the `sessions_task_idx` index. The down migration drops the referencing columns first, then the tables in reverse order.

## Documents
- `docs/data-model.md` "Tasks" (every subsection), "Sessions and events" (`sessions.task_id`, `sessions.handoff_id`, `sessions_task_idx`), "Migration list for v1" item 5.

## Acceptance criteria
- [ ] `task_states` (`id UUID PK`, `project_id ... ON DELETE CASCADE`, `name TEXT NOT NULL`, `kind task_state_kind NOT NULL`, `position INTEGER NOT NULL`, `created_at`) with `UNIQUE (project_id, name)` and `CREATE UNIQUE INDEX task_states_one_human_idx ON task_states (project_id) WHERE kind = 'human'`.
- [ ] `profile_states` (`profile_id UUID NOT NULL REFERENCES agent_profiles(id) ON DELETE CASCADE`, `state_id UUID NOT NULL REFERENCES task_states(id) ON DELETE CASCADE`, `PRIMARY KEY (profile_id, state_id)`).
- [ ] `tasks` with every column: `id UUID PK`, `project_id ... CASCADE`, `number INTEGER NOT NULL`, `title TEXT NOT NULL`, `description TEXT NOT NULL DEFAULT ''`, `state_id UUID NOT NULL REFERENCES task_states(id) ON DELETE RESTRICT`, `priority SMALLINT NOT NULL DEFAULT 2`, `blocked BOOLEAN NOT NULL DEFAULT FALSE`, `labels TEXT[] NOT NULL DEFAULT '{}'`, `parent_id UUID NULL REFERENCES tasks(id) ON DELETE SET NULL`, `assignee_user_id UUID NULL REFERENCES users(id) ON DELETE SET NULL`, `lease_holder_session_id UUID NULL REFERENCES sessions(id) ON DELETE SET NULL`, `lease_since TIMESTAMPTZ NULL`, `attempts SMALLINT NOT NULL DEFAULT 0`, `needs_human_reason TEXT NULL`, `created_by_user_id`, `created_by_session_id` (both NULL, SET NULL), `created_at`, `updated_at`, `closed_at NULL`; constraints `UNIQUE (project_id, number)`, `CHECK ((lease_holder_session_id IS NULL) = (lease_since IS NULL))`, `CHECK (priority BETWEEN 0 AND 3)`, `CHECK (parent_id <> id)`.
- [ ] Task indexes: `tasks_project_state_idx (project_id, state_id, priority, number)`, `tasks_claimable_idx (project_id, state_id, priority, number) WHERE NOT blocked AND lease_holder_session_id IS NULL`, `tasks_lease_holder_idx (lease_holder_session_id) WHERE lease_holder_session_id IS NOT NULL`, `tasks_parent_idx (parent_id)`.
- [ ] `task_dependencies` (`task_id`, `depends_on_task_id` both `UUID NOT NULL REFERENCES tasks(id) ON DELETE CASCADE`, `kind task_dependency_kind NOT NULL DEFAULT 'blocks'`) with `PRIMARY KEY (task_id, depends_on_task_id, kind)`, `CHECK (task_id <> depends_on_task_id)`, index `task_dependencies_depends_on_idx (depends_on_task_id)`.
- [ ] `task_comments` (`id UUID PK`, `task_id ... CASCADE`, `author_user_id UUID NULL ... SET NULL`, `author_session_id UUID NULL ... SET NULL`, `system BOOLEAN NOT NULL DEFAULT FALSE`, `body TEXT NOT NULL`, `created_at`) with `task_comments_task_idx (task_id, created_at)`.
- [ ] `task_handoffs` (`id UUID PK`, `task_id ... CASCADE`, `source_session_id UUID NULL REFERENCES sessions(id) ON DELETE SET NULL`, `source_branch TEXT NOT NULL`, `commit TEXT NOT NULL`, `comment_id UUID NULL REFERENCES task_comments(id) ON DELETE SET NULL`, `review_status TEXT NOT NULL DEFAULT 'unreviewed' CHECK (review_status IN ('unreviewed','approved','changes_requested'))`, `reviewed_by_user_id`, `reviewed_by_session_id`, `reviewed_at NULL`, `created_by_user_id`, `created_by_session_id` (all nullable, SET NULL), `created_at`) with `task_handoffs_task_idx (task_id, created_at)`.
- [ ] `task_sessions` (`task_id ... CASCADE`, `session_id ... CASCADE`, `first_touched_at`, `last_touched_at` both `NOT NULL DEFAULT NOW()`) with `PRIMARY KEY (task_id, session_id)` and `task_sessions_session_idx (session_id)`.
- [ ] `task_events` (`project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE`, `seq BIGINT NOT NULL`, `ts TIMESTAMPTZ NOT NULL`, `task_id UUID NULL` with **no** foreign key, `kind TEXT NOT NULL`, `payload JSONB NOT NULL`) with `PRIMARY KEY (project_id, seq)`.
- [ ] Then `ALTER TABLE tasks ADD COLUMN current_handoff_id UUID NULL REFERENCES task_handoffs(id) ON DELETE SET NULL`; `ALTER TABLE sessions ADD COLUMN task_id UUID NULL REFERENCES tasks(id) ON DELETE SET NULL, ADD COLUMN handoff_id UUID NULL REFERENCES task_handoffs(id) ON DELETE SET NULL`; `CREATE INDEX sessions_task_idx ON sessions (task_id) WHERE task_id IS NOT NULL`.
- [ ] Down: drop `sessions_task_idx`, drop the three added columns, then drop `task_events`, `task_sessions`, `task_handoffs`, `task_comments`, `task_dependencies`, `tasks`, `profile_states`, `task_states`. Round-trip test passes.

## Implementation notes
- File pair `orchestrator/migrations/<ts>_tasks.{up,down}.sql`, created after `sessions`.
- Keep the column order of the document; the ALTERs go at the end of the up file so the file reads like the document.
- Schema assertions to add to `tests/migrations.rs`: `task_events.task_id` has no FK; `tasks.state_id` delete rule is `RESTRICT`; `tasks_claimable_idx` is partial with the documented predicate; `task_states_one_human_idx` is unique and partial.

## Edge cases
- `sessions` already exists (migration 4); adding `task_id` with a FK to `tasks` requires `tasks` to exist, which is why these columns live here.
- Postgres requires the referenced `task_handoffs(id)` to exist before `tasks.current_handoff_id` can reference it, so the ALTER on `tasks` must follow `CREATE TABLE task_handoffs`.

## Testing
- `tests/migrations.rs` round-trip and the schema assertions above.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented schema as written.

## Assumes from other epics
- none.