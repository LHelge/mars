-- Projects and profiles (`docs/data-model.md`, "Projects and profiles").
--
-- A project is one git repository, mirrored bare under
-- `/data/projects/<id>/repo.git`; the row's `id` is that directory name. The
-- remote credential is deliberately not a column: it is the project-scoped,
-- orchestrator-only secret named `GIT_CREDENTIAL` added by the `secrets`
-- migration.

CREATE TABLE projects (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    remote_url TEXT NOT NULL,
    default_branch TEXT,
    status project_status NOT NULL DEFAULT 'cloning',
    status_message TEXT,
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    last_fetched_at TIMESTAMPTZ,
    -- How many claims a task may go through in one state before a release
    -- sends it to the project's human state instead.
    max_attempts SMALLINT NOT NULL DEFAULT 3 CHECK (max_attempts BETWEEN 1 AND 20),
    -- Counter for `tasks.number`, taken with `UPDATE ... RETURNING` inside the
    -- task insert transaction, which serialises concurrent inserts on the
    -- project row.
    next_task_number INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- The branch may be unknown while discovery from the remote `HEAD` is
    -- pending or has failed, but a ready project always has a resolvable
    -- integration head.
    CHECK (status <> 'ready' OR default_branch IS NOT NULL)
);

-- Per-project configuration of one kind of agent. Every project gets one
-- default conversational profile named `default` on creation.
CREATE TABLE agent_profiles (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    kind profile_kind NOT NULL DEFAULT 'conversational',
    backend agent_backend NOT NULL DEFAULT 'claude',
    model TEXT,
    system_prompt TEXT,
    permission_mode TEXT NOT NULL DEFAULT 'bypass',
    image TEXT NOT NULL,
    runtime TEXT,
    mcp_tools TEXT[] NOT NULL DEFAULT '{}',
    secrets TEXT[] NOT NULL DEFAULT '{}',
    -- No column default by design: the model fills it from `kind` (`true` for
    -- `conversational`, `false` for `ephemeral`) when the caller does not
    -- specify it, so an insert that forgets it fails here.
    partial_messages BOOLEAN NOT NULL,
    idle_timeout_secs INTEGER NOT NULL DEFAULT 1800,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (project_id, name)
);

-- Exactly one default profile per project. Partial rather than a table
-- constraint, so the non-default profiles do not compete for the slot.
CREATE UNIQUE INDEX agent_profiles_one_default_idx ON agent_profiles (project_id) WHERE is_default;

-- Directories under `/data/projects/<project_id>/shared/<name>` that every
-- session container of the project mounts read-write at `container_path`
-- (ADR 0015). The rows are configuration; the directories are created lazily
-- at launch and removed with the row or the project.
CREATE TABLE project_shared_dirs (
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    container_path TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (project_id, name),
    UNIQUE (project_id, container_path)
);
