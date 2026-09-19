//! All SQL against `users`, plus the two locks the authentication epic
//! composes its invariants from.
//!
//! `docs/data-model.md`, "Users and authentication" is the contract. Two
//! paragraphs of it are the reason this file has locking primitives at all:
//!
//! - *Administrator membership.* "User deletion and changes to `users.admin`
//!   preserve at least one administrator. Before authoritative reads or
//!   writes, these operations acquire the same transaction-scoped database
//!   advisory lock for administrator membership; after waiting, re-read the
//!   current administrator count and reject a deletion or demotion that would
//!   leave none. Hold the lock through commit, and acquire it before any
//!   user-row or project locks needed by the operation. **This is a repository
//!   invariant**, not a per-row `CHECK`." So it is held here, whole:
//!   [`UserRepository::replace`] and [`UserRepository::delete`] own the
//!   transaction that locks, counts, mutates and commits, and a caller that
//!   can call either cannot leave the database without an administrator. The
//!   lock and the count are private for the same reason — a caller holding the
//!   pieces is a caller that can drop one.
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

use chrono::Utc;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::{Email, NewUser, User, UserUpdate, Username};
use crate::prelude::*;
use crate::repositories::unique_violation;

/// The 409 for demoting the last administrator (`SPEC.md`, "Users").
const LAST_ADMIN_DEMOTION: &str = "cannot demote the last administrator";

/// The 409 for deleting the last administrator.
const LAST_ADMIN_DELETION: &str = "cannot delete the last administrator";

/// The 409 for deleting your own account. Separate from
/// [`LAST_ADMIN_DELETION`]: self-deletion is refused even when ten other
/// administrators remain, so no count can make it legal.
const SELF_DELETION: &str = "cannot delete yourself";

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
/// has to be read or written under someone else's lock takes the caller's
/// `&mut PgConnection`, so one transaction — a password change, say — can hold
/// a whole composition together. The two mutations that carry the
/// administrator-membership invariant are the other way round: they open their
/// own transaction, because the lock, the count and the mutation are not
/// separable.
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

    /// Every administrator who still wants email, for an escalation that has
    /// no assignee to send to.
    ///
    /// `ARCHITECTURE.md`, "Task tracker" → "Notification": a move into the
    /// human state goes to the task's assignee if it has one, "otherwise to
    /// every admin, skipping users whose `notify_email` is off". Both
    /// conditions are in the `WHERE` clause rather than in a filter over
    /// [`UserRepository::list`], so the sender never holds a row it must not
    /// write to.
    ///
    /// Ordered by username like the listing, so a test asserting several
    /// recipients reads them in a fixed order.
    pub async fn list_admin_recipients(&self) -> Result<Vec<User>> {
        let users = sqlx::query_as!(
            User,
            r#"
            SELECT id, username, email, password_hash, auth_version,
                   must_change_password, admin, notify_email, created_at, updated_at
            FROM users
            WHERE admin AND notify_email
            ORDER BY username
            "#,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(users)
    }

    /// Apply the set fields of `update` and return the stored row, or `None`
    /// when no user has this id (the caller decides whether that is a 404).
    ///
    /// `COALESCE` per column keeps "leave it alone" in the statement rather
    /// than in a query builder, so one prepared statement serves `PUT
    /// /users/{id}` and `PATCH /users/me`. `updated_at` moves on every call.
    ///
    /// Private, because it can clear `admin`: the administrator-membership
    /// invariant is this module's, and a statement that can break it is not
    /// something a caller gets to hold on its own
    /// (`docs/data-model.md`, "Users and authentication"). The two public
    /// mutations below are how it is reached.
    async fn update(
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

    /// Set the two administrator-settable fields of one user and return the
    /// stored row (`PUT /users/{id}`).
    ///
    /// The whole administrator-membership invariant, in one call: `BEGIN`, the
    /// advisory lock as the first statement, the target row `FOR UPDATE`, the
    /// count re-read under the lock, the update, `COMMIT`. A rejection returns
    /// before `commit`, so `tx` rolls back and `username` is not applied
    /// either — "a rejected request changes no fields" (`SPEC.md`, "Users").
    ///
    /// The count is only consulted for a true → false transition. Promoting,
    /// renaming or writing the same two values back cannot reduce the number
    /// of administrators, so they never fail the check — but they still take
    /// the lock, which is what makes a concurrent demotion wait for them
    /// rather than counting around them.
    ///
    /// A missing id is [`Error::NotFound`], a duplicate username the
    /// [`Error::Conflict`] [`UserRepository::insert`] describes, and the last
    /// administrator's demotion [`Error::Conflict`] with the message
    /// `SPEC.md`, "Users" fixes. Demoting *yourself* is allowed while another
    /// administrator remains: the acting user is not a parameter here, only in
    /// [`UserRepository::delete`].
    pub async fn replace(&self, id: Uuid, username: Username, admin: bool) -> Result<User> {
        let mut tx = self.pool.begin().await?;

        self.lock_admin_membership(&mut tx).await?;

        let Some(current) = self.lock_user(&mut tx, id).await? else {
            return Err(Error::NotFound);
        };

        if current.admin && !admin && self.count_admins(&mut tx).await? <= 1 {
            debug!(user_id = %id, "demotion refused: last administrator");
            return Err(Error::Conflict(LAST_ADMIN_DEMOTION.to_string()));
        }

        let update = UserUpdate {
            username: Some(username),
            admin: Some(admin),
            ..UserUpdate::default()
        };

        // `lock_user` already proved the row exists and holds it, so `None` is
        // unreachable; `NotFound` rather than a panic all the same.
        let updated = self
            .update(&mut tx, id, &update)
            .await?
            .ok_or(Error::NotFound)?;
        tx.commit().await?;

        Ok(updated)
    }

    /// Set the one field a user owns about themselves and return the stored
    /// row, or [`Error::NotFound`] (`PATCH /users/me`).
    ///
    /// No administrator-membership lock: `notify_email` is not `admin` and
    /// this cannot change how many administrators exist, so serialising every
    /// preference change against every role change would buy nothing.
    pub async fn set_notify_email(&self, id: Uuid, notify_email: bool) -> Result<User> {
        let update = UserUpdate {
            notify_email: Some(notify_email),
            ..UserUpdate::default()
        };

        let mut tx = self.pool.begin().await?;
        let updated = self
            .update(&mut tx, id, &update)
            .await?
            .ok_or(Error::NotFound)?;
        tx.commit().await?;

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
    /// The revocations and the spent reset tokens are stamped with the
    /// database's `NOW()`, which in Postgres is the start of this transaction,
    /// so they share one clock. The replacement's expiry is the one value that
    /// does not come from SQL: it is `REFRESH_TOKEN_TTL` from the
    /// orchestrator's clock, bound like every other parameter, so the thirty
    /// days are written down once and the cookie's `Max-Age` cannot drift from
    /// the row's `expires_at`.
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
        //
        // The expiry is bound rather than written as an `INTERVAL` literal:
        // [`REFRESH_TOKEN_TTL`] is the one place the thirty days are spelled,
        // and a lifetime repeated in SQL is a lifetime that will disagree with
        // the cookie's `Max-Age` one day.
        if let Some(token_hash) = replacement {
            sqlx::query!(
                r#"
                INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at)
                VALUES ($1, $2, $3, $4)
                "#,
                Uuid::new_v4(),
                id,
                token_hash,
                Utc::now() + REFRESH_TOKEN_TTL,
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

    /// Delete a user, or say why not (`DELETE /users/{id}`).
    ///
    /// The deletion half of the administrator-membership invariant, in the
    /// same shape as [`UserRepository::replace`]: advisory lock, target row,
    /// count, delete, commit. `acting_user_id` is here for one rule only —
    /// "self-deletion is separately prohibited" (`docs/data-model.md`) — and
    /// that rule is settled before the transaction opens, because it is not a
    /// question about administrator membership and there is nothing to lock or
    /// count.
    ///
    /// An unknown id is [`Error::NotFound`]; the two refusals are
    /// [`Error::Conflict`]. Both token tables cascade, and everything else the
    /// user owns follows the schema's foreign keys (`docs/data-model.md`).
    pub async fn delete(&self, id: Uuid, acting_user_id: Uuid) -> Result<()> {
        if id == acting_user_id {
            debug!(actor_id = %acting_user_id, "deletion refused: self");
            return Err(Error::Conflict(SELF_DELETION.to_string()));
        }

        let mut tx = self.pool.begin().await?;

        self.lock_admin_membership(&mut tx).await?;

        let Some(target) = self.lock_user(&mut tx, id).await? else {
            return Err(Error::NotFound);
        };

        if target.admin && self.count_admins(&mut tx).await? <= 1 {
            debug!(user_id = %id, "deletion refused: last administrator");
            return Err(Error::Conflict(LAST_ADMIN_DELETION.to_string()));
        }

        // `lock_user` holds the row, so this matches it; `NotFound` rather
        // than an assertion all the same.
        let deleted = sqlx::query!("DELETE FROM users WHERE id = $1", id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            > 0;
        if !deleted {
            return Err(Error::NotFound);
        }
        tx.commit().await?;

        debug!(user_id = %id, "user deleted");

        Ok(())
    }

    /// How many administrators exist right now.
    ///
    /// Takes the caller's connection because the only correct way to use it is
    /// inside the transaction that holds
    /// [`UserRepository::lock_admin_membership`]: a count read outside that
    /// lock is stale the moment it is returned. Private with that lock, and
    /// for the same reason.
    async fn count_admins(&self, tx: &mut PgConnection) -> Result<i64> {
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
    /// that one is taken **first**; the two mutations that need both,
    /// [`UserRepository::replace`] and [`UserRepository::delete`], take them in
    /// that order inside this module.
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
    /// Private: the only two operations that may take it are
    /// [`UserRepository::replace`] and [`UserRepository::delete`], which take
    /// it as their first statement. [`ADMIN_MEMBERSHIP_LOCK_KEY`] stays public
    /// so a test can hold the same lock from outside and prove they wait for
    /// it.
    ///
    /// **Lock order.** Acquired before the user-row lock
    /// ([`UserRepository::lock_user`]) and before any project lock in the same
    /// transaction. Everything that takes both takes them in this order, which
    /// is what keeps the pair deadlock-free.
    ///
    /// `pg_advisory_xact_lock` releases at commit or rollback; there is
    /// nothing to unlock by hand, and a connection returned to the pool
    /// mid-transaction cannot leak the lock.
    async fn lock_admin_membership(&self, tx: &mut PgConnection) -> Result<()> {
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
