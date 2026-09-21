-- One agent credential per scope for the `claude` backend
-- (`docs/data-model.md`, `secrets`; `ARCHITECTURE.md`, "Secrets", Agent
-- credentials; ADR 0036).
--
-- The names are literals because an index predicate cannot be anything else;
-- `AgentBackend::credential_names` is where they really live and a test in
-- `tests/migrations.rs` reads `pg_get_indexdef` to hold the two together.

-- A database written before the rule can hold both credentials at one scope,
-- and the index would then fail with nothing but a constraint name. Say which
-- scope it is and what to do about it first.
DO $$
DECLARE
    offending TEXT;
BEGIN
    SELECT string_agg(format('%s %s', scope, COALESCE(scope_id::text, '(no scope_id)')), ', '
                      ORDER BY scope, scope_id)
      INTO offending
      FROM (SELECT scope, scope_id
              FROM secrets
             WHERE name IN ('ANTHROPIC_API_KEY', 'CLAUDE_CODE_OAUTH_TOKEN')
             GROUP BY scope, scope_id
            HAVING COUNT(*) > 1) AS both_credentials;

    IF offending IS NOT NULL THEN
        RAISE EXCEPTION
            'these scopes hold both agent credentials of the claude backend: %; delete either ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN in each before migrating',
            offending;
    END IF;
END
$$;

CREATE UNIQUE INDEX secrets_claude_credential_idx
    ON secrets (scope, scope_id) NULLS NOT DISTINCT
    WHERE name IN ('ANTHROPIC_API_KEY', 'CLAUDE_CODE_OAUTH_TOKEN');
