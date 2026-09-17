-- Envelope-encrypted secrets and the audit of their use
-- (`docs/data-model.md`, "Secrets"; the design is in `ARCHITECTURE.md`,
-- "Secrets").
--
-- The orchestrator never stores a secret value in the clear: a random per-row
-- data key encrypts the value, and the master key from the keyring wraps that
-- data key. Only the wrapped form and its nonces are in the database, so a
-- dump without the keyring yields nothing. `key_version` records which master
-- key did the wrapping, which is what rotation sweeps walk.

CREATE TABLE secrets (
    id UUID PRIMARY KEY,
    -- Resolution order at launch is `global`, then `project`, then the
    -- launching user; the last one found wins.
    scope secret_scope NOT NULL,
    -- `users.id` for `user`, `projects.id` for `project`, NULL for `global`.
    -- Deliberately no foreign key: the scope decides which table the id points
    -- at. The repository validates existence and a reaper deletes orphans.
    scope_id UUID,
    -- Environment-variable style, `^[A-Z][A-Z0-9_]{0,127}$`, validated in the
    -- model.
    name TEXT NOT NULL,
    -- AES-256-GCM ciphertext of the value, including the 16-byte tag. The
    -- additional authenticated data is `<scope>:<scope_id or empty>:<name>`,
    -- so a ciphertext copied to another row fails to decrypt.
    ciphertext BYTEA NOT NULL,
    -- 12-byte nonce for `ciphertext`.
    nonce BYTEA NOT NULL,
    -- The per-row data key, AES-256-GCM wrapped by the master key.
    data_key_wrapped BYTEA NOT NULL,
    -- 12-byte nonce for the wrap.
    data_key_nonce BYTEA NOT NULL,
    -- Which master key wrapped `data_key_wrapped`. Rotation re-wraps the data
    -- key and updates this; the ciphertext is untouched.
    key_version INTEGER NOT NULL,
    -- Never injected into containers; used by the orchestrator itself, for
    -- example by the git credential provider.
    orchestrator_only BOOLEAN NOT NULL DEFAULT FALSE,
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- NULLS NOT DISTINCT so that two global secrets of the same name collide:
    -- with the default nulls-distinct behaviour the NULL `scope_id` would let
    -- duplicates in. Left unnamed on purpose, so Postgres assigns
    -- `secrets_scope_scope_id_name_key` and the repository can match that name
    -- to report a conflict.
    UNIQUE NULLS NOT DISTINCT (scope, scope_id, name),
    -- A global secret has no target and a scoped one always has one; either
    -- half alone would make resolution ambiguous.
    CHECK ((scope = 'global') = (scope_id IS NULL))
);

-- Rotation sweeps select the rows still wrapped by an older master key.
CREATE INDEX secrets_key_version_idx ON secrets (key_version);

-- Which secret was resolved into which session, and why. Written per injected
-- secret on every launch, including relaunches of parked sessions, and by the
-- git credential provider.
CREATE TABLE secret_uses (
    id BIGINT PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
    secret_id UUID NOT NULL REFERENCES secrets (id) ON DELETE CASCADE,
    -- Set by launches and by the MCP git tools; NULL for the REST git path and
    -- for the mirror-fetch job, which sets neither.
    session_id UUID REFERENCES sessions (id) ON DELETE CASCADE,
    -- Set when a user's request used the secret.
    user_id UUID REFERENCES users (id) ON DELETE SET NULL,
    -- `launch` or `git`. Validated by the model rather than by a CHECK: the
    -- document lists no constraint for it.
    purpose TEXT NOT NULL,
    at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- "When was this secret last used", newest first.
CREATE INDEX secret_uses_secret_idx ON secret_uses (secret_id, at DESC);

-- "Which secrets did this session get", skipping the rows with no session at
-- all.
CREATE INDEX secret_uses_session_idx ON secret_uses (session_id) WHERE session_id IS NOT NULL;
