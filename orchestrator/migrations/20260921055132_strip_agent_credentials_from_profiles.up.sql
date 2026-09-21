-- Remove every agent credential name from `agent_profiles.secrets`
-- (`docs/data-model.md`, `agent_profiles`; ADR 0036).
--
-- The launcher resolves the backend's credential for every session of that
-- backend without the profile declaring it, so a listed credential is
-- redundant and misleading. `ProfileInput` now refuses one on the way in;
-- this is the rows written before that rule existed.
--
-- The names are literal here, as in the credential index migration: a
-- migration is a statement about the database at a point in time and cannot
-- read a constant that a later release may change.
--
-- `array_remove` keeps the order of what is left and the other entries
-- untouched, and the `WHERE` keeps the write to the rows that carry one.
UPDATE agent_profiles
SET secrets = array_remove(
        array_remove(secrets, 'ANTHROPIC_API_KEY'),
        'CLAUDE_CODE_OAUTH_TOKEN'
    ),
    updated_at = NOW()
WHERE secrets && ARRAY['ANTHROPIC_API_KEY', 'CLAUDE_CODE_OAUTH_TOKEN'];
