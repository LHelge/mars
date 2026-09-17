//! All SQL against `secrets` and `secret_uses`.
//!
//! `docs/data-model.md`, "Secrets" is the column contract and
//! `ARCHITECTURE.md`, "Secrets" is the design. This file moves the encrypted
//! columns and nothing else: it never wraps, unwraps, encrypts or decrypts,
//! never touches the keyring and never sees a value in the clear. The
//! envelope-crypto epic hands it an already-built
//! [`crate::models::EncryptedValue`] and reads the bytes back out again.
//!
//! Three compositions are deliberately *not* here, because each is a policy
//! rather than a statement:
//!
//! - *Resolution at launch.* Looking a name up in the order `global`,
//!   `project`, `user` with the last one found winning, and skipping the name
//!   entirely when the winner is `orchestrator_only`, is the resolver's
//!   (`ARCHITECTURE.md`, "Secrets"). It composes that from
//!   [`SecretRepository::find_by_name`].
//! - *Renaming.* A rename re-encrypts the value under the new additional
//!   authenticated data, because the name is part of it
//!   (`docs/data-model.md`, `secrets`). [`SecretRepository::rename`] writes
//!   the new name and the re-encrypted columns in one statement; deciding what
//!   those bytes are, and doing it in one transaction with the decrypt, is the
//!   caller's.
//! - *Rotation.* [`SecretRepository::list_for_rotation`] selects a batch and
//!   [`SecretRepository::rewrap`] writes one row back; the unwrap/re-wrap
//!   between them, and the loop around them, belong to the `rotate-secrets`
//!   subcommand.
//!
//! No statement here logs a value, a ciphertext, a nonce or a key, and no
//! `tracing` field carries one: the only fields in this file are `secret_id`,
//! `name`, `scope` and counts (`CLAUDE.md`, rule 3).

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::{
    EncryptedValue, NewSecret, ScopeRef, Secret, SecretMeta, SecretName, SecretScope, SecretUse,
    SecretUsePurpose,
};
use crate::prelude::*;
use crate::repositories::unique_violation;

/// How many rows one rotation sweep takes at a time (`ARCHITECTURE.md`,
/// "Secrets", Rotation: "in batches of 100").
///
/// Rust has no default arguments, so this is the value the `rotate-secrets`
/// subcommand passes to [`SecretRepository::list_for_rotation`]; tests pass a
/// smaller one to exercise the bound.
pub const ROTATION_BATCH: i64 = 100;

/// All SQL against `secrets` and `secret_uses` (`ARCHITECTURE.md`,
/// "Orchestrator internals").
///
/// Reads that need no transaction go straight to the pool; every write takes
/// the caller's `&mut PgConnection`, because none of them is ever alone — an
/// insert accompanies a `secret_uses` row or a project creation, and a rename
/// accompanies the re-encryption it depends on.
pub struct SecretRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> SecretRepository<'a> {
    /// Borrow `pool` for the lifetime of this repository.
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new secret and return the stored row.
    ///
    /// A second secret of the same name in the same scope is the caller's
    /// mistake, not an internal failure, so the unique constraint maps to
    /// [`Error::Conflict`] and the 409 `SPEC.md`, "Secrets" promises. Because
    /// the index is `NULLS NOT DISTINCT`, that includes two `global` secrets
    /// of the same name, whose `scope_id` is NULL in both rows.
    pub async fn insert(&self, tx: &mut PgConnection, secret: &NewSecret) -> Result<Secret> {
        let inserted = sqlx::query_as!(
            Secret,
            r#"
            INSERT INTO secrets (id, scope, scope_id, name, ciphertext, nonce,
                                 data_key_wrapped, data_key_nonce, key_version,
                                 orchestrator_only, created_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING id, scope AS "scope: SecretScope", scope_id, name, ciphertext, nonce,
                      data_key_wrapped, data_key_nonce, key_version, orchestrator_only,
                      created_by, created_at, updated_at
            "#,
            secret.id,
            secret.scope.scope() as SecretScope,
            secret.scope.scope_id(),
            secret.name.as_str(),
            secret.value.ciphertext.as_slice(),
            secret.value.nonce.as_slice(),
            secret.value.data_key_wrapped.as_slice(),
            secret.value.data_key_nonce.as_slice(),
            secret.value.key_version,
            secret.orchestrator_only,
            secret.created_by,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(map_duplicate)?;

        // The name is the identifying half and is safe; everything else this
        // row carries is not (rule 3).
        debug!(
            secret_id = %inserted.id,
            name = %inserted.name,
            scope = %inserted.scope,
            "secret inserted"
        );

        Ok(inserted)
    }

    /// The secret with this id, or `None`.
    pub async fn find(&self, id: Uuid) -> Result<Option<Secret>> {
        let secret = sqlx::query_as!(
            Secret,
            r#"
            SELECT id, scope AS "scope: SecretScope", scope_id, name, ciphertext, nonce,
                   data_key_wrapped, data_key_nonce, key_version, orchestrator_only,
                   created_by, created_at, updated_at
            FROM secrets
            WHERE id = $1
            "#,
            id,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(secret)
    }

    /// The secret of this name in this scope, or `None`.
    ///
    /// The scope arrives as one validated [`ScopeRef`] rather than a loose
    /// `(scope, scope_id)` pair, so a lookup cannot ask for a combination the
    /// table's `CHECK` forbids and would never match.
    ///
    /// `IS NOT DISTINCT FROM` rather than `=` for `scope_id`: the global scope
    /// stores NULL there, and `NULL = NULL` is unknown, so a plain equality
    /// would never find a global secret. It is the same comparison the
    /// `NULLS NOT DISTINCT` unique index makes.
    ///
    /// One of the three lookups the launch resolver composes; it decides the
    /// precedence, not this method.
    pub async fn find_by_name(
        &self,
        scope: &ScopeRef,
        name: &SecretName,
    ) -> Result<Option<Secret>> {
        let secret = sqlx::query_as!(
            Secret,
            r#"
            SELECT id, scope AS "scope: SecretScope", scope_id, name, ciphertext, nonce,
                   data_key_wrapped, data_key_nonce, key_version, orchestrator_only,
                   created_by, created_at, updated_at
            FROM secrets
            WHERE scope = $1 AND scope_id IS NOT DISTINCT FROM $2 AND name = $3
            "#,
            scope.scope() as SecretScope,
            scope.scope_id(),
            name.as_str(),
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(secret)
    }

    /// Every secret in this scope as the API shape, by name (`GET /secrets`).
    ///
    /// The `LEFT JOIN` is what makes `last_used_at` an aggregate rather than a
    /// column: `MAX(secret_uses.at)` per secret, and NULL for a secret that
    /// has never been used, which the join keeps in the result instead of
    /// dropping it (`SPEC.md`, "Secrets"). A scope with no secrets at all is
    /// an empty vector, never a 404 — the scope exists whether or not anything
    /// is in it.
    pub async fn list_meta(&self, scope: &ScopeRef) -> Result<Vec<SecretMeta>> {
        let meta = sqlx::query_as!(
            SecretMeta,
            r#"
            SELECT s.id, s.scope AS "scope: SecretScope", s.scope_id, s.name,
                   s.orchestrator_only, s.key_version, s.created_by, s.created_at, s.updated_at,
                   MAX(u.at) AS "last_used_at?"
            FROM secrets s
            LEFT JOIN secret_uses u ON u.secret_id = s.id
            WHERE s.scope = $1 AND s.scope_id IS NOT DISTINCT FROM $2
            GROUP BY s.id
            ORDER BY s.name
            "#,
            scope.scope() as SecretScope,
            scope.scope_id(),
        )
        .fetch_all(self.pool)
        .await?;

        Ok(meta)
    }

    /// Replace the encrypted value, returning the stored row or `None` when no
    /// secret has this id (the route decides whether that is a 404).
    ///
    /// All five encrypted fields move together (`PUT /secrets/{id}`): a new
    /// value gets a new data key, so leaving `data_key_wrapped` behind would
    /// make the row undecryptable. The name and the scope are untouched, so
    /// the additional authenticated data is unchanged and the caller
    /// re-encrypted under the one it already had.
    pub async fn update_value(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        value: &EncryptedValue,
    ) -> Result<Option<Secret>> {
        let updated = sqlx::query_as!(
            Secret,
            r#"
            UPDATE secrets
            SET ciphertext = $2,
                nonce = $3,
                data_key_wrapped = $4,
                data_key_nonce = $5,
                key_version = $6,
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, scope AS "scope: SecretScope", scope_id, name, ciphertext, nonce,
                      data_key_wrapped, data_key_nonce, key_version, orchestrator_only,
                      created_by, created_at, updated_at
            "#,
            id,
            value.ciphertext.as_slice(),
            value.nonce.as_slice(),
            value.data_key_wrapped.as_slice(),
            value.data_key_nonce.as_slice(),
            value.key_version,
        )
        .fetch_optional(&mut *tx)
        .await?;

        debug!(secret_id = %id, updated = updated.is_some(), "secret value replaced");

        Ok(updated)
    }

    /// Rename the secret and write the re-encrypted value in one statement.
    ///
    /// One `UPDATE`, not two, because the name is part of the additional
    /// authenticated data: a row that had the new name and the old ciphertext,
    /// even briefly, could not be decrypted (`docs/data-model.md`, `secrets`).
    /// Producing `value` — decrypt under the old AAD, re-encrypt under the new
    /// one, inside the caller's transaction — is the caller's; this method
    /// writes exactly what it is given and does not care whether `name`
    /// differs from the stored one.
    ///
    /// A name already taken in the same scope maps to [`Error::Conflict`], the
    /// same way [`SecretRepository::insert`] does.
    pub async fn rename(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        name: &SecretName,
        value: &EncryptedValue,
    ) -> Result<Option<Secret>> {
        let renamed = sqlx::query_as!(
            Secret,
            r#"
            UPDATE secrets
            SET name = $2,
                ciphertext = $3,
                nonce = $4,
                data_key_wrapped = $5,
                data_key_nonce = $6,
                key_version = $7,
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, scope AS "scope: SecretScope", scope_id, name, ciphertext, nonce,
                      data_key_wrapped, data_key_nonce, key_version, orchestrator_only,
                      created_by, created_at, updated_at
            "#,
            id,
            name.as_str(),
            value.ciphertext.as_slice(),
            value.nonce.as_slice(),
            value.data_key_wrapped.as_slice(),
            value.data_key_nonce.as_slice(),
            value.key_version,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_duplicate)?;

        debug!(secret_id = %id, name = %name, renamed = renamed.is_some(), "secret renamed");

        Ok(renamed)
    }

    /// Set whether this secret is kept out of containers
    /// (`PATCH /secrets/{id}`), returning the stored row or `None`.
    ///
    /// Nothing is re-encrypted: the flag is not part of the additional
    /// authenticated data.
    pub async fn set_orchestrator_only(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        orchestrator_only: bool,
    ) -> Result<Option<Secret>> {
        let updated = sqlx::query_as!(
            Secret,
            r#"
            UPDATE secrets
            SET orchestrator_only = $2,
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, scope AS "scope: SecretScope", scope_id, name, ciphertext, nonce,
                      data_key_wrapped, data_key_nonce, key_version, orchestrator_only,
                      created_by, created_at, updated_at
            "#,
            id,
            orchestrator_only,
        )
        .fetch_optional(&mut *tx)
        .await?;

        debug!(
            secret_id = %id,
            orchestrator_only,
            updated = updated.is_some(),
            "secret orchestrator-only flag set"
        );

        Ok(updated)
    }

    /// Delete a secret, reporting whether a row matched.
    ///
    /// `Ok(false)` rather than an error when nothing matched: the route turns
    /// that into 404. The audit rows cascade with it — a `secret_uses` row
    /// naming a secret that no longer exists would say nothing useful, and
    /// `GET /secrets/{id}/uses` has no id to be asked about any more.
    pub async fn delete(&self, tx: &mut PgConnection, id: Uuid) -> Result<bool> {
        let result = sqlx::query!("DELETE FROM secrets WHERE id = $1", id)
            .execute(&mut *tx)
            .await?;

        let deleted = result.rows_affected() > 0;
        debug!(secret_id = %id, deleted, "secret deleted");

        Ok(deleted)
    }

    /// One rotation batch: up to `limit` rows still wrapped by a master key
    /// older than `below_version`.
    ///
    /// The sweep runs `below_version = <newest version>` and repeats until an
    /// empty batch comes back, unwrapping and re-wrapping each row through
    /// [`SecretRepository::rewrap`] (`ARCHITECTURE.md`, "Secrets", Rotation).
    /// Ordered by `key_version` then `id` so the oldest keys are retired first
    /// and a batch is reproducible; `secrets_key_version_idx` serves the
    /// predicate.
    ///
    /// Read outside any transaction: each row is re-wrapped in its own
    /// statement, and a row that another request changed in between simply
    /// comes back in a later batch.
    pub async fn list_for_rotation(&self, below_version: i32, limit: i64) -> Result<Vec<Secret>> {
        let batch = sqlx::query_as!(
            Secret,
            r#"
            SELECT id, scope AS "scope: SecretScope", scope_id, name, ciphertext, nonce,
                   data_key_wrapped, data_key_nonce, key_version, orchestrator_only,
                   created_by, created_at, updated_at
            FROM secrets
            WHERE key_version < $1
            ORDER BY key_version, id
            LIMIT $2
            "#,
            below_version,
            limit,
        )
        .fetch_all(self.pool)
        .await?;

        debug!(below_version, rows = batch.len(), "rotation batch selected");

        Ok(batch)
    }

    /// Re-wrap one row's data key under a newer master key.
    ///
    /// One statement, and `ciphertext` and `nonce` are deliberately absent
    /// from it: rotation changes which master key protects the data key, never
    /// the data key itself and never the encryption of the value
    /// (`ARCHITECTURE.md`, "Secrets", Rotation). Returns whether a row
    /// matched, so a sweep can notice a secret deleted between the batch and
    /// the write instead of failing.
    pub async fn rewrap(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        data_key_wrapped: &[u8],
        data_key_nonce: &[u8],
        key_version: i32,
    ) -> Result<bool> {
        let result = sqlx::query!(
            r#"
            UPDATE secrets
            SET data_key_wrapped = $2,
                data_key_nonce = $3,
                key_version = $4,
                updated_at = NOW()
            WHERE id = $1
            "#,
            id,
            data_key_wrapped,
            data_key_nonce,
            key_version,
        )
        .execute(&mut *tx)
        .await?;

        let rewrapped = result.rows_affected() > 0;
        debug!(secret_id = %id, key_version, rewrapped, "secret data key re-wrapped");

        Ok(rewrapped)
    }

    /// Every distinct `key_version` in the table, ascending.
    ///
    /// The startup check: the keyring verifies it can unwrap one row per
    /// version present and refuses to start otherwise, because a missing key
    /// version would otherwise only be discovered at a session launch
    /// (`ARCHITECTURE.md`, "Secrets", Keyring). An empty table returns an
    /// empty vector, which is a valid keyring — there is nothing to unwrap.
    pub async fn distinct_key_versions(&self) -> Result<Vec<i32>> {
        let versions = sqlx::query_scalar!(
            r#"SELECT DISTINCT key_version AS "key_version!" FROM secrets ORDER BY key_version"#
        )
        .fetch_all(self.pool)
        .await?;

        Ok(versions)
    }

    /// The ids of secrets whose scope target no longer exists.
    ///
    /// `scope_id` has no foreign key — the scope decides which table it points
    /// at, so there is nothing for one constraint to reference
    /// (`docs/data-model.md`, `secrets`) — which means deleting a user or a
    /// project leaves its secrets behind. This is what the reaper sweeps.
    /// `global` rows have no target and are never orphans.
    pub async fn list_orphans(&self) -> Result<Vec<Uuid>> {
        let orphans = sqlx::query_scalar!(
            r#"
            SELECT s.id AS "id!"
            FROM secrets s
            WHERE (s.scope = 'user'
                   AND NOT EXISTS (SELECT 1 FROM users u WHERE u.id = s.scope_id))
               OR (s.scope = 'project'
                   AND NOT EXISTS (SELECT 1 FROM projects p WHERE p.id = s.scope_id))
            ORDER BY s.id
            "#
        )
        .fetch_all(self.pool)
        .await?;

        debug!(rows = orphans.len(), "orphaned secrets selected");

        Ok(orphans)
    }

    /// Record that a secret was read, and why.
    ///
    /// Written per injected secret on every launch, including relaunches of
    /// parked sessions, and by the git credential provider
    /// (`docs/data-model.md`, `secret_uses`). Both ids are optional and both
    /// being `None` is normal: the mirror-fetch job has neither a session nor
    /// a user behind it.
    ///
    /// Takes the caller's connection so the audit row commits with whatever
    /// made it true — the launch, or the git operation — rather than
    /// separately from it.
    pub async fn insert_use(
        &self,
        tx: &mut PgConnection,
        secret_id: Uuid,
        session_id: Option<Uuid>,
        user_id: Option<Uuid>,
        purpose: SecretUsePurpose,
    ) -> Result<SecretUse> {
        let inserted = sqlx::query_as!(
            SecretUse,
            r#"
            INSERT INTO secret_uses (secret_id, session_id, user_id, purpose)
            VALUES ($1, $2, $3, $4)
            RETURNING id, secret_id, session_id, user_id,
                      purpose AS "purpose: SecretUsePurpose", at
            "#,
            secret_id,
            session_id,
            user_id,
            purpose.as_str(),
        )
        .fetch_one(&mut *tx)
        .await?;

        debug!(secret_id = %secret_id, purpose = %purpose, "secret use recorded");

        Ok(inserted)
    }

    /// The most recent uses of one secret, newest first
    /// (`GET /secrets/{id}/uses?limit=`).
    ///
    /// `secret_uses_secret_idx (secret_id, at DESC)` serves the order, so the
    /// limit is applied without sorting the whole history. `id` breaks ties so
    /// two uses recorded in the same transaction — and therefore sharing
    /// `NOW()` — still come back in a stable order.
    pub async fn list_uses(&self, secret_id: Uuid, limit: i64) -> Result<Vec<SecretUse>> {
        let uses = sqlx::query_as!(
            SecretUse,
            r#"
            SELECT id, secret_id, session_id, user_id,
                   purpose AS "purpose: SecretUsePurpose", at
            FROM secret_uses
            WHERE secret_id = $1
            ORDER BY at DESC, id DESC
            LIMIT $2
            "#,
            secret_id,
            limit,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(uses)
    }
}

/// Map the `secrets` unique constraint to the conflict `SPEC.md` promises.
///
/// The index is deliberately unnamed in the migration so that Postgres assigns
/// `secrets_scope_scope_id_name_key`, which is the name matched here. Anything
/// else — including a unique violation on a constraint this function does not
/// know — widens through `#[from] sqlx::Error`, which logs the detail once and
/// answers a generic 500 rather than guessing at a client message.
fn map_duplicate(err: sqlx::Error) -> Error {
    match unique_violation(&err) {
        Some("secrets_scope_scope_id_name_key") => Error::Conflict("secret already exists".into()),
        _ => Error::from(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rotation_batch_is_the_documented_hundred() {
        // `ARCHITECTURE.md`, "Secrets": "in batches of 100".
        assert_eq!(ROTATION_BATCH, 100);
    }

    #[test]
    fn a_duplicate_secret_is_a_conflict_only_for_the_known_constraint() {
        // Not a database error at all: the generic mapping answers 500.
        let error = map_duplicate(sqlx::Error::Protocol("unexpected packet".into()));
        assert!(matches!(error, Error::Database(_)), "{error:?}");
    }
}
