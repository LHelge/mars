-- Automatic merges, configuration half (`docs/data-model.md`, `task_states`;
-- `SPEC.md`, "Task states"; ADR 0045).
--
-- `auto_merge` marks a queue state whose approved hand-offs the orchestrator
-- merges; `conflict_state_id` is where a task goes when that merge conflicts.
-- The two are set and cleared together, and only a queue state may carry
-- them. The API validates both rules first so a caller reads its message; the
-- `CHECK`s are the backstop. `ON DELETE RESTRICT` keeps a conflict state from
-- vanishing under the state that names it; the repository checks first so the
-- 409 names that state. Existing rows get `auto_merge = false`.
ALTER TABLE task_states
    ADD COLUMN auto_merge BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN conflict_state_id UUID NULL
        CONSTRAINT task_states_conflict_state_id_fkey
        REFERENCES task_states (id) ON DELETE RESTRICT,
    ADD CONSTRAINT task_states_auto_merge_pair_check
        CHECK (auto_merge = (conflict_state_id IS NOT NULL)),
    ADD CONSTRAINT task_states_auto_merge_queue_check
        CHECK (NOT auto_merge OR kind = 'queue');
