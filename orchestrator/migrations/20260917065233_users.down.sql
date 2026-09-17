-- Drops the tables created by the `.up.sql`, in reverse order; the seeded
-- administrator goes with `users`. The indexes belong to their tables and are
-- dropped with them.

DROP TABLE password_reset_tokens;

DROP TABLE user_invites;

DROP TABLE refresh_tokens;

DROP TABLE users;
