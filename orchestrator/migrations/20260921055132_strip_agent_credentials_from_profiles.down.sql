-- A deliberate no-op.
--
-- The `.up.sql` removed agent credential names from `agent_profiles.secrets`.
-- Those entries carried no information the launcher does not already act on:
-- the credential is resolved for every session of its backend whether or not
-- the profile lists it (ADR 0036), so a restored entry would change nothing
-- and there is nothing to restore. Reverting the schema is what a `.down.sql`
-- owes, and this migration changed no schema.
SELECT 1;
