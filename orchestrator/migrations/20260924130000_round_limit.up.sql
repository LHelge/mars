-- The round limit (`docs/data-model.md`, `projects.max_rounds` and
-- `tasks.rounds`; `ARCHITECTURE.md`, "Task tracker" → "Rounds"; ADR 0046).
--
-- `max_rounds` is how many revision hand-offs a task may go through before a
-- send-back by a session or the system sends it to the project's human state
-- instead. `rounds` counts the revisions published since the task last left
-- the human state: publication increments it, any state change out of the
-- human state resets it to 0. Existing tasks start at 0, which gives every
-- task in flight a full allowance.
ALTER TABLE projects
    ADD COLUMN max_rounds SMALLINT NOT NULL DEFAULT 5 CHECK (max_rounds BETWEEN 1 AND 50);

ALTER TABLE tasks
    ADD COLUMN rounds SMALLINT NOT NULL DEFAULT 0;
