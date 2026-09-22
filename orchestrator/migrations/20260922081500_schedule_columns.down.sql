-- Reverse `schedule_columns`, in the opposite order: the index, then the
-- constraint and the three columns it is over.

DROP INDEX agent_profiles_schedule_idx;

ALTER TABLE agent_profiles
    DROP CONSTRAINT agent_profiles_schedule_pair_check,
    DROP COLUMN last_scheduled_at,
    DROP COLUMN schedule_prompt,
    DROP COLUMN schedule_cron;
