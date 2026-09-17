//! All SQL against `user_invites` (`docs/data-model.md`, `user_invites`).
//!
//! An invite is the only way a user comes into existence (ADR 0013), so this
//! file carries the whole life cycle: an admin creates one for an email
//! address, the invitee follows the emailed link, and accepting it inserts the
//! user. Two rules from `docs/data-model.md` shape nearly every statement
//! here:
//!
//! - *"At most one open invite per email."* `user_invites_open_email_idx` is a
//!   partial unique index over `email WHERE accepted_at IS NULL`, so the
//!   database — not a pre-read — decides the duplicate, and a violation
//!   becomes the [`Error::Conflict`] `SPEC.md`, "Users" promises for `POST
//!   /users/invites`.
//! - *"Inviting an email that already belongs to a user is refused with a
//!   conflict."* No constraint can say that, because it spans two tables, so
//!   [`UserInviteRepository::insert`] carries the check in the statement
//!   itself rather than in a separate `SELECT` the caller would race against.
//!
//! "Open" means *unaccepted and unexpired*, and the two halves are not the
//! same thing: the partial index treats an expired, unaccepted invite as
//! occupying its address until the reaper deletes it, while every read here
//! excludes it. That is why re-inviting an address takes
//! [`UserInviteRepository::delete_expired_open_for_email`] first — see its
//! documentation.
//!
//! Acceptance locks the *invite* row rather than a user row, because the user
//! it creates does not exist yet; everything else that touches credentials
//! locks the user row first (`docs/data-model.md`, "Users and
//! authentication"; ADR 0025).

use chrono::{DateTime, TimeDelta, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::{Email, UserInvite};
use crate::prelude::*;
use crate::repositories::conflict_on;

/// How long an invite stays valid, in days (`docs/data-model.md`,
/// `user_invites`: "Creation time plus 7 days"; `SPEC.md`, "User-facing
/// features").
pub const INVITE_TTL_DAYS: i64 = 7;

/// The partial unique index that enforces one open invite per address.
const OPEN_EMAIL_INDEX: &str = "user_invites_open_email_idx";

/// The 409 message for a second open invite to the same address.
const OPEN_INVITE_EXISTS: &str = "an open invite already exists for this email";

/// The 409 message for inviting an address that is already a user.
const EMAIL_BELONGS_TO_A_USER: &str = "email already belongs to a user";

/// When an invite created at `now` expires.
///
/// The expiry is a column rather than a rule the reader applies, so it is
/// computed once here and passed to [`UserInviteRepository::insert`] and
/// [`UserInviteRepository::rotate_token`]; the invite route and the resend
/// route get the same 7 days without repeating the arithmetic.
pub fn expires_at(now: DateTime<Utc>) -> DateTime<Utc> {
    now + TimeDelta::days(INVITE_TTL_DAYS)
}

/// All SQL against `user_invites` (`ARCHITECTURE.md`, "Orchestrator
/// internals").
///
/// Reads that stand alone go to the pool; the statements that have to share a
/// transaction with an insert into `users` — the acceptance path — take the
/// caller's `&mut PgConnection`.
pub struct UserInviteRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> UserInviteRepository<'a> {
    /// Borrow `pool` for the lifetime of this repository.
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    /// Insert an invite for `email` and return the stored row.
    ///
    /// `token_hash` is the SHA-256 hex of the token in the emailed link; the
    /// raw token is never stored (`CLAUDE.md`, rule 3). `expires_at` is
    /// normally [`expires_at`] of the current time.
    ///
    /// Both conflicts `SPEC.md`, "Users" lists for `POST /users/invites` are
    /// decided by this one statement, neither by a pre-read:
    ///
    /// - An address that already belongs to a user is excluded by the
    ///   `WHERE NOT EXISTS` guard, which makes the insert affect no row; that
    ///   is the only way it can affect none, so `None` means exactly `email
    ///   already belongs to a user`. A separate `SELECT` would leave a window
    ///   between the read and the insert.
    /// - A second open invite for the address violates
    ///   `user_invites_open_email_idx` and is mapped by constraint name.
    ///
    /// Takes the caller's transaction because the invite route's other half
    /// lives there too: an expired invite is reaped out of the partial index's
    /// way by [`UserInviteRepository::delete_expired_open_for_email`] first,
    /// and the two statements have to commit together or the address is left
    /// with no invite at all.
    pub async fn insert(
        &self,
        tx: &mut PgConnection,
        email: &Email,
        token_hash: &str,
        admin: bool,
        invited_by: Option<Uuid>,
        expires_at: DateTime<Utc>,
    ) -> Result<UserInvite> {
        let inserted = sqlx::query_as!(
            UserInvite,
            r#"
            INSERT INTO user_invites (id, email, token_hash, admin, invited_by, expires_at)
            SELECT $1, $2, $3, $4, $5, $6
            WHERE NOT EXISTS (SELECT 1 FROM users WHERE email = $2)
            RETURNING id, email, token_hash, admin, invited_by, expires_at,
                      accepted_at, accepted_user_id, created_at
            "#,
            Uuid::new_v4(),
            email.as_str(),
            token_hash,
            admin,
            invited_by,
            expires_at,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| conflict_on(err, OPEN_EMAIL_INDEX, OPEN_INVITE_EXISTS))?
        .ok_or_else(|| Error::Conflict(EMAIL_BELONGS_TO_A_USER.to_string()))?;

        // The address is personal data and the hash is a credential: neither
        // belongs in a log line (`CLAUDE.md`, "Backend conventions", rule 3).
        debug!(invite_id = %inserted.id, admin = inserted.admin, "invite inserted");

        Ok(inserted)
    }

    /// Every open invite, newest first (`GET /users/invites`).
    ///
    /// Expired invites are not open even though they still occupy the partial
    /// unique index, so the admin list shows what can still be accepted rather
    /// than what the reaper has not got to yet.
    pub async fn list_open(&self) -> Result<Vec<UserInvite>> {
        let invites = sqlx::query_as!(
            UserInvite,
            r#"
            SELECT id, email, token_hash, admin, invited_by, expires_at,
                   accepted_at, accepted_user_id, created_at
            FROM user_invites
            WHERE accepted_at IS NULL AND expires_at > NOW()
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(invites)
    }

    /// The open invite with this token hash, or `None` (`GET
    /// /auth/invite/{token}`).
    ///
    /// Unlocked: this answers the preview the invitee's browser asks for
    /// before it submits anything, and the answer is `{email, admin,
    /// expires_at}` or a 400. Acceptance must not build on it — it re-reads
    /// under [`UserInviteRepository::lock_open_by_hash`].
    pub async fn find_open_by_hash(&self, token_hash: &str) -> Result<Option<UserInvite>> {
        let invite = sqlx::query_as!(
            UserInvite,
            r#"
            SELECT id, email, token_hash, admin, invited_by, expires_at,
                   accepted_at, accepted_user_id, created_at
            FROM user_invites
            WHERE token_hash = $1 AND accepted_at IS NULL AND expires_at > NOW()
            "#,
            token_hash,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(invite)
    }

    /// The open invite with this id, or `None`.
    ///
    /// The same view as [`UserInviteRepository::find_open_by_hash`] for the
    /// admin-side routes, which know the id rather than the token.
    pub async fn find_open_by_id(&self, id: Uuid) -> Result<Option<UserInvite>> {
        let invite = sqlx::query_as!(
            UserInvite,
            r#"
            SELECT id, email, token_hash, admin, invited_by, expires_at,
                   accepted_at, accepted_user_id, created_at
            FROM user_invites
            WHERE id = $1 AND accepted_at IS NULL AND expires_at > NOW()
            "#,
            id,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(invite)
    }

    /// Lock the open invite with this token hash `FOR UPDATE` and return it as
    /// read under the lock.
    ///
    /// The acceptance transaction starts here. There is no user row to lock
    /// yet, so the invite row is what serialises two browsers submitting the
    /// same link: the loser waits, then finds `accepted_at` set and gets
    /// `None` from this very statement. The lock is held until the caller's
    /// transaction commits or rolls back, which is the same transaction that
    /// inserts the user and calls [`UserInviteRepository::mark_accepted`].
    pub async fn lock_open_by_hash(
        &self,
        tx: &mut PgConnection,
        token_hash: &str,
    ) -> Result<Option<UserInvite>> {
        let invite = sqlx::query_as!(
            UserInvite,
            r#"
            SELECT id, email, token_hash, admin, invited_by, expires_at,
                   accepted_at, accepted_user_id, created_at
            FROM user_invites
            WHERE token_hash = $1 AND accepted_at IS NULL AND expires_at > NOW()
            FOR UPDATE
            "#,
            token_hash,
        )
        .fetch_optional(&mut *tx)
        .await?;

        debug!(found = invite.is_some(), "invite row locked");

        Ok(invite)
    }

    /// Replace an unaccepted invite's token and expiry, returning the stored
    /// row or `None` (`POST /users/invites/{id}/resend`).
    ///
    /// Resending issues a new token rather than re-sending the old one, so the
    /// link in the earlier email stops working the moment this commits: one
    /// invite is one live token. An expired invite may be resent — that is the
    /// point of resending — so only `accepted_at IS NULL` is required, and an
    /// accepted invite yields `None`, which the route answers with 404.
    pub async fn rotate_token(
        &self,
        id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<Option<UserInvite>> {
        let rotated = sqlx::query_as!(
            UserInvite,
            r#"
            UPDATE user_invites
            SET token_hash = $2, expires_at = $3
            WHERE id = $1 AND accepted_at IS NULL
            RETURNING id, email, token_hash, admin, invited_by, expires_at,
                      accepted_at, accepted_user_id, created_at
            "#,
            id,
            token_hash,
            expires_at,
        )
        .fetch_optional(self.pool)
        .await?;

        debug!(invite_id = %id, rotated = rotated.is_some(), "invite token rotated");

        Ok(rotated)
    }

    /// Revoke an unaccepted invite, reporting whether a row matched (`DELETE
    /// /users/invites/{id}`).
    ///
    /// An accepted invite is not deleted: it is the record of how a user came
    /// to exist, and deleting it would not un-create them. `Ok(false)` is the
    /// route's 404.
    pub async fn delete_open(&self, id: Uuid) -> Result<bool> {
        let result = sqlx::query!(
            "DELETE FROM user_invites WHERE id = $1 AND accepted_at IS NULL",
            id,
        )
        .execute(self.pool)
        .await?;

        let deleted = result.rows_affected() > 0;
        debug!(invite_id = %id, deleted, "invite revoked");

        Ok(deleted)
    }

    /// Delete this address's expired, unaccepted invites, returning how many
    /// rows went.
    ///
    /// `user_invites_open_email_idx` is partial on `accepted_at IS NULL` and
    /// knows nothing about `expires_at`, so a week-old unaccepted invite still
    /// blocks a new one for the same address until the reaper gets to it. The
    /// invite route therefore clears the address itself, in the transaction
    /// that inserts the replacement: without this, re-inviting someone who
    /// never clicked their link would be refused over an invite that can no
    /// longer be accepted.
    ///
    /// Only *expired* rows go. A live invite is a real conflict and the caller
    /// is meant to see it.
    pub async fn delete_expired_open_for_email(
        &self,
        tx: &mut PgConnection,
        email: &Email,
    ) -> Result<u64> {
        let result = sqlx::query!(
            r#"
            DELETE FROM user_invites
            WHERE email = $1 AND accepted_at IS NULL AND expires_at <= NOW()
            "#,
            email.as_str(),
        )
        .execute(&mut *tx)
        .await?;

        let deleted = result.rows_affected();
        debug!(deleted, "expired invites cleared for an email");

        Ok(deleted)
    }

    /// Mark an invite accepted by `user_id`, reporting whether a row matched.
    ///
    /// The `accepted_at IS NULL` in the `WHERE` is the single-use rule, and it
    /// is what makes the acceptance transaction safe even though the caller
    /// read the invite earlier: `Ok(false)` means another transaction accepted
    /// it in between, and the caller answers `Error::BadRequest("invalid or
    /// expired invite")` and rolls back the user it was about to create. Under
    /// [`UserInviteRepository::lock_open_by_hash`] that race cannot happen;
    /// the guard costs nothing and does not rely on the caller having locked.
    pub async fn mark_accepted(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        user_id: Uuid,
    ) -> Result<bool> {
        let result = sqlx::query!(
            r#"
            UPDATE user_invites
            SET accepted_at = NOW(), accepted_user_id = $2
            WHERE id = $1 AND accepted_at IS NULL
            "#,
            id,
            user_id,
        )
        .execute(&mut *tx)
        .await?;

        let accepted = result.rows_affected() > 0;
        debug!(invite_id = %id, user_id = %user_id, accepted, "invite accepted");

        Ok(accepted)
    }

    /// Delete every expired, unaccepted invite, returning how many rows went.
    ///
    /// The hourly token-cleanup job (`ARCHITECTURE.md`, "Background jobs").
    /// Accepted invites are kept: they are history, not garbage.
    pub async fn delete_expired(&self) -> Result<u64> {
        let result = sqlx::query!(
            "DELETE FROM user_invites WHERE accepted_at IS NULL AND expires_at <= NOW()",
        )
        .execute(self.pool)
        .await?;

        let deleted = result.rows_affected();
        debug!(deleted, "expired invites deleted");

        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invite_expires_seven_days_after_it_is_created() {
        let now = Utc::now();
        assert_eq!(expires_at(now) - now, TimeDelta::days(7));
    }
}
