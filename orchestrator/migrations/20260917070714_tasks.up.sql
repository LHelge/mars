-- The shared task tracker (`docs/data-model.md`, "Tasks").
--
-- Agents reach these tables through MCP and users through the REST API; both
-- write the same rows. A task's state is the queue it waits in, the set of
-- states is defined per project, and the lease says which session is working
-- on it right now (ADR 0016).
--
-- The file ends with the ALTERs the schema cannot express earlier: `tasks`
-- references `sessions` and `sessions` references `tasks`, so one direction of
-- that cycle has to be added after both tables exist. `tasks.current_handoff_id`
-- is in the same position, since `task_handoffs` references `tasks`
-- (`docs/data-model.md`, "Migration list for v1", item 5).

-- The states a project's tasks can be in, in board order. Every project is
-- created with the documented default set; users add, rename, reorder and
-- remove states from the project page.
CREATE TABLE task_states (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    -- 1-32 chars matching `[a-z0-9][a-z0-9_-]*`. The API and agents use the
    -- name; the id is internal.
    name TEXT NOT NULL,
    -- `queue`: agents claim from it. `human`: agents never claim from it;
    -- escalations land here. `terminal`: closes the task and satisfies
    -- dependencies. Immutable after creation.
    kind task_state_kind NOT NULL,
    -- Board column order, ascending.
    position INTEGER NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (project_id, name)
);

-- At most one human state per project. Partial rather than a table
-- constraint, so the queue and terminal states do not compete for the slot.
CREATE UNIQUE INDEX task_states_one_human_idx ON task_states (project_id) WHERE kind = 'human';

-- Which states an agent profile serves: the whole role mechanism. The MCP
-- `ready` tool lists, and `claim` claims, only tasks in the calling profile's
-- served states. Both rows belonging to the same project, and only `queue`
-- states being served, are repository checks: a cross-table check needs a
-- trigger.
CREATE TABLE profile_states (
    profile_id UUID NOT NULL REFERENCES agent_profiles (id) ON DELETE CASCADE,
    state_id UUID NOT NULL REFERENCES task_states (id) ON DELETE CASCADE,
    PRIMARY KEY (profile_id, state_id)
);

CREATE TABLE tasks (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    -- Per-project human-readable number, taken from
    -- `projects.next_task_number` inside the insert transaction.
    number INTEGER NOT NULL,
    -- 1-200 chars.
    title TEXT NOT NULL,
    -- Markdown.
    description TEXT NOT NULL DEFAULT '',
    -- The queue the task is in. RESTRICT is what refuses to delete a state
    -- that still holds tasks.
    state_id UUID NOT NULL REFERENCES task_states (id) ON DELETE RESTRICT,
    -- `0` critical, `1` high, `2` medium, `3` low.
    priority SMALLINT NOT NULL DEFAULT 2,
    -- True while any `blocks` dependency or any child is in a non-terminal
    -- state. Stored, not derived, so the claimable query stays a plain indexed
    -- read and becoming unblocked can emit an event.
    blocked BOOLEAN NOT NULL DEFAULT FALSE,
    -- Free-form tags, each 1-32 chars matching the state-name pattern.
    labels TEXT[] NOT NULL DEFAULT '{}',
    -- The epic this task belongs to. One level deep; the repository enforces
    -- that under the project lock.
    parent_id UUID REFERENCES tasks (id) ON DELETE SET NULL,
    -- The person who should look at it, mostly for the human state.
    assignee_user_id UUID REFERENCES users (id) ON DELETE SET NULL,
    -- Session currently working on the task. NULL means nobody.
    lease_holder_session_id UUID REFERENCES sessions (id) ON DELETE SET NULL,
    -- When the current holder claimed it.
    lease_since TIMESTAMPTZ,
    -- Claims since the task last changed state.
    attempts SMALLINT NOT NULL DEFAULT 0,
    -- Why the task was last escalated; set by the `needs_human` tool and by
    -- the reaper.
    needs_human_reason TEXT,
    created_by_user_id UUID REFERENCES users (id) ON DELETE SET NULL,
    created_by_session_id UUID REFERENCES sessions (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Set on entering a terminal state, cleared on leaving one.
    closed_at TIMESTAMPTZ,
    UNIQUE (project_id, number),
    -- A lease is either held with a time or not held at all; a half-set lease
    -- would make the reaper's age calculation meaningless.
    CHECK ((lease_holder_session_id IS NULL) = (lease_since IS NULL)),
    CHECK (priority BETWEEN 0 AND 3),
    CHECK (parent_id <> id)
);

-- The board: every column of a project in display order.
CREATE INDEX tasks_project_state_idx ON tasks (project_id, state_id, priority, number);

-- The `ready` tool: the claimable subset only, so the index stays small even
-- when most of a project's tasks are blocked or already held.
CREATE INDEX tasks_claimable_idx ON tasks (project_id, state_id, priority, number)
    WHERE NOT blocked AND lease_holder_session_id IS NULL;

-- The stuck-task reaper, which walks held tasks by their holder.
CREATE INDEX tasks_lease_holder_idx ON tasks (lease_holder_session_id)
    WHERE lease_holder_session_id IS NOT NULL;

CREATE INDEX tasks_parent_idx ON tasks (parent_id);

-- Edges between tasks. Both tasks belonging to the same project, and the
-- absence of cycles among `blocks` edges, are repository checks made under the
-- project row lock in the same transaction as the insert.
CREATE TABLE task_dependencies (
    task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    depends_on_task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    -- `blocks`: `task_id` is not claimable until `depends_on_task_id` is
    -- terminal. `discovered_from`: provenance only. `related`: informational.
    kind task_dependency_kind NOT NULL DEFAULT 'blocks',
    -- `kind` is part of the key on purpose: removing a blocker must not erase
    -- the provenance edge for the same pair.
    PRIMARY KEY (task_id, depends_on_task_id, kind),
    CHECK (task_id <> depends_on_task_id)
);

-- "Who is waiting on me", read whenever a task becomes terminal.
CREATE INDEX task_dependencies_depends_on_idx ON task_dependencies (depends_on_task_id);

-- The agent-to-agent (and human-to-agent) communication channel. That exactly
-- one author column is set when `system` is false, and none when it is true,
-- is checked at insert time in the repository: either author may later become
-- NULL through `ON DELETE SET NULL`.
CREATE TABLE task_comments (
    id UUID PRIMARY KEY,
    task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    author_user_id UUID REFERENCES users (id) ON DELETE SET NULL,
    author_session_id UUID REFERENCES sessions (id) ON DELETE SET NULL,
    -- True for comments the orchestrator writes (reaper releases,
    -- escalations). Both author columns are NULL.
    system BOOLEAN NOT NULL DEFAULT FALSE,
    body TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX task_comments_task_idx ON task_comments (task_id, created_at);

-- An immutable record of code passed between workers, including the review
-- status of that exact commit (ADR 0018). Rows never change except for
-- foreign keys becoming NULL on deletion.
CREATE TABLE task_handoffs (
    -- The commit is retained at `refs/handoffs/<id>` in the project's bare
    -- repository.
    id UUID PRIMARY KEY,
    -- The project is derived from the task.
    task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    -- Original producing session; required when first publishing, copied when
    -- forwarding.
    source_session_id UUID REFERENCES sessions (id) ON DELETE SET NULL,
    -- Snapshot of the original `session/<id>` branch name, retained after the
    -- session is deleted.
    source_branch TEXT NOT NULL,
    -- Full git object id, validated as a commit in this project repository.
    commit TEXT NOT NULL,
    -- Required at creation and belongs to this task (repository check); the
    -- comment describes work, checks or review findings.
    comment_id UUID REFERENCES task_comments (id) ON DELETE SET NULL,
    -- Applies only to `commit`: a new revision starts unreviewed, and
    -- forwarding either carries a decision forward or records a new one.
    review_status TEXT NOT NULL DEFAULT 'unreviewed'
        CHECK (review_status IN ('unreviewed', 'approved', 'changes_requested')),
    reviewed_by_user_id UUID REFERENCES users (id) ON DELETE SET NULL,
    reviewed_by_session_id UUID REFERENCES sessions (id) ON DELETE SET NULL,
    -- Carried forward with an existing decision.
    reviewed_at TIMESTAMPTZ,
    created_by_user_id UUID REFERENCES users (id) ON DELETE SET NULL,
    created_by_session_id UUID REFERENCES sessions (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX task_handoffs_task_idx ON task_handoffs (task_id, created_at);

-- Which sessions worked on which tasks. Upserted on an actual change:
-- `first_touched_at` is preserved on conflict and `last_touched_at` advances
-- only when something really changed (ADR 0030).
CREATE TABLE task_sessions (
    task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    session_id UUID NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    first_touched_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_touched_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (task_id, session_id)
);

CREATE INDEX task_sessions_session_idx ON task_sessions (session_id);

-- The project-scoped `TaskEvent` stream, delivered over SSE. Append-only, with
-- `seq` allocated as `MAX(seq) + 1` for this project while holding the project
-- row lock (ADR 0021), in the same transaction as the change it describes and
-- its `pg_notify` (ADR 0028). `kind` is TEXT on purpose: the set of kinds is
-- owned by `src/events/`, not by the schema.
CREATE TABLE task_events (
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    -- Monotonic per project; used as the SSE `id`.
    seq BIGINT NOT NULL,
    ts TIMESTAMPTZ NOT NULL,
    -- Deliberately no foreign key: the original task UUID is retained after
    -- the task is deleted, and history is never rewritten (ADR 0022). NULL
    -- only for project-wide events such as `states_changed`.
    task_id UUID,
    kind TEXT NOT NULL,
    payload JSONB NOT NULL,
    PRIMARY KEY (project_id, seq)
);

-- The columns that close the cycles, added now that both ends exist.

-- Current immutable code hand-off; must belong to this task (repository
-- check). The current record is selected through this column, never by
-- timestamp.
ALTER TABLE tasks
    ADD COLUMN current_handoff_id UUID REFERENCES task_handoffs (id) ON DELETE SET NULL;

-- `task_id`: the task the session was launched for, if any; the launch claims
-- it in the same transaction. `handoff_id`: the hand-off used to choose the
-- initial checkout, whose commit is stored in `base_ref`
-- (`docs/data-model.md`, "Sessions and events").
ALTER TABLE sessions
    ADD COLUMN task_id UUID REFERENCES tasks (id) ON DELETE SET NULL,
    ADD COLUMN handoff_id UUID REFERENCES task_handoffs (id) ON DELETE SET NULL;

CREATE INDEX sessions_task_idx ON sessions (task_id) WHERE task_id IS NOT NULL;
