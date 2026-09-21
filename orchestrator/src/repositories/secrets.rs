//! All SQL against `secrets` and `secret_uses`.
//!
//! `docs/data-model.md`, "Secrets" is the column contract and
//! `ARCHITECTURE.md`, "Secrets" is the design. This file moves the encrypted
//! columns and nothing else: it never wraps, unwraps, encrypts or decrypts,
//! never touches the keyring and never sees a value in the clear. It is handed
//! an already-built [`SealedSecret`] and reads the bytes back out again.
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
//!   the re-sealed envelope and the name it is bound to in one statement;
//!   deciding what those bytes are, and doing it in one transaction with the
//!   decrypt, is the caller's.
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
    NewSecret, ScopeRef, Secret, SecretMeta, SecretName, SecretScope, SecretUse, SecretUsePurpose,
};
use crate::prelude::*;
use crate::repositories::unique_violation;
use crate::secrets::SealedSecret;
use crate::secrets::service::credential_conflict;

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

/// The `WHERE` clause of `GET /secrets`, as two independent narrowings.
///
/// `scope` is the `?scope=&scope_id=` pair of `SPEC.md`, "Secrets" as the one
/// validated [`ScopeRef`] the service resolved them into, and `None` is the
/// unscoped listing; `user_ids` is the visibility rule above. A pair the
/// table's `CHECK` would reject cannot be asked for here at all, because
/// [`ScopeRef::new`] is the only way to build one and it is the only place
/// that rule lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretListFilter {
    /// Only this scope, or every one of them.
    pub scope: Option<ScopeRef>,
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
    /// The scope and the name are bound from the envelope's identity rather
    /// than from fields beside it, so the row's naming columns are by
    /// construction the ones its ciphertext was sealed under
    /// (`docs/data-model.md`, `secrets`).
    ///
    /// A second secret of the same name in the same scope is the caller's
    /// mistake, not an internal failure, so the unique constraint maps to
    /// [`Error::Conflict`] and the 409 `SPEC.md`, "Secrets" promises. Because
    /// the index is `NULLS NOT DISTINCT`, that includes two `global` secrets
    /// of the same name, whose `scope_id` is NULL in both rows.
    pub async fn insert(&self, tx: &mut PgConnection, secret: &NewSecret) -> Result<Secret> {
        let sealed = &secret.sealed;
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
            sealed.identity.scope() as SecretScope,
            sealed.identity.scope_id(),
            sealed.identity.name(),
            sealed.ciphertext.as_slice(),
            sealed.nonce.as_slice(),
            sealed.wrapped.wrapped.as_slice(),
            sealed.wrapped.nonce.as_slice(),
            sealed.wrapped.version,
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

    /// The name of the agent credential this scope already holds, out of
    /// `names`, or `None`.
    ///
    /// The one-credential-per-scope rule of `SPEC.md`, "Secrets", Agent
    /// credentials, as the question the service asks before it writes: `names`
    /// is one backend's `credential_names` and the scope is the pair the row
    /// would be created or renamed into, so a match is the row the caller must
    /// replace or delete first. `exclude` is the row being patched, which is
    /// not its own conflict.
    ///
    /// Takes the caller's connection, because the answer is only worth
    /// anything inside the transaction that then writes; the partial unique
    /// index `secrets_claude_credential_idx` is what holds when two
    /// transactions ask at once (`docs/data-model.md`, `secrets`).
    ///
    /// `IS NOT DISTINCT FROM` for `scope_id` for the reason
    /// [`SecretRepository::find_by_name`] gives: the global scope stores NULL
    /// there, and it is the comparison the index makes.
    pub async fn find_credential_at_scope(
        &self,
        tx: &mut PgConnection,
        scope: &ScopeRef,
        names: &[String],
        exclude: Option<Uuid>,
    ) -> Result<Option<String>> {
        let existing = sqlx::query_scalar!(
            r#"
            SELECT name
            FROM secrets
            WHERE scope = $1
              AND scope_id IS NOT DISTINCT FROM $2
              AND name = ANY($3)
              AND ($4::uuid IS NULL OR id <> $4)
            ORDER BY name
            LIMIT 1
            "#,
            scope.scope() as SecretScope,
            scope.scope_id(),
            names,
            exclude,
        )
        .fetch_optional(&mut *tx)
        .await?;

        Ok(existing)
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

    /// The secrets one listing request may see, by scope and then by name.
    ///
    /// The two clauses of [`SecretListFilter`], each of which narrows only
    /// when it is set: `GET /secrets?scope=&scope_id=` are the caller's
    /// question, resolved into one [`ScopeRef`], and `user_ids` is the
    /// visibility the service resolved from who is asking (`SPEC.md`,
    /// "Secrets"). The rule stays in the `WHERE` clause
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
            filter.scope.map(|scope| scope.scope()) as Option<SecretScope>,
            filter.scope.and_then(|scope| scope.scope_id()),
            all_users,
            visible,
        )
        .fetch_all(self.pool)
        .await?;

        debug!(rows = meta.len(), "secrets listed");

        Ok(meta)
    }

    /// Replace the encrypted value, returning the stored metadata or `None`
    /// when no secret has this id (the route decides whether that is a 404).
    ///
    /// All five encrypted fields move together (`PUT /secrets/{id}`): a new
    /// value gets a new data key, so leaving `data_key_wrapped` behind would
    /// make the row undecryptable. The name and the scope are untouched, so
    /// the additional authenticated data is unchanged and the caller
    /// re-encrypted under the one it already had.
    ///
    /// The answer is a [`SecretMeta`] rather than the row, projected by the
    /// same `RETURNING` clause that wrote it: the endpoints answer metadata,
    /// and a write that returns what it stored saves the second read that
    /// asking for it afterwards would need (`SPEC.md`, "Secrets"). The
    /// encrypted columns are not in the projection at all, so a caller that
    /// has just replaced a value is not handed the bytes back.
    pub async fn update_value(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        sealed: &SealedSecret,
    ) -> Result<Option<SecretMeta>> {
        let updated = sqlx::query_as!(
            SecretMeta,
            r#"
            UPDATE secrets s
            SET ciphertext = $2,
                nonce = $3,
                data_key_wrapped = $4,
                data_key_nonce = $5,
                key_version = $6,
                updated_at = NOW()
            WHERE s.id = $1
            RETURNING s.id, s.scope AS "scope: SecretScope", s.scope_id, s.name,
                      s.orchestrator_only, s.key_version, s.created_by, s.created_at, s.updated_at,
                      (SELECT MAX(u.at) FROM secret_uses u WHERE u.secret_id = s.id)
                          AS "last_used_at?"
            "#,
            id,
            sealed.ciphertext.as_slice(),
            sealed.nonce.as_slice(),
            sealed.wrapped.wrapped.as_slice(),
            sealed.wrapped.nonce.as_slice(),
            sealed.wrapped.version,
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
    /// Re-sealing the value under the new identity, inside the caller's
    /// transaction, is the caller's
    /// ([`SealedSecret::reseal`](crate::secrets::SealedSecret::reseal)); this
    /// method writes exactly what it is given and does not care whether the
    /// name differs from the stored one.
    ///
    /// The new name comes from the envelope's own identity rather than beside
    /// it, so there is no way to write a name the ciphertext was not sealed
    /// under — which is the one mistake this statement exists to make
    /// impossible.
    ///
    /// A name already taken in the same scope maps to [`Error::Conflict`], the
    /// same way [`SecretRepository::insert`] does.
    ///
    /// The answer is the stored [`SecretMeta`], for the reason
    /// [`SecretRepository::update_value`] gives.
    pub async fn rename(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        sealed: &SealedSecret,
    ) -> Result<Option<SecretMeta>> {
        let renamed = sqlx::query_as!(
            SecretMeta,
            r#"
            UPDATE secrets s
            SET name = $2,
                ciphertext = $3,
                nonce = $4,
                data_key_wrapped = $5,
                data_key_nonce = $6,
                key_version = $7,
                updated_at = NOW()
            WHERE s.id = $1
            RETURNING s.id, s.scope AS "scope: SecretScope", s.scope_id, s.name,
                      s.orchestrator_only, s.key_version, s.created_by, s.created_at, s.updated_at,
                      (SELECT MAX(u.at) FROM secret_uses u WHERE u.secret_id = s.id)
                          AS "last_used_at?"
            "#,
            id,
            sealed.identity.name(),
            sealed.ciphertext.as_slice(),
            sealed.nonce.as_slice(),
            sealed.wrapped.wrapped.as_slice(),
            sealed.wrapped.nonce.as_slice(),
            sealed.wrapped.version,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_duplicate)?;

        debug!(
            secret_id = %id,
            name = %sealed.identity.name(),
            renamed = renamed.is_some(),
            "secret renamed"
        );

        Ok(renamed)
    }

    /// Set whether this secret is kept out of containers
    /// (`PATCH /secrets/{id}`), returning the stored metadata or `None`.
    ///
    /// Nothing is re-encrypted: the flag is not part of the additional
    /// authenticated data. The answer is a [`SecretMeta`], for the reason
    /// [`SecretRepository::update_value`] gives.
    pub async fn set_orchestrator_only(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        orchestrator_only: bool,
    ) -> Result<Option<SecretMeta>> {
        let updated = sqlx::query_as!(
            SecretMeta,
            r#"
            UPDATE secrets s
            SET orchestrator_only = $2,
                updated_at = NOW()
            WHERE s.id = $1
            RETURNING s.id, s.scope AS "scope: SecretScope", s.scope_id, s.name,
                      s.orchestrator_only, s.key_version, s.created_by, s.created_at, s.updated_at,
                      (SELECT MAX(u.at) FROM secret_uses u WHERE u.secret_id = s.id)
                          AS "last_used_at?"
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
    /// orphans the reaper exists to find (`ARCHITECTURE.md`, "Background
    /// jobs"). Project
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

    /// Delete every scoped secret whose scope row is gone, reporting how many
    /// rows went.
    ///
    /// The token-cleanup job's sweep (`ARCHITECTURE.md`, "Background jobs":
    /// "...and secrets whose scope row no longer exists"). `secrets.scope_id`
    /// carries no foreign key, because the scope decides which table it points
    /// at (`docs/data-model.md`, `secrets`), so nothing in the schema removes
    /// these: deleting a user or a project removes its secrets in its own
    /// transaction, and what is left here is the residue of a deletion that
    /// was interrupted between the two.
    ///
    /// `global` rows have no scope row to lose and are never touched — their
    /// `scope_id` is NULL, so the `NOT EXISTS` halves would match every one of
    /// them if the `scope =` guards were dropped. Each secret takes its
    /// `secret_uses` rows with it through the table's `ON DELETE CASCADE`,
    /// which is what is wanted: the audit belongs to a scope that no longer
    /// exists.
    ///
    /// No `now`, and no lock: the statement's own predicate is the whole
    /// condition, and a row it matches has no owner left to race with.
    pub async fn delete_orphans(&self) -> Result<u64> {
        let result = sqlx::query!(
            r#"
            DELETE FROM secrets
            WHERE (scope = 'user'
                   AND NOT EXISTS (SELECT 1 FROM users u WHERE u.id = secrets.scope_id))
               OR (scope = 'project'
                   AND NOT EXISTS (SELECT 1 FROM projects p WHERE p.id = secrets.scope_id))
            "#,
        )
        .execute(self.pool)
        .await?;

        let deleted = result.rows_affected();
        debug!(deleted, "orphaned secrets deleted");

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
    /// from it although `sealed` carries them: rotation changes which master
    /// key protects the data key, never the data key itself and never the
    /// encryption of the value
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
        sealed: &SealedSecret,
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
            sealed.wrapped.wrapped.as_slice(),
            sealed.wrapped.nonce.as_slice(),
            sealed.wrapped.version,
        )
        .execute(&mut *tx)
        .await?;

        let rewrapped = result.rows_affected() > 0;
        debug!(
            secret_id = %id,
            expected_key_version,
            key_version = sealed.wrapped.version,
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

    /// One sealed row per distinct `key_version` in the table, ascending.
    ///
    /// What the startup check reads: knowing which versions are present is not
    /// enough to know the configured master keys can still open them, so this
    /// hands the check a sample per version whose data key it can try to
    /// unwrap ([`verify_keyring_at_startup`](crate::secrets::verify_keyring_at_startup);
    /// `ARCHITECTURE.md`, "Secrets", Keyring). `DISTINCT ON` with a matching
    /// `ORDER BY` picks the lowest id per version, which makes the sample the
    /// same row on every start and the query one index scan rather than a full
    /// table read. An empty table returns an empty vector and the check passes
    /// trivially.
    ///
    /// A [`SealedSecret`] rather than the wrapping columns alone: the sealed
    /// envelope is the one shape a stored row is read into, and it is the type
    /// that owns unwrapping. The value's ciphertext comes along and is never
    /// decrypted here; nothing on this path is logged (rule 3).
    pub async fn sample_per_key_version(&self) -> Result<Vec<SealedSecret>> {
        let samples = sqlx::query_as!(
            Secret,
            r#"
            SELECT DISTINCT ON (key_version)
                   id, scope AS "scope: SecretScope", scope_id, name, ciphertext, nonce,
                   data_key_wrapped, data_key_nonce, key_version, orchestrator_only,
                   created_by, created_at, updated_at
            FROM secrets
            ORDER BY key_version, id
            "#
        )
        .fetch_all(self.pool)
        .await?;

        Ok(samples.iter().map(Secret::sealed).collect())
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
    /// which is what the orphan reaper is for (`ARCHITECTURE.md`,
    /// "Background jobs").
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
    /// `limit` arrives validated and is bound as it is: the range — at least
    /// one, [`DEFAULT_USES_LIMIT`](crate::secrets::service::DEFAULT_USES_LIMIT)
    /// when absent and never more than
    /// [`MAX_USES_LIMIT`](crate::secrets::service::MAX_USES_LIMIT) — is the
    /// service's one rule (`SPEC.md`, "Secrets"), so this statement neither
    /// clamps nor re-checks it. A `u32` because the only limit that has no
    /// meaning here is a negative one, which the type forbids.
    pub async fn list_uses(&self, secret_id: Uuid, limit: u32) -> Result<Vec<SecretUse>> {
        let limit = i64::from(limit);

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

/// Map the `secrets` unique constraints to the conflicts `SPEC.md` promises.
///
/// The name index is deliberately unnamed in the migration so that Postgres
/// assigns `secrets_scope_scope_id_name_key`, which is the name matched here.
/// `secrets_claude_credential_idx` is the partial unique index behind the
/// one-agent-credential-per-scope rule (`docs/data-model.md`, `secrets`): the
/// service checks that rule first and answers the friendly message naming the
/// credential already there, so reaching this arm means a concurrent write won
/// the race between the check and the insert. The message is the same 409
/// without the name, because reading back which name won would be a second
/// statement on a path that has just failed — and never a 500.
///
/// Anything else — including a unique violation on a constraint this function
/// does not know — widens through `#[from] sqlx::Error`, which logs the detail
/// once and answers a generic 500 rather than guessing at a client message.
fn map_duplicate(err: sqlx::Error) -> Error {
    match unique_violation(&err) {
        Some("secrets_scope_scope_id_name_key") => Error::Conflict("secret already exists".into()),
        Some("secrets_claude_credential_idx") => credential_conflict(None),
        _ => Error::from(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duplicate_secret_is_a_conflict_only_for_the_known_constraint() {
        // Not a database error at all: the generic mapping answers 500.
        let error = map_duplicate(sqlx::Error::Protocol("unexpected packet".into()));
        assert!(matches!(error, Error::Database(_)), "{error:?}");
    }

    #[test]
    fn the_credential_index_has_a_conflict_message_of_its_own() {
        // The nameless half of the message the service builds; the two are
        // the same sentence so a client sees one rule, not two.
        let error = credential_conflict(None);
        assert_eq!(error.status(), axum::http::StatusCode::CONFLICT);
        assert!(
            error
                .to_string()
                .contains("already has an agent credential"),
            "{error}"
        );
    }
}
