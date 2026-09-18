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
//! - *Resolution at launch.* Ordering `global`, `project`, `user` so that the
//!   last one found wins, and skipping a name entirely when the winner is
//!   `orchestrator_only`, is the resolver's (`ARCHITECTURE.md`, "Secrets").
//!   [`SecretRepository::find_for_resolution`] reads every candidate row for a
//!   launch in one query and [`SecretRepository::find_by_name`] reads one
//!   scope; neither ranks them, because ranking is the rule itself.
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
    EncryptedValue, KeyVersionSample, NewSecret, ScopeRef, Secret, SecretMeta, SecretName,
    SecretScope, SecretUse, SecretUsePurpose,
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

/// Which user-scoped secrets one listing is allowed to show.
///
/// "User-scoped secrets are listed, changed and deleted only by their owner or
/// an admin" (`SPEC.md`, "Secrets") is a decision about the caller, not about
/// the rows, so the service makes it and hands the answer down as one of these
/// two. [`UserFilter::Only`] with an empty vector is a caller who may see no
/// user-scoped secret at all; that is a legitimate filter rather than a
/// mistake, and `= ANY('{}')` matches nothing without a special case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserFilter {
    /// Every user's, which is what an administrator sees.
    All,
    /// Only the listed users'.
    Only(Vec<Uuid>),
}

/// The `WHERE` clause of `GET /secrets`, as three independent narrowings.
///
/// `scope` and `scope_id` are the query parameters `SPEC.md`, "Secrets" names,
/// each optional and each `None` meaning "do not narrow on this"; `user_ids`
/// is the visibility rule above. They compose, and nothing here validates the
/// pair the way [`ScopeRef`] does — a filter is a question, so asking for
/// `scope = global` together with a `scope_id` is an empty answer rather than
/// a rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretListFilter {
    /// Only this population, or every one of them.
    pub scope: Option<SecretScope>,
    /// Only this user or project, or every target.
    pub scope_id: Option<Uuid>,
    /// Whose user-scoped secrets the caller may see.
    pub user_ids: UserFilter,
}

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

    /// The secret with this id, locked for the rest of the caller's
    /// transaction.
    ///
    /// `SELECT ... FOR UPDATE` because replacing a value and renaming are both
    /// read-modify-write across a decrypt: the caller reads the row, opens it,
    /// re-encrypts it and writes it back, and two requests that both read the
    /// old row would each re-encrypt under an identity the other is about to
    /// change (`docs/data-model.md`, `secrets`). The lock is on this one row
    /// and nothing else — a secret is not part of a tracker or event
    /// transaction, so no project or session row is involved
    /// (`ARCHITECTURE.md`, "Task tracker").
    ///
    /// `None` is a secret that does not exist, and locks nothing; the route
    /// turns that into 404.
    pub async fn find_for_update(&self, tx: &mut PgConnection, id: Uuid) -> Result<Option<Secret>> {
        let secret = sqlx::query_as!(
            Secret,
            r#"
            SELECT id, scope AS "scope: SecretScope", scope_id, name, ciphertext, nonce,
                   data_key_wrapped, data_key_nonce, key_version, orchestrator_only,
                   created_by, created_at, updated_at
            FROM secrets
            WHERE id = $1
            FOR UPDATE
            "#,
            id,
        )
        .fetch_optional(&mut *tx)
        .await?;

        Ok(secret)
    }

    /// The secret with this id as the API shape, or `None`.
    ///
    /// The metadata half of [`SecretRepository::find`], for the routes that
    /// answer a `SecretMeta` and never need the value (`SPEC.md`, "Secrets").
    /// The four encrypted columns are not in the projection at all, so a
    /// handler with no business decrypting a row is not even handed the bytes
    /// to try.
    ///
    /// `last_used_at` is a correlated `MAX(secret_uses.at)` rather than a join
    /// and a `GROUP BY`, served by `secret_uses_secret_idx`: one row in, one
    /// row out, and no grouping to write.
    pub async fn find_meta(&self, id: Uuid) -> Result<Option<SecretMeta>> {
        let meta = sqlx::query_as!(
            SecretMeta,
            r#"
            SELECT s.id, s.scope AS "scope: SecretScope", s.scope_id, s.name,
                   s.orchestrator_only, s.key_version, s.created_by, s.created_at, s.updated_at,
                   (SELECT MAX(u.at) FROM secret_uses u WHERE u.secret_id = s.id)
                       AS "last_used_at?"
            FROM secrets s
            WHERE s.id = $1
            "#,
            id,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(meta)
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

    /// Whether a secret of this name exists in this scope.
    ///
    /// The `EXISTS` form of [`SecretRepository::find_by_name`], for the
    /// callers that want the answer and not the row: the git credential
    /// provider asking whether a project or a user has a `GIT_CREDENTIAL` at
    /// all before it offers to authenticate, and the service checking a name
    /// before it seals a value it would otherwise have to throw away. Moving
    /// four encrypted columns to decide a boolean would be a row read for
    /// nothing.
    pub async fn exists_by_name(&self, scope: &ScopeRef, name: &SecretName) -> Result<bool> {
        let exists = sqlx::query_scalar!(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM secrets
                WHERE scope = $1 AND scope_id IS NOT DISTINCT FROM $2 AND name = $3
            ) AS "exists!"
            "#,
            scope.scope() as SecretScope,
            scope.scope_id(),
            name.as_str(),
        )
        .fetch_one(self.pool)
        .await?;

        Ok(exists)
    }

    /// Every row any of `names` could resolve to for this project and this
    /// user, in one query.
    ///
    /// Resolution walks `global`, `project(session.project_id)`,
    /// `user(session.created_by)` and the last row found wins
    /// (`ARCHITECTURE.md`, "Secrets", Resolution at launch). Composing that
    /// from [`SecretRepository::find_by_name`] is three round trips per name on
    /// the launch path, so this reads the whole candidate set at once and the
    /// resolver groups it by name in memory.
    ///
    /// Precedence stays the resolver's, and so does skipping: an
    /// `orchestrator_only` winner means the name is not injected *at all* and
    /// a lower-precedence row must not leak through, which is a decision about
    /// the group rather than about a row. This method therefore filters
    /// nothing and orders only so that a batch is reproducible.
    ///
    /// `$3::uuid IS NOT NULL` is spelled out rather than left to
    /// `scope_id = $3`: a session whose creator has been deleted has no user
    /// scope at all, and saying so is better than resting that on `NULL = NULL`
    /// being unknown.
    pub async fn find_for_resolution(
        &self,
        names: &[String],
        project_id: Uuid,
        user_id: Option<Uuid>,
    ) -> Result<Vec<Secret>> {
        let candidates = sqlx::query_as!(
            Secret,
            r#"
            SELECT id, scope AS "scope: SecretScope", scope_id, name, ciphertext, nonce,
                   data_key_wrapped, data_key_nonce, key_version, orchestrator_only,
                   created_by, created_at, updated_at
            FROM secrets
            WHERE name = ANY($1)
              AND (scope = 'global'
                   OR (scope = 'project' AND scope_id = $2)
                   OR (scope = 'user' AND $3::uuid IS NOT NULL AND scope_id = $3))
            ORDER BY name, scope
            "#,
            names,
            project_id,
            user_id,
        )
        .fetch_all(self.pool)
        .await?;

        debug!(
            project_id = %project_id,
            requested = names.len(),
            rows = candidates.len(),
            "resolution candidates selected"
        );

        Ok(candidates)
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

    /// The secrets one listing request may see, by scope and then by name.
    ///
    /// The three clauses of [`SecretListFilter`], each of which narrows only
    /// when it is set: `GET /secrets?scope=&scope_id=` are the caller's
    /// question and `user_ids` is the visibility the service resolved from who
    /// is asking (`SPEC.md`, "Secrets"). The rule stays in the `WHERE` clause
    /// rather than in a filter over the result, so a row the caller may not see
    /// is never read (`CLAUDE.md`, "Backend conventions").
    ///
    /// `s.scope <> 'user' OR $3 OR s.scope_id = ANY($4)` is the whole of it:
    /// `global` and `project` secrets are visible to every user, and a `user`
    /// one only to an administrator or to the listed owners.
    ///
    /// `ORDER BY s.scope` is the enum's declaration order — `global`, `user`,
    /// `project` (`docs/data-model.md`, "Enums") — not alphabetical; what it
    /// guarantees is that a listing is grouped by population and stable, not
    /// which population comes first.
    pub async fn list_meta_filtered(&self, filter: &SecretListFilter) -> Result<Vec<SecretMeta>> {
        // `All` binds an empty array it never compares against, because the
        // `$3` disjunct has already answered the clause.
        let all_users = matches!(filter.user_ids, UserFilter::All);
        let visible: &[Uuid] = match &filter.user_ids {
            UserFilter::All => &[],
            UserFilter::Only(ids) => ids,
        };

        let meta = sqlx::query_as!(
            SecretMeta,
            r#"
            SELECT s.id, s.scope AS "scope: SecretScope", s.scope_id, s.name,
                   s.orchestrator_only, s.key_version, s.created_by, s.created_at, s.updated_at,
                   (SELECT MAX(u.at) FROM secret_uses u WHERE u.secret_id = s.id)
                       AS "last_used_at?"
            FROM secrets s
            WHERE ($1::secret_scope IS NULL OR s.scope = $1)
              AND ($2::uuid IS NULL OR s.scope_id = $2)
              AND (s.scope <> 'user' OR $3::bool OR s.scope_id = ANY($4))
            ORDER BY s.scope, s.name
            "#,
            filter.scope as Option<SecretScope>,
            filter.scope_id,
            all_users,
            visible,
        )
        .fetch_all(self.pool)
        .await?;

        debug!(rows = meta.len(), "secrets listed");

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

    /// Delete every secret of one project, reporting how many rows went.
    ///
    /// `secrets.scope_id` carries no foreign key — the scope decides which
    /// table it points at (`docs/data-model.md`, `secrets`) — so deleting a
    /// project takes nothing with it, and its secrets would become exactly the
    /// orphans [`SecretRepository::list_orphans`] exists to find. Project
    /// deletion therefore removes them itself, in the same transaction as the
    /// row ([`crate::projects::delete_project`]). The `secret_uses` audit rows
    /// cascade with each secret, as they do for a single
    /// [`SecretRepository::delete`].
    ///
    /// The scope is built from [`ScopeRef`] rather than written as a literal,
    /// so the pair in the `WHERE` clause is the one the model considers valid;
    /// `IS NOT DISTINCT FROM` is the comparison the rest of this file uses for
    /// a `scope_id` that may be NULL, although a project's never is.
    pub async fn delete_all_for_project(
        &self,
        tx: &mut PgConnection,
        project_id: Uuid,
    ) -> Result<u64> {
        let scope = ScopeRef::project(project_id);

        let result = sqlx::query!(
            "DELETE FROM secrets WHERE scope = $1 AND scope_id IS NOT DISTINCT FROM $2",
            scope.scope() as SecretScope,
            scope.scope_id(),
        )
        .execute(&mut *tx)
        .await?;

        let deleted = result.rows_affected();
        debug!(project_id = %project_id, deleted, "project secrets deleted");

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
    ///
    /// `expected_key_version` is the version the batch read, and it is in the
    /// `WHERE` clause because the sweep runs outside any transaction: between
    /// the select and this statement a `PUT /secrets/{id}` can have replaced
    /// the value, which writes a *new* data key under the newest master key.
    /// Writing the re-wrap of the old data key over that would make the row
    /// undecryptable. Guarding on the version the row still had means the
    /// stale write matches nothing; `Ok(false)` is the sweep's "skipped", not
    /// an error, and the row is already current anyway.
    pub async fn rewrap(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        expected_key_version: i32,
        data_key_wrapped: &[u8],
        data_key_nonce: &[u8],
        key_version: i32,
    ) -> Result<bool> {
        let result = sqlx::query!(
            r#"
            UPDATE secrets
            SET data_key_wrapped = $3,
                data_key_nonce = $4,
                key_version = $5,
                updated_at = NOW()
            WHERE id = $1 AND key_version = $2
            "#,
            id,
            expected_key_version,
            data_key_wrapped,
            data_key_nonce,
            key_version,
        )
        .execute(&mut *tx)
        .await?;

        let rewrapped = result.rows_affected() > 0;
        debug!(
            secret_id = %id,
            expected_key_version,
            key_version,
            rewrapped,
            "secret data key re-wrapped"
        );

        Ok(rewrapped)
    }

    /// How many rows are still wrapped by a master key older than `newest`.
    ///
    /// What a rotation sweep reports before it starts and what tells an
    /// operator whether an old master key can be dropped from the environment:
    /// "Once no row references the old version, it can be removed"
    /// (`ARCHITECTURE.md`, "Secrets", Rotation). A separate statement from
    /// [`SecretRepository::list_for_rotation`] because a sweep asks it once,
    /// not once per batch, and `secrets_key_version_idx` answers it without
    /// reading a row.
    pub async fn count_below_version(&self, newest: i32) -> Result<i64> {
        let remaining = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "count!" FROM secrets WHERE key_version < $1"#,
            newest,
        )
        .fetch_one(self.pool)
        .await?;

        Ok(remaining)
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

    /// One row per distinct `key_version`, with its wrapping.
    ///
    /// What the startup check actually reads: knowing which versions are
    /// present is not enough to know the configured master keys can still
    /// open them, so this hands the keyring a sample it can try to unwrap
    /// (`ARCHITECTURE.md`, "Secrets", Keyring). `DISTINCT ON` with a matching
    /// `ORDER BY` picks the lowest id per version, which makes the sample the
    /// same row on every start and the query one index scan rather than a
    /// full table read. An empty table returns an empty vector and the check
    /// passes trivially.
    ///
    /// The bytes are a wrapped data key, never a value, and neither they nor
    /// the nonce are logged anywhere on this path (rule 3).
    pub async fn distinct_key_version_samples(&self) -> Result<Vec<KeyVersionSample>> {
        let samples = sqlx::query_as!(
            KeyVersionSample,
            r#"
            SELECT DISTINCT ON (key_version)
                   key_version AS "key_version!",
                   data_key_wrapped AS "data_key_wrapped!",
                   data_key_nonce AS "data_key_nonce!"
            FROM secrets
            ORDER BY key_version, id
            "#
        )
        .fetch_all(self.pool)
        .await?;

        Ok(samples)
    }

    /// Whether the user or project a scope points at still exists.
    ///
    /// `scope_id` has no foreign key — the scope decides which table it points
    /// at — so nothing in the schema stops a secret being created for a user
    /// deleted a moment ago; "the repository validates existence"
    /// (`docs/data-model.md`, `secrets`) is this. The service calls it before
    /// an insert and answers 404 for a scope that is not there.
    ///
    /// `global` has no target, so it is true without a query. The answer is a
    /// snapshot either way: the row could be deleted immediately afterwards,
    /// which is what [`SecretRepository::list_orphans`] and the reaper are for.
    pub async fn scope_exists(&self, scope: &ScopeRef) -> Result<bool> {
        let exists = match scope.scope() {
            SecretScope::Global => true,
            SecretScope::User => {
                sqlx::query_scalar!(
                    r#"SELECT EXISTS(SELECT 1 FROM users WHERE id = $1) AS "exists!""#,
                    scope.scope_id(),
                )
                .fetch_one(self.pool)
                .await?
            }
            SecretScope::Project => {
                sqlx::query_scalar!(
                    r#"SELECT EXISTS(SELECT 1 FROM projects WHERE id = $1) AS "exists!""#,
                    scope.scope_id(),
                )
                .fetch_one(self.pool)
                .await?
            }
        };

        Ok(exists)
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
    ///
    /// `limit` is clamped to at least one row. A zero or negative limit is the
    /// caller's mistake — the route validates `?limit=` before it gets here —
    /// and `LIMIT 0` would answer an empty list, which reads as a secret that
    /// has never been used rather than as a bad request.
    pub async fn list_uses(&self, secret_id: Uuid, limit: i64) -> Result<Vec<SecretUse>> {
        let limit = limit.max(1);

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
