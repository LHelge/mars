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
//! Token-table statements beyond the row types themselves belong to the
//! authentication epic.

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

    /// Every user, oldest first (`GET /users`).
    ///
    /// `id` breaks ties so two users created in the same transaction — and
    /// therefore sharing `NOW()` — still come back in a stable order.
    pub async fn list(&self) -> Result<Vec<User>> {
        let users = sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, password_hash, auth_version,
                   must_change_password, admin, notify_email, created_at, updated_at
            FROM users
            ORDER BY created_at, id
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
