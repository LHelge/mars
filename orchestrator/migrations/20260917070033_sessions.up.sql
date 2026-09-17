-- Sessions and their event streams (`docs/data-model.md`, "Sessions and
-- events").
--
-- `sessions.task_id` and `sessions.handoff_id` are deliberately absent: they
-- reference tables the `tasks` migration creates, so that migration adds both
-- columns, their foreign keys and `sessions_task_idx` (`docs/data-model.md`,
-- "Migration list for v1", item 5).

-- One agent instance. The row outlives the container: a session may be
-- relaunched into a new container many times.
CREATE TABLE sessions (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    -- Deleting a profile that still has sessions is refused.
    profile_id UUID NOT NULL REFERENCES agent_profiles (id) ON DELETE RESTRICT,
    -- Copied from the profile at launch. Ephemeral sessions are never parked,
    -- resumed or retried.
    kind profile_kind NOT NULL,
    -- The launching user; determines the `user` secret scope.
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    title TEXT,
    state session_state NOT NULL DEFAULT 'creating',
    base_ref TEXT NOT NULL,
    -- Always `session/<id>`; stored so it is queryable.
    branch TEXT NOT NULL,
    container_id TEXT,
    -- The CLI's own session id from its init event. Needed for resume.
    cli_session_id TEXT,
    -- SHA-256 of a fresh random MCP bearer token per process launch. The raw
    -- token is written to `mcp.json` and never recovered from this hash
    -- (ADR 0029).
    mcp_token_hash TEXT NOT NULL UNIQUE,
    -- Cache of the highest committed `events.seq`; the truth is
    -- `MAX(events.seq)`.
    last_seq BIGINT NOT NULL DEFAULT 0,
    -- Advanced on every event; the idle reaper reads it.
    last_activity_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    cost_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
    input_tokens BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    -- Reason for `failed`.
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    parked_at TIMESTAMPTZ,
    ended_at TIMESTAMPTZ
);

CREATE INDEX sessions_project_created_idx ON sessions (project_id, created_at DESC);

CREATE INDEX sessions_state_idx ON sessions (state);

CREATE INDEX sessions_container_id_idx ON sessions (container_id);

-- Append-only; the UI's source of truth for a session. `seq` is monotonic per
-- session and derived from this table at insert time under a session row lock
-- (ADR 0021), never from an in-memory counter. `kind` is TEXT on purpose: the
-- set of kinds is owned by `src/events/`, not by the schema.
CREATE TABLE events (
    session_id UUID NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    seq BIGINT NOT NULL,
    -- Time the orchestrator observed the event.
    ts TIMESTAMPTZ NOT NULL,
    kind TEXT NOT NULL,
    payload JSONB NOT NULL,
    PRIMARY KEY (session_id, seq)
);

-- No further index: every read is by `session_id` and a `seq` range, which the
-- primary key already serves.
