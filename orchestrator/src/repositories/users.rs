//! All SQL against `users`, plus the two locks the authentication epic
//! composes its invariants from.
//!
//! `docs/data-model.md`, "Users and authentication" is the contract. Two
//! paragraphs of it are the reason this file has locking primitives at all:
//!
//! - *Administrator membership.* "User deletion and changes to `users.admin`
//!   preserve at least one administrator. Before authoritative reads or
//!   writes, these operations acquire the same transaction-scoped database
//!   advisory lock." [`UserRepository::lock_admin_membership`] is that lock.
//!   The invariant is a composition — lock, re-count, then mutate — and lives
//!   with the routes; the repository only provides the pieces, so
//!   [`UserRepository::delete`] deliberately does not check anything.
//! - *The user row.* "Login, refresh, reset-link issuance, password changes
//!   and reset-token consumption lock the user row before locking or writing
//!   that user's token rows." [`UserRepository::lock_user`] is that lock, and
//!   it returns the row read *under* it so a caller cannot accidentally act on
//!   a value it read before waiting.
//!
//! One statement here is not about `users` alone. A password change has to
//! update the hash, bump `auth_version`, revoke the user's refresh tokens and
//! spend its outstanding reset tokens *atomically*, so
//! [`UserRepository::apply_password_change`] owns all four statements rather
//! than delegating three of them to
//! [`crate::repositories::RefreshTokenRepository`]. Every other token-table
//! statement lives with its own table.

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::{Email, NewUser, User, UserUpdate};
use crate::prelude::*;
use crate::repositories::unique_violation;

/// The key every administrator-membership lock uses.
///
/// `pg_advisory_xact_lock` namespaces nothing for us: two unrelated locks that
/// pick the same `bigint` block each other, so every advisory key in the crate
/// is a constant defined exactly once, next to the only code that takes it.
/// The value is the ASCII of `MARSUSER`, chosen to be recognisable in
/// `pg_locks` while debugging and to be nowhere near another subsystem's.
pub const ADMIN_MEMBERSHIP_LOCK_KEY: i64 = 0x4D41_5253_5553_4552;

/// All SQL against `users` (`ARCHITECTURE.md`, "Orchestrator internals").
///
/// Reads that need no transaction go straight to the pool; everything that
/// mutates, locks or has to be read under someone else's lock takes the
/// caller's `&mut PgConnection`, so one transaction can hold a whole
/// composition — the administrator lock, a re-count and a delete — together.
pub struct UserRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> UserRepository<'a> {
    /// Borrow `pool` for the lifetime of this repository.
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new user and return the stored row.
    ///
    /// A duplicate username or email is the caller's mistake, not an internal
    /// failure, so the two unique constraints map to [`Error::Conflict`] with
    /// the message `SPEC.md`, "Users" implies for a 409. Every other database
    /// error widens as usual.
    pub async fn insert(&self, tx: &mut PgConnection, user: &NewUser) -> Result<User> {
        let inserted = sqlx::query_as!(
            User,
            r#"
            INSERT INTO users (id, username, email, password_hash, admin, must_change_password)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, username, email, password_hash, auth_version,
                      must_change_password, admin, notify_email, created_at, updated_at
            "#,
            user.id,
            user.username.as_str(),
            user.email.as_str(),
            user.password_hash,
            user.admin,
            user.must_change_password,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(map_duplicate)?;

        // The email is deliberately absent: an address is personal data and a
        // log line is not the place for it (`CLAUDE.md`, "Backend
        // conventions"). The hash is absent at every level (rule 3).
        debug!(user_id = %inserted.id, admin = inserted.admin, "user inserted");

        Ok(inserted)
    }

    /// The user with this id, or `None`.
    pub async fn find(&self, id: Uuid) -> Result<Option<User>> {
        let user = sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, password_hash, auth_version,
                   must_change_password, admin, notify_email, created_at, updated_at
            FROM users
            WHERE id = $1
            "#,
            id,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(user)
    }

    /// The user with this username, or `None`.
    ///
    /// Compared verbatim: usernames are stored with the case they were
    /// registered with, so this is the same comparison `users_username_key`
    /// makes. Login passes what the caller typed.
    pub async fn find_by_username(&self, username: &str) -> Result<Option<User>> {
        let user = sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, password_hash, auth_version,
                   must_change_password, admin, notify_email, created_at, updated_at
            FROM users
            WHERE username = $1
            "#,
            username,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(user)
    }

    /// The user with this email, or `None`.
    ///
    /// Takes a parsed [`crate::models::Email`] rather than a `&str` because
    /// the stored form is the normalised one; looking up an address that has
    /// not been through `Email::parse` would miss rows that differ only in
    /// case or surrounding whitespace.
    pub async fn find_by_email(&self, email: &Email) -> Result<Option<User>> {
        let user = sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, password_hash, auth_version,
                   must_change_password, admin, notify_email, created_at, updated_at
            FROM users
            WHERE email = $1
            "#,
            email.as_str(),
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(user)
    }

    /// The user whose username *or* email is `identifier`, or `None`.
    ///
    /// What a password-reset request has to work with: the form asks for "your
    /// username or email" and the caller cannot know which it was given
    /// (`SPEC.md`, "Authentication"). Both comparisons happen in the one
    /// statement so the choice is not a read-then-read.
    ///
    /// The two comparisons are deliberately different, because the two columns
    /// are stored differently (see [`UserRepository::find_by_username`] and
    /// [`UserRepository::find_by_email`]): the username is matched verbatim,
    /// the email against `identifier` trimmed and lower-cased, which is the
    /// same normalisation [`crate::models::Email::parse`] applies before a row
    /// is stored. Passing something that is not a valid address is fine — it
    /// simply matches no email.
    ///
    /// Both can match at once, when one user's username is another user's
    /// email address. `ORDER BY` settles it in favour of the username owner:
    /// the identifier was typed into a field that offers a username first, and
    /// a rule in the statement is better than whichever row Postgres happened
    /// to return.
    pub async fn find_by_username_or_email(&self, identifier: &str) -> Result<Option<User>> {
        let email = identifier.trim().to_lowercase();

        let user = sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, password_hash, auth_version,
                   must_change_password, admin, notify_email, created_at, updated_at
            FROM users
            WHERE username = $1 OR email = $2
            ORDER BY (username = $1) DESC
            LIMIT 1
            "#,
            identifier,
            email,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(user)
    }

    /// Every user, by username (`GET /users`).
    ///
    /// The order is the one `GET /users` answers in (`SPEC.md`, "Users"), and
    /// the column it sorts on is unique, so the listing is stable without a
    /// tie-breaker and a client never has to sort it again.
    pub async fn list(&self) -> Result<Vec<User>> {
        let users = sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, password_hash, auth_version,
                   must_change_password, admin, notify_email, created_at, updated_at
            FROM users
            ORDER BY username
            "#,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(users)
    }

    /// Apply the set fields of `update` and return the stored row, or `None`
    /// when no user has this id (the route decides whether that is a 404).
    ///
    /// `COALESCE` per column keeps "leave it alone" in the statement rather
    /// than in a query builder, so one prepared statement serves `PUT
    /// /users/{id}` and `PATCH /users/me`. `updated_at` moves on every call.
    ///
    /// Changing `admin` is only half of the administrator-membership
    /// invariant: the caller has to hold [`UserRepository::lock_admin_membership`]
    /// and re-count first (`docs/data-model.md`, "Users and authentication").
    pub async fn update(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        update: &UserUpdate,
    ) -> Result<Option<User>> {
        let updated = sqlx::query_as!(
            User,
            r#"
            UPDATE users
            SET username = COALESCE($2, username),
                admin = COALESCE($3, admin),
                notify_email = COALESCE($4, notify_email),
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, username, email, password_hash, auth_version,
                      must_change_password, admin, notify_email, created_at, updated_at
            "#,
            id,
            update.username.as_ref().map(|name| name.as_str()),
            update.admin,
            update.notify_email,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_duplicate)?;

        debug!(user_id = %id, updated = updated.is_some(), "user updated");

        Ok(updated)
    }

    /// Perform the whole password mutation in the caller's transaction and
    /// return the user as it now stands.
    ///
    /// The one statement sequence `docs/data-model.md`, "Users and
    /// authentication" requires to be atomic: "Password changes and resets
    /// atomically update `password_hash`, clear `must_change_password`,
    /// increment `auth_version`, revoke all of the user's existing refresh
    /// tokens and invalidate outstanding reset tokens." Splitting it across
    /// repositories would let a caller commit three of the four; keeping it
    /// here means a caller cannot. The three flows `SPEC.md`,
    /// "Authentication" lists — self-service change, an administrator changing
    /// someone else's password, and a reset link — differ only in
    /// `replacement`.
    ///
    /// `replacement` is the SHA-256 hex of a fresh refresh token, or `None`.
    /// `Some` is the self-service change, which keeps the acting browser
    /// signed in by inserting its replacement *inside* this transaction, after
    /// the blanket revocation; the token gets the documented 30-day life. An
    /// administrator's change and a reset link pass `None` and log nobody in
    /// (ADR 0025).
    ///
    /// Every timestamp comes from the database's `NOW()`, which in Postgres is
    /// the start of this transaction, so the revocations, the spent reset
    /// tokens and the replacement's expiry are all measured from one clock.
    ///
    /// **Caller's contract.** Take
    /// [`UserRepository::lock_user`] first and revalidate against the row it
    /// returned — a current-password check made before the lock is not
    /// evidence once the lock is granted. Return the new credentials only
    /// after the transaction commits.
    ///
    /// A missing id is [`Error::NotFound`]: `RETURNING` yields no row, and
    /// there is nothing to revoke either. The token statements are
    /// unconditional and match zero rows for a user who has none.
    pub async fn apply_password_change(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        password_hash: &str,
        replacement: Option<&str>,
    ) -> Result<User> {
        let user = sqlx::query_as!(
            User,
            r#"
            UPDATE users
            SET password_hash = $2,
                auth_version = auth_version + 1,
                must_change_password = FALSE,
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, username, email, password_hash, auth_version,
                      must_change_password, admin, notify_email, created_at, updated_at
            "#,
            id,
            password_hash,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;

        let revoked = sqlx::query!(
            "UPDATE refresh_tokens SET revoked_at = NOW() WHERE user_id = $1 AND revoked_at IS NULL",
            id,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();

        let spent = sqlx::query!(
            "UPDATE password_reset_tokens SET used_at = NOW() WHERE user_id = $1 AND used_at IS NULL",
            id,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();

        // After the revocation, never before it: a replacement inserted first
        // would revoke itself.
        if let Some(token_hash) = replacement {
            sqlx::query!(
                r#"
                INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at)
                VALUES ($1, $2, $3, NOW() + INTERVAL '30 days')
                "#,
                Uuid::new_v4(),
                id,
                token_hash,
            )
            .execute(&mut *tx)
            .await?;
        }

        // Counts and the new version, never a hash or a token (rule 3).
        debug!(
            user_id = %id,
            auth_version = user.auth_version,
            revoked,
            spent,
            replaced = replacement.is_some(),
            "password changed",
        );

        Ok(user)
    }

    /// Delete a user, reporting whether a row matched.
    ///
    /// `Ok(false)` rather than an error when nothing matched: the route turns
    /// that into 404. This never checks the administrator invariant — see the
    /// module documentation — and both token tables cascade.
    pub async fn delete(&self, tx: &mut PgConnection, id: Uuid) -> Result<bool> {
        let result = sqlx::query!("DELETE FROM users WHERE id = $1", id)
            .execute(&mut *tx)
            .await?;

        let deleted = result.rows_affected() > 0;
        debug!(user_id = %id, deleted, "user deleted");

        Ok(deleted)
    }

    /// How many administrators exist right now.
    ///
    /// Takes the caller's connection because the only correct way to use it is
    /// inside the transaction that holds
    /// [`UserRepository::lock_admin_membership`]: a count read outside that
    /// lock is stale the moment it is returned.
    pub async fn count_admins(&self, tx: &mut PgConnection) -> Result<i64> {
        let count = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "count!" FROM users WHERE admin"#)
            .fetch_one(&mut *tx)
            .await?;

        Ok(count)
    }

    /// Lock the user row `FOR UPDATE` and return it as read under the lock.
    ///
    /// Every credential mutation serialises here (`docs/data-model.md`, "Users
    /// and authentication"): login, refresh, reset-link issuance, password
    /// changes and reset-token consumption lock the user row before locking or
    /// writing that user's token rows, and revalidate against the row this
    /// returns rather than against anything read earlier. The lock is held
    /// until the caller's transaction commits or rolls back.
    ///
    /// An unlocked [`UserRepository::find`] may locate the user first; the
    /// value it returned is not authoritative and must be replaced by this one.
    ///
    /// When the same transaction also needs the administrator-membership lock,
    /// take that one **first** — see
    /// [`UserRepository::lock_admin_membership`].
    pub async fn lock_user(&self, tx: &mut PgConnection, id: Uuid) -> Result<Option<User>> {
        let user = sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, password_hash, auth_version,
                   must_change_password, admin, notify_email, created_at, updated_at
            FROM users
            WHERE id = $1
            FOR UPDATE
            "#,
            id,
        )
        .fetch_optional(&mut *tx)
        .await?;

        debug!(user_id = %id, found = user.is_some(), "user row locked");

        Ok(user)
    }

    /// Take the transaction-scoped advisory lock that serialises every change
    /// to administrator membership.
    ///
    /// Deleting a user and changing `users.admin` have to preserve at least one
    /// administrator, and that cannot be a per-row `CHECK`: two concurrent
    /// requests each demoting one of the final two administrators would both
    /// see a count of two. So both operations take this single lock, then
    /// re-read the count under it, then mutate, and hold it through commit
    /// (`docs/data-model.md`, "Users and authentication").
    ///
    /// **Lock order.** Acquire this before any user-row lock
    /// ([`UserRepository::lock_user`]) or project lock in the same
    /// transaction. Everything that takes both takes them in this order, which
    /// is what keeps the pair deadlock-free.
    ///
    /// `pg_advisory_xact_lock` releases at commit or rollback; there is
    /// nothing for the caller to unlock, and a connection returned to the pool
    /// mid-transaction cannot leak the lock.
    pub async fn lock_admin_membership(&self, tx: &mut PgConnection) -> Result<()> {
        sqlx::query!(
            "SELECT pg_advisory_xact_lock($1)",
            ADMIN_MEMBERSHIP_LOCK_KEY
        )
        .execute(&mut *tx)
        .await?;

        debug!("administrator membership locked");

        Ok(())
    }
}

/// Map the two `users` unique constraints to the conflicts `SPEC.md` promises.
///
/// Anything else — including a unique violation on a constraint this function
/// does not know — widens through `#[from] sqlx::Error`, which logs the detail
/// once and answers a generic 500 rather than guessing at a client message.
fn map_duplicate(err: sqlx::Error) -> Error {
    match unique_violation(&err) {
        Some("users_username_key") => Error::Conflict("username already taken".into()),
        Some("users_email_key") => Error::Conflict("email already registered".into()),
        _ => Error::from(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_admin_lock_key_is_a_fixed_constant() {
        // Changing it silently would stop serialising against a Mars process
        // still running the old value, so it is pinned by a test.
        assert_eq!(ADMIN_MEMBERSHIP_LOCK_KEY, 5_566_821_132_273_927_506);
    }
}
