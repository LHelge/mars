-- Reverses the `.up.sql` exactly: the referencing columns it added to the
-- tables of earlier migrations first, then its own tables in reverse order.
-- The indexes belong to their tables and are dropped with them; only
-- `sessions_task_idx` needs naming, because `sessions` outlives this
-- migration.

DROP INDEX sessions_task_idx;

ALTER TABLE sessions
    DROP COLUMN handoff_id,
    DROP COLUMN task_id;

ALTER TABLE tasks
    DROP COLUMN current_handoff_id;

DROP TABLE task_events;

DROP TABLE task_sessions;

DROP TABLE task_handoffs;

DROP TABLE task_comments;

DROP TABLE task_dependencies;

DROP TABLE tasks;

DROP TABLE profile_states;

DROP TABLE task_states;
