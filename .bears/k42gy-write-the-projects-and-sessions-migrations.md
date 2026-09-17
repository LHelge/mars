---
id: k42gy
title: Write the projects and sessions migrations
status: done
priority: P0
created: "2026-09-16T20:28:46.663165904Z"
updated: "2026-09-17T07:05:35.810442962Z"
tags:
  - orchestrator
  - core
  - projects
  - sessions
depends_on:
  - suzac
parent: p5tsd
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create migrations 3 and 4: `projects` (`projects`, `agent_profiles`, `project_shared_dirs`) and `sessions` (`sessions`, `events`). The `sessions.task_id` and `sessions.handoff_id` columns are deliberately *not* part of this migration; the `tasks` migration adds them.

## Documents
- `docs/data-model.md` "Projects and profiles" (`projects`, `project_shared_dirs`, `agent_profiles`), "Sessions and events" (`sessions`, `events`), "Migration list for v1" items 3 and 4.

## Acceptance criteria
- [ ] `sqlx migrate add -r projects` creates `projects` (`id UUID PK`, `name TEXT NOT NULL UNIQUE`, `remote_url TEXT NOT NULL`, `default_branch TEXT NULL`, `status project_status NOT NULL DEFAULT 'cloning'`, `status_message TEXT NULL`, `created_by UUID NULL REFERENCES users(id) ON DELETE SET NULL`, `last_fetched_at TIMESTAMPTZ NULL`, `max_attempts SMALLINT NOT NULL DEFAULT 3 CHECK (max_attempts BETWEEN 1 AND 20)`, `next_task_number INTEGER NOT NULL DEFAULT 1`, `created_at`, `updated_at`) with `CHECK (status <> 'ready' OR default_branch IS NOT NULL)`.
- [ ] `agent_profiles` (`id UUID PK`, `project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE`, `name TEXT NOT NULL`, `kind profile_kind NOT NULL DEFAULT 'conversational'`, `backend agent_backend NOT NULL DEFAULT 'claude'`, `model TEXT NULL`, `system_prompt TEXT NULL`, `permission_mode TEXT NOT NULL DEFAULT 'bypass'`, `image TEXT NOT NULL`, `runtime TEXT NULL`, `mcp_tools TEXT[] NOT NULL DEFAULT '{}'`, `secrets TEXT[] NOT NULL DEFAULT '{}'`, `partial_messages BOOLEAN NOT NULL` (no default), `idle_timeout_secs INTEGER NOT NULL DEFAULT 1800`, `is_default BOOLEAN NOT NULL DEFAULT FALSE`, `created_at`, `updated_at`) with `UNIQUE (project_id, name)` and `CREATE UNIQUE INDEX agent_profiles_one_default_idx ON agent_profiles (project_id) WHERE is_default`.
- [ ] `project_shared_dirs` (`project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE`, `name TEXT NOT NULL`, `container_path TEXT NOT NULL`, `created_at`) with `PRIMARY KEY (project_id, name)` and `UNIQUE (project_id, container_path)`.
- [ ] `sqlx migrate add -r sessions` creates `sessions` with every column from the document except `task_id` and `handoff_id`: `id UUID PK`, `project_id ... ON DELETE CASCADE`, `profile_id UUID NOT NULL REFERENCES agent_profiles(id) ON DELETE RESTRICT`, `kind profile_kind NOT NULL`, `created_by UUID NULL ... ON DELETE SET NULL`, `title TEXT NULL`, `state session_state NOT NULL DEFAULT 'creating'`, `base_ref TEXT NOT NULL`, `branch TEXT NOT NULL`, `container_id TEXT NULL`, `cli_session_id TEXT NULL`, `mcp_token_hash TEXT NOT NULL UNIQUE`, `last_seq BIGINT NOT NULL DEFAULT 0`, `last_activity_at TIMESTAMPTZ NOT NULL DEFAULT NOW()`, `cost_usd DOUBLE PRECISION NOT NULL DEFAULT 0`, `input_tokens BIGINT NOT NULL DEFAULT 0`, `output_tokens BIGINT NOT NULL DEFAULT 0`, `error TEXT NULL`, `created_at`, `parked_at NULL`, `ended_at NULL`.
- [ ] Session indexes `sessions_project_created_idx (project_id, created_at DESC)`, `sessions_state_idx (state)`, `sessions_container_id_idx (container_id)`. (`sessions_task_idx` is created by the `tasks` migration together with the column.)
- [ ] `events` (`session_id UUID NOT NULL REFERENCES sessions(id) ON DELETE CASCADE`, `seq BIGINT NOT NULL`, `ts TIMESTAMPTZ NOT NULL`, `kind TEXT NOT NULL`, `payload JSONB NOT NULL`) with `PRIMARY KEY (session_id, seq)` and no other index.
- [ ] Down migrations drop `events`, `sessions` and `project_shared_dirs`, `agent_profiles`, `projects` in reverse creation order; the round-trip test passes.

## Implementation notes
- Files: `orchestrator/migrations/<ts>_projects.{up,down}.sql`, `<ts>_sessions.{up,down}.sql`, created after the `users` migration so the version order matches the document.
- The partial unique index for the default profile uses `WHERE is_default` (boolean column, no comparison needed).
- `events.kind` is `TEXT` on purpose (the set of kinds is owned by `src/events/`); do not create an enum.
- Add a schema assertion to `tests/migrations.rs` (or a `tests/schema.rs`) that queries `information_schema.columns` / `pg_indexes` for a handful of load-bearing details: the `CHECK` on `projects.default_branch`, `sessions.profile_id` delete rule `RESTRICT`, `agent_profiles_one_default_idx` is partial and unique, `events` has exactly one index (the primary key).

## Edge cases
- `partial_messages` has no default by design; an insert without it must fail at the database, which the profile model prevents by filling it from `kind`.
- `mcp_token_hash` is UNIQUE from the start; tests inserting sessions must generate distinct hashes.

## Testing
- `tests/migrations.rs` round-trip; schema assertions listed above.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` must pass.

## Documentation
- None: implements the documented schema as written.

## Assumes from other epics
- none beyond the scaffolding assumptions of the harness task.