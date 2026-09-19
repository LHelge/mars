//! All SQL against `password_reset_tokens` (`docs/data-model.md`,
//! `password_reset_tokens`).
//!
//! The table is small and so is this file; what matters is the order its
//! statements are used in. `docs/data-model.md` requires that "a token must be
//! unexpired and have null `used_at` when revalidated under the user lock",
//! and ADR 0025 that "credential issuance locks the user row and revalidates
//! after locking". Consuming a reset link is therefore three steps, not one:
//!
//! 1. [`PasswordResetTokenRepository::find_by_hash`], unlocked, only to learn
//!    *which user* the token belongs to — a token is the only thing the caller
//!    presents, and there is no row to lock until the user is known.
//! 2. `UserRepository::lock_user` on that user.
//! 3. [`PasswordResetTokenRepository::find_valid_by_hash_for_user`], which
//!    re-reads the same token under that lock and is the read the decision is
//!    actually made on. The value from step 1 is stale by construction.
//!
//! Step 3 also pins the token to the user locked in step 2, so a token that
//! belongs to somebody else cannot be spent against this lock.
//!
//! The password transaction then updates the hash, increments
//! `users.auth_version`, revokes the refresh tokens and calls
//! [`PasswordResetTokenRepository::mark_all_used_for_user`], all committing
//! together (`SPEC.md`, "Authentication").

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::PasswordResetToken;
use crate::prelude::*;

/// When a reset link issued at `now` expires (`docs/data-model.md`,
/// `password_reset_tokens`).
///
/// The counterpart of [`crate::repositories::user_invites::expires_at`], and
/// the only place [`PASSWORD_RESET_TTL`] is added to a clock: the lifetime is
/// written down once and the arithmetic happens once.
pub fn expires_at(now: DateTime<Utc>) -> DateTime<Utc> {
    now + PASSWORD_RESET_TTL
}

/// All SQL against `password_reset_tokens` (`ARCHITECTURE.md`, "Orchestrator
/// internals").
///
/// Only [`PasswordResetTokenRepository::find_by_hash`] and
/// [`PasswordResetTokenRepository::delete_expired`] go to the pool; everything
/// else takes the caller's `&mut PgConnection`, because everything else is
/// part of a transaction that holds the user row lock.
pub struct PasswordResetTokenRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> PasswordResetTokenRepository<'a> {
    /// Borrow `pool` for the lifetime of this repository.
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    /// Insert a reset token and return the stored row.
    ///
    /// `token_hash` is the SHA-256 hex of the token in the emailed link; the
    /// raw token is never stored (`CLAUDE.md`, rule 3). Takes the caller's
    /// transaction because issuing a link happens under the user row lock, so
    /// that a concurrent password change cannot commit between the lock's
    /// revalidation and this insert and leave a live link behind (ADR 0025).
    ///
    /// Two outstanding tokens for one user are legitimate — the reset endpoint
    /// allows three requests an hour — and consuming either one invalidates
    /// both through
    /// [`PasswordResetTokenRepository::mark_all_used_for_user`].
    pub async fn insert(
        &self,
        tx: &mut PgConnection,
        user_id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<PasswordResetToken> {
        let inserted = sqlx::query_as!(
            PasswordResetToken,
            r#"
            INSERT INTO password_reset_tokens (id, user_id, token_hash, expires_at)
            VALUES ($1, $2, $3, $4)
            RETURNING id, user_id, token_hash, expires_at, used_at, created_at
            "#,
            Uuid::new_v4(),
            user_id,
            token_hash,
            expires_at,
        )
        .fetch_one(&mut *tx)
        .await?;

        debug!(user_id = %user_id, "password reset token inserted");

        Ok(inserted)
    }

    /// The token row with this hash in any state, or `None`.
    ///
    /// Unlocked, and deliberately unfiltered: its only job is to turn the
    /// token the caller presented into a `user_id` to lock. The row it returns
    /// says nothing authoritative about whether the token may be spent — that
    /// is [`PasswordResetTokenRepository::find_valid_by_hash_for_user`], under
    /// the lock. `None` here and a `None` from that revalidation are the same
    /// answer to the caller: `POST /auth/reset-password` refuses an unknown,
    /// spent and expired token identically, so that the response never says
    /// which one it was.
    pub async fn find_by_hash(&self, token_hash: &str) -> Result<Option<PasswordResetToken>> {
        let token = sqlx::query_as!(
            PasswordResetToken,
            r#"
            SELECT id, user_id, token_hash, expires_at, used_at, created_at
            FROM password_reset_tokens
            WHERE token_hash = $1
            "#,
            token_hash,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(token)
    }

    /// The unspent, unexpired token with this hash *belonging to this user*,
    /// or `None`.
    ///
    /// The authoritative read, executed after `UserRepository::lock_user` has
    /// taken the row lock on `user_id`: it is the "unexpired and null
    /// `used_at` when revalidated under the user lock" of
    /// `docs/data-model.md`. `None` is the caller's 400, whatever the reason —
    /// unknown, spent, expired or another user's — because distinguishing them
    /// would tell an attacker which.
    pub async fn find_valid_by_hash_for_user(
        &self,
        tx: &mut PgConnection,
        token_hash: &str,
        user_id: Uuid,
    ) -> Result<Option<PasswordResetToken>> {
        let token = sqlx::query_as!(
            PasswordResetToken,
            r#"
            SELECT id, user_id, token_hash, expires_at, used_at, created_at
            FROM password_reset_tokens
            WHERE token_hash = $1 AND user_id = $2 AND used_at IS NULL AND expires_at > NOW()
            "#,
            token_hash,
            user_id,
        )
        .fetch_optional(&mut *tx)
        .await?;

        debug!(user_id = %user_id, valid = token.is_some(), "reset token revalidated");

        Ok(token)
    }

    /// Spend every outstanding token of this user, returning how many rows
    /// moved.
    ///
    /// Single use is per *user*, not per token (`docs/data-model.md`): every
    /// successful password change or reset invalidates all outstanding links,
    /// so a second link mailed a minute later cannot be used afterwards. Part
    /// of the password transaction, hence the caller's connection; already
    /// spent rows are left alone so `used_at` keeps saying when the token was
    /// actually spent.
    pub async fn mark_all_used_for_user(
        &self,
        tx: &mut PgConnection,
        user_id: Uuid,
    ) -> Result<u64> {
        let result = sqlx::query!(
            r#"
            UPDATE password_reset_tokens
            SET used_at = NOW()
            WHERE user_id = $1 AND used_at IS NULL
            "#,
            user_id,
        )
        .execute(&mut *tx)
        .await?;

        let spent = result.rows_affected();
        debug!(user_id = %user_id, spent, "password reset tokens invalidated");

        Ok(spent)
    }

    /// Delete every expired token, returning how many rows went.
    ///
    /// The hourly token-cleanup job (`ARCHITECTURE.md`, "Background jobs").
    /// Spent-but-unexpired rows stay until they expire, so that replaying a
    /// link within its window is still recognised as spent rather than as
    /// unknown.
    pub async fn delete_expired(&self) -> Result<u64> {
        let result = sqlx::query!("DELETE FROM password_reset_tokens WHERE expires_at <= NOW()")
            .execute(self.pool)
            .await?;

        let deleted = result.rows_affected();
        debug!(deleted, "expired password reset tokens deleted");

        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reset_link_expires_an_hour_after_it_is_issued() {
        let now = Utc::now();
        assert_eq!(expires_at(now) - now, chrono::TimeDelta::hours(1));
    }
}
