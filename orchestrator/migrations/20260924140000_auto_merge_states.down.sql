ALTER TABLE task_states
    DROP CONSTRAINT task_states_auto_merge_queue_check,
    DROP CONSTRAINT task_states_auto_merge_pair_check,
    DROP COLUMN conflict_state_id,
    DROP COLUMN auto_merge;
