-- Users and authentication (`docs/data-model.md`, "Users and authentication").
--
-- There is no self-registration: this migration seeds the one administrator
-- and every other user is created by accepting an invite (ADR 0013).

CREATE TABLE users (
    id UUID PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    auth_version BIGINT NOT NULL DEFAULT 0 CHECK (auth_version >= 0),
    must_change_password BOOLEAN NOT NULL DEFAULT FALSE,
    admin BOOLEAN NOT NULL DEFAULT FALSE,
    notify_email BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- The bootstrap administrator, with the fixed default password documented in
-- `README.md`, "Start" (ADR 0024): an Argon2id PHC string of `changeme`,
-- generated once with the default parameters and a random salt. The operator
-- is forced to replace it at first login by `must_change_password`. Not a real
-- credential (`CLAUDE.md`, rule 3).
INSERT INTO users (id, username, email, password_hash, must_change_password, admin)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'admin',
    'admin@localhost',
    '$argon2id$v=19$m=19456,t=2,p=1$LdkhVNG/wlqQnG2ibpyNiA$8Bl44TOasplKBxanrfHFXTjyUXCZ4EAe30zLkRQmscE',
    TRUE,
    TRUE
);

CREATE TABLE refresh_tokens (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX refresh_tokens_user_id_idx ON refresh_tokens (user_id);

CREATE INDEX refresh_tokens_expires_at_idx ON refresh_tokens (expires_at);

CREATE TABLE user_invites (
    id UUID PRIMARY KEY,
    email TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    admin BOOLEAN NOT NULL DEFAULT FALSE,
    invited_by UUID REFERENCES users (id) ON DELETE SET NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    accepted_at TIMESTAMPTZ,
    accepted_user_id UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- At most one open invite per email address. A partial index rather than a
-- table constraint, so an accepted invite no longer occupies the address.
CREATE UNIQUE INDEX user_invites_open_email_idx ON user_invites (email) WHERE accepted_at IS NULL;

CREATE INDEX user_invites_expires_at_idx ON user_invites (expires_at);

CREATE TABLE password_reset_tokens (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX password_reset_tokens_user_id_idx ON password_reset_tokens (user_id);

CREATE INDEX password_reset_tokens_expires_at_idx ON password_reset_tokens (expires_at);
