//! All SQL against `refresh_tokens` (`docs/data-model.md`, `refresh_tokens`).
//!
//! The table stores the SHA-256 hex of an opaque token, never the token
//! itself, so every lookup here is by hash and nothing in this module can
//! reconstruct a credential. Neither a hash nor a raw token is ever logged
//! (`CLAUDE.md`, rule 3): the log lines below carry the user id and a count.
//!
//! **Lock order.** `docs/data-model.md`, "Users and authentication" requires
//! that login, refresh, reset-link issuance, password changes and
//! reset-token consumption "lock the user row before locking or writing that
//! user's token rows". So a caller takes
//! [`crate::repositories::UserRepository::lock_user`] first and only then
//! calls the transaction-taking methods here; when the administrator-membership
//! advisory lock is also needed it comes before both. An unlocked
//! [`RefreshTokenRepository::find_by_hash`] may locate the row first — that is
//! how refresh finds the user to lock — but the value it returned is not
//! authoritative and has to be re-read with
//! [`RefreshTokenRepository::find_by_hash_for_user`] under the lock before any
//! credential is issued.
//!
//! The revocation half of a password change is not here: it belongs to the one
//! statement sequence that has to be atomic with the hash update, and lives in
//! [`crate::repositories::UserRepository::apply_password_change`].

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::RefreshToken;
use crate::prelude::*;

/// All SQL against `refresh_tokens` (`ARCHITECTURE.md`, "Orchestrator
/// internals").
///
/// Reads that need no transaction go straight to the pool; everything that has
/// to happen under the caller's user-row lock takes the caller's
/// `&mut PgConnection`, so one transaction can hold the whole composition —
/// lock the user, re-read the token, revoke it, insert its replacement —
/// together.
pub struct RefreshTokenRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> RefreshTokenRepository<'a> {
    /// Borrow `pool` for the lifetime of this repository.
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    /// Insert a refresh token for `user_id` and return the stored row.
    ///
    /// `token_hash` is the SHA-256 hex of the raw token that goes into the
    /// cookie; the caller computes it and keeps the raw token to itself.
    /// `expires_at` is the caller's, because login, invite acceptance and
    /// refresh all issue a 30-day token from *their* now (`SPEC.md`,
    /// "Authentication").
    ///
    /// Always inside the transaction that holds the user-row lock: a token
    /// inserted outside it could be issued to a browser after a concurrent
    /// password change has already revoked everything (ADR 0025).
    ///
    /// The id is generated here — nothing outside needs to know it before the
    /// insert. A collision on `refresh_tokens_token_hash_key` is not a client
    /// mistake but a broken token generator, so it widens to 500 like any
    /// other database error rather than becoming a conflict.
    pub async fn insert(
        &self,
        tx: &mut PgConnection,
        user_id: Uuid,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<RefreshToken> {
        let inserted = sqlx::query_as!(
            RefreshToken,
            r#"
            INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at)
            VALUES ($1, $2, $3, $4)
            RETURNING id, user_id, token_hash, expires_at, revoked_at, created_at
            "#,
            Uuid::new_v4(),
            user_id,
            token_hash,
            expires_at,
        )
        .fetch_one(&mut *tx)
        .await?;

        debug!(user_id = %user_id, "refresh token inserted");

        Ok(inserted)
    }

    /// The token row with this hash, or `None`, read without any lock.
    ///
    /// This is the first half of a refresh: the request arrives with a cookie
    /// and nothing else, so the row is what names the user to lock. Whether
    /// the token is still usable is *not* decided from what this returns — see
    /// [`RefreshTokenRepository::find_by_hash_for_user`].
    pub async fn find_by_hash(&self, token_hash: &str) -> Result<Option<RefreshToken>> {
        let token = sqlx::query_as!(
            RefreshToken,
            r#"
            SELECT id, user_id, token_hash, expires_at, revoked_at, created_at
            FROM refresh_tokens
            WHERE token_hash = $1
            "#,
            token_hash,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(token)
    }

    /// Re-read the token under the caller's user-row lock, or `None`.
    ///
    /// The authoritative read: `revoked_at` and `expires_at` are only worth
    /// checking on the row this returns, because only this one cannot have
    /// been revoked by a password change that committed while the caller was
    /// waiting for the lock (`docs/data-model.md`; ADR 0025). Callers apply
    /// [`RefreshToken::is_usable`] to it and never to an earlier read.
    ///
    /// `user_id` is in the `WHERE` clause rather than compared afterwards, so
    /// a token belonging to another user is simply not found: the scope is the
    /// statement's (`ARCHITECTURE.md`, "Orchestrator internals").
    pub async fn find_by_hash_for_user(
        &self,
        tx: &mut PgConnection,
        token_hash: &str,
        user_id: Uuid,
    ) -> Result<Option<RefreshToken>> {
        let token = sqlx::query_as!(
            RefreshToken,
            r#"
            SELECT id, user_id, token_hash, expires_at, revoked_at, created_at
            FROM refresh_tokens
            WHERE token_hash = $1 AND user_id = $2
            "#,
            token_hash,
            user_id,
        )
        .fetch_optional(&mut *tx)
        .await?;

        Ok(token)
    }

    /// Revoke one token by id, reporting whether this call was the one that
    /// revoked it.
    ///
    /// `revoked_at IS NULL` is part of the `WHERE` clause, so an already
    /// revoked token is `Ok(false)` and its original revocation time survives.
    /// A second revocation is a no-op, never an error: refresh rotates the
    /// presented token, and a client that retries a request whose response it
    /// never saw must not get a 500.
    pub async fn revoke(&self, tx: &mut PgConnection, id: Uuid) -> Result<bool> {
        let result = sqlx::query!(
            "UPDATE refresh_tokens SET revoked_at = NOW() WHERE id = $1 AND revoked_at IS NULL",
            id,
        )
        .execute(&mut *tx)
        .await?;

        let revoked = result.rows_affected() > 0;
        debug!(revoked, "refresh token revoked by id");

        Ok(revoked)
    }

    /// Revoke one token by hash, reporting whether this call revoked it.
    ///
    /// Logout's statement: the cookie is all the request carries, and a logout
    /// neither needs the user-row lock nor has anything to revalidate, so it
    /// goes straight to the pool. [`RefreshTokenRepository::revoke`] is the
    /// one to use inside a transaction that already holds the lock.
    pub async fn revoke_by_hash(&self, token_hash: &str) -> Result<bool> {
        let result = sqlx::query!(
            "UPDATE refresh_tokens SET revoked_at = NOW() WHERE token_hash = $1 AND revoked_at IS NULL",
            token_hash,
        )
        .execute(self.pool)
        .await?;

        let revoked = result.rows_affected() > 0;
        debug!(revoked, "refresh token revoked by hash");

        Ok(revoked)
    }

    /// Revoke every outstanding token of one user, returning how many rows
    /// changed.
    ///
    /// "Sign out everywhere", and the statement a caller that revokes without
    /// changing the password needs. The password change has its own copy
    /// inside [`crate::repositories::UserRepository::apply_password_change`],
    /// which cannot delegate: the revocation has to be in the same statement
    /// sequence as the hash update and the `auth_version` increment.
    pub async fn revoke_all_for_user(&self, tx: &mut PgConnection, user_id: Uuid) -> Result<u64> {
        let result = sqlx::query!(
            "UPDATE refresh_tokens SET revoked_at = NOW() WHERE user_id = $1 AND revoked_at IS NULL",
            user_id,
        )
        .execute(&mut *tx)
        .await?;

        let revoked = result.rows_affected();
        debug!(user_id = %user_id, revoked, "refresh tokens revoked for user");

        Ok(revoked)
    }

    /// Delete every token that has expired, returning how many rows went.
    ///
    /// The token-cleanup cron job's statement (`ARCHITECTURE.md`, "Background
    /// jobs": "Delete expired refresh tokens, reset tokens, unaccepted
    /// invites..."). Only expiry decides: a revoked but unexpired row is still
    /// evidence that its hash has been used, and keeping it until `expires_at`
    /// costs one row while `refresh_tokens_token_hash_key` keeps a replayed
    /// token from being re-inserted. `refresh_tokens_expires_at_idx` serves
    /// the scan. Deleting a user cascades the rest.
    pub async fn delete_expired(&self) -> Result<u64> {
        let result = sqlx::query!("DELETE FROM refresh_tokens WHERE expires_at < NOW()")
            .execute(self.pool)
            .await?;

        let deleted = result.rows_affected();
        debug!(deleted, "expired refresh tokens deleted");

        Ok(deleted)
    }
}
