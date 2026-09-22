-- The columns a scheduled agent is launched from (`ARCHITECTURE.md`, "Task
-- tracker" → "Scheduled agents"; `docs/data-model.md`, `agent_profiles`;
-- ADR 0043).
--
-- Nothing fires yet: this migration only makes a schedule storable and its
-- last firing recordable.

-- The expression, the prompt that run is given, and the guard that keeps one
-- tick from firing twice. The expression is a 5-field UTC cron and the prompt
-- is the `message` of the launch, so one is meaningless without the other:
-- `ProfileInput` refuses either alone and the `CHECK` is what keeps a
-- half-configured schedule out of the column. `last_scheduled_at` is written
-- by the scheduler in the transaction that decides a tick fires and is
-- read-only over REST; clearing the schedule clears it.
ALTER TABLE agent_profiles
    ADD COLUMN schedule_cron TEXT NULL,
    ADD COLUMN schedule_prompt TEXT NULL,
    ADD COLUMN last_scheduled_at TIMESTAMPTZ NULL,
    ADD CONSTRAINT agent_profiles_schedule_pair_check
        CHECK ((schedule_cron IS NULL) = (schedule_prompt IS NULL));

-- The scheduler's scan: the profiles that have a schedule at all, which are a
-- small minority of the rows, with the project the capacity rules are asked
-- about beside them.
CREATE INDEX agent_profiles_schedule_idx
    ON agent_profiles (project_id)
    WHERE schedule_cron IS NOT NULL;
