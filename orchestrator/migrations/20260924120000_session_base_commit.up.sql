-- The commit a session's work clone was created at (`docs/data-model.md`,
-- `sessions.base_commit`; `ARCHITECTURE.md`, "Git model", Ref ownership;
-- ADR 0050).
--
-- `base_ref` records the name the launch was given, and a branch may move
-- after the launch; the end-of-session fetch-back needs the commit the
-- session actually started from to tell whether it made any commits of its
-- own. Written by the launch when it creates the work clone. Rows launched
-- before this migration keep NULL, and the rule that reads it then does
-- nothing for them.
ALTER TABLE sessions
    ADD COLUMN base_commit TEXT NULL;
