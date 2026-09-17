-- The enum types of the schema (`docs/data-model.md`, "Enums").
--
-- Values are only ever added by a later migration, never removed or renamed
-- (`CLAUDE.md`, "Backend conventions"). Event kinds are deliberately TEXT and
-- are not listed here.

CREATE TYPE project_status AS ENUM ('cloning', 'ready', 'error');

CREATE TYPE profile_kind AS ENUM ('conversational', 'ephemeral');

CREATE TYPE agent_backend AS ENUM ('claude');

CREATE TYPE session_state AS ENUM ('creating', 'running', 'parked', 'done', 'failed');

CREATE TYPE task_state_kind AS ENUM ('queue', 'human', 'terminal');

CREATE TYPE task_dependency_kind AS ENUM ('blocks', 'discovered_from', 'related');

CREATE TYPE secret_scope AS ENUM ('global', 'user', 'project');
