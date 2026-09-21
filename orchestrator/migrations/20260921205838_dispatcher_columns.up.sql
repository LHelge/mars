-- The columns an unattended launch is decided by, and the record of who
-- launched a session (`ARCHITECTURE.md`, "Task tracker" → "Unattended
-- launches" and "Dispatcher"; `docs/data-model.md`, `agent_profiles`,
-- `projects`, `sessions`; ADR 0042).
--
-- Nothing launches by itself yet: this migration only makes the settings
-- storable and the attribution recordable.

-- Who launched a session. `user` is every launch a person makes, and is the
-- default so that every row written before this migration is one
-- (`docs/data-model.md`, "Enums").
CREATE TYPE session_launch_source AS ENUM ('user', 'dispatcher', 'schedule');

ALTER TABLE sessions
    ADD COLUMN launch_source session_launch_source NOT NULL DEFAULT 'user';

-- Whether the dispatcher may start a session of this profile by itself, and
-- how many live sessions of it an unattended launch may leave behind. Both are
-- refused on a `conversational` profile / below 1 by `ProfileInput`; the
-- `CHECK` is what keeps a value the model never saw out of the column.
ALTER TABLE agent_profiles
    ADD COLUMN auto_launch BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN max_concurrent INTEGER NOT NULL DEFAULT 1 CHECK (max_concurrent >= 1);

-- The project's own cap, NULL meaning "no project cap", and the pause switch
-- that stops every unattended launch of the project while it is set.
ALTER TABLE projects
    ADD COLUMN max_concurrent_sessions INTEGER NULL CHECK (max_concurrent_sessions >= 1),
    ADD COLUMN automation_paused BOOLEAN NOT NULL DEFAULT FALSE;

-- The dispatcher's selection: the `auto_launch` profiles of one project,
-- oldest first, which is the tie-break between two profiles serving the same
-- state (ADR 0042, "Dispatcher"). Partial, because the rows it will never look
-- at are the great majority.
CREATE INDEX agent_profiles_auto_launch_idx
    ON agent_profiles (project_id, created_at)
    WHERE auto_launch;
