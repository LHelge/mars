-- Reverse `dispatcher_columns`, in the opposite order: the index, the columns,
-- then the type the `sessions` column was of, which cannot be dropped while a
-- column still uses it.

DROP INDEX agent_profiles_auto_launch_idx;

ALTER TABLE projects
    DROP COLUMN automation_paused,
    DROP COLUMN max_concurrent_sessions;

ALTER TABLE agent_profiles
    DROP COLUMN max_concurrent,
    DROP COLUMN auto_launch;

ALTER TABLE sessions
    DROP COLUMN launch_source;

DROP TYPE session_launch_source;
