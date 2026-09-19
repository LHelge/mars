//! The domain operations behind `/api/secrets` (`SPEC.md`, "Secrets
//! (`/api/secrets`)").
//!
//! Everything the six endpoints do except reading the request and writing the
//! response: who may touch which row, whether a scope target exists, whether a
//! name and a value are acceptable, and — the one rule that is neither a
//! repository statement nor a model invariant — that renaming a secret
//! re-encrypts its value, because the name is part of the additional
//! authenticated data (`docs/data-model.md`, `secrets`). The route module is a
//! thin adapter over this one.
//!
//! **Ownership.** "User-scoped secrets are listed, changed and deleted only by
//! their owner or an admin (403 otherwise); global and project secrets by any
//! user" (`SPEC.md`, "Secrets") is [`authorize`], and it is the same check for
//! every operation. v1 has no project membership, so a `project` secret is open
//! to every authenticated user; when membership arrives, it arrives in that one
//! function.
//!
//! **Transactions.** `replace_value`, `patch` and `delete` each open one
//! transaction and read the row with `SELECT ... FOR UPDATE` before they decide
//! anything. The lock is not about the read: a rename is decrypt, re-encrypt,
//! write, and two of them interleaving — or one interleaving with a rotation's
//! `rewrap` — would write a ciphertext bound to an identity the row no longer
//! has, which nothing could decrypt afterwards. `create` holds a transaction
//! for the scope check and the insert.
//!
//! **Values.** A plaintext value exists in this module as a
//! [`Zeroizing<String>`] from the moment the route deserialises it until it is
//! sealed, and never longer: it is not logged, not copied into an error
//! message, not returned and not held in a struct that derives `Debug`
//! (`ARCHITECTURE.md`, "Secrets", Credential handling; `CLAUDE.md`, rule 3).
//! The tracing fields on this path are `secret_id`, `secret_name`, `scope` and
//! `scope_id`, all of which are identity rather than content. No response ever
//! carries a value, which is why every operation answers a [`SecretMeta`].

use uuid::Uuid;
use zeroize::Zeroizing;

use super::envelope::{SealedSecret, SecretIdentity};
use super::keyring::{SecretsError, SecretsKeyring};
use crate::models::{
    NewSecret, ScopeRef, Secret, SecretMeta, SecretName, SecretScope, SecretUse,
    validate_secret_value,
};
use crate::prelude::*;
use crate::repositories::{SecretListFilter, SecretRepository, UserFilter};

/// How many uses `GET /secrets/{id}/uses` returns when `?limit=` is absent
/// (`SPEC.md`, "Secrets").
pub const DEFAULT_USES_LIMIT: u32 = 50;

/// The largest `?limit=` the same endpoint honours; anything above is silently
/// reduced to it.
///
/// A cap rather than a rejection: a client asking for more history than exists
/// is not making a mistake, it just cannot have the whole table in one
/// response.
pub const MAX_USES_LIMIT: u32 = 500;

/// The 400 for a scope pointing at a user or project that does not exist.
///
/// Deliberately 400 rather than 404: the request's *target* is fine — it is
/// `POST /secrets` — and the unknown id is a field of the body, which is the
/// same thing as a malformed one from the client's point of view.
const UNKNOWN_SCOPE_ID: &str = "unknown scope_id";

/// The 403 for touching another user's secret without being an administrator.
const NOT_THE_OWNER: &str = "user-scoped secrets belong to their owner";

/// The 400 for `?limit=0`.
const LIMIT_TOO_SMALL: &str = "limit must be at least 1";

/// The message a failed decrypt of a *stored* row answers.
///
/// A 500, not a 400: the caller cannot influence a row's `key_version` or its
/// ciphertext, so a row that will not open is a fault in the deployment — a
/// master key dropped from the environment, which the startup check
/// ([`SecretsKeyring::verify_against_db`]) exists to catch first
/// (`ARCHITECTURE.md`, "Secrets", Keyring). The detail is logged; the client
/// gets the generic internal message.
const UNREADABLE_ROW: &str = "a stored secret could not be re-encrypted";

/// Who is asking.
///
/// The two facts every operation needs about the caller and nothing else: the
/// route builds one from `CurrentUser` and this module never looks a user up.
/// `admin` is the snapshot the request resolved, which is the row rather than
/// the token's claim (ADR 0025).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actor {
    /// The caller's `users.id`, and the default owner of a `user`-scoped
    /// secret they create.
    pub user_id: Uuid,
    /// Whether the caller may reach other users' secrets.
    pub admin: bool,
}

impl Actor {
    /// The caller as a non-administrator, for the routes that have a plain
    /// `CurrentUser`.
    pub fn new(user_id: Uuid, admin: bool) -> Self {
        Self { user_id, admin }
    }
}

/// The body of `POST /secrets`, validated by [`SecretsService::create`].
///
/// No `Debug`, deliberately: it holds a plaintext credential, and a derived
/// one would put it in every `?`-formatted tracing field and every panic
/// message that ever carried this struct (rule 3). `value` is
/// [`Zeroizing<String>`] so the buffer is wiped when the request ends,
/// whichever way it ends.
pub struct CreateSecret {
    /// Which population the secret belongs to.
    pub scope: SecretScope,
    /// The user or project it belongs to. `None` for `global`, and for `user`
    /// it defaults to the caller.
    pub scope_id: Option<Uuid>,
    /// The environment-variable style name, validated here.
    pub name: String,
    /// The plaintext value, sealed and dropped before this call returns.
    pub value: Zeroizing<String>,
    /// Whether the secret is kept out of containers.
    pub orchestrator_only: bool,
}

/// The body of `PATCH /secrets/{id}`: either field, both or neither.
///
/// Neither is legal and answers the current metadata unchanged — a client
/// re-sending a form it did not edit has not made a mistake — so nothing is
/// written and nothing is re-encrypted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchSecret {
    /// The new name, which re-encrypts the value under the new additional
    /// authenticated data when it differs from the current one.
    pub name: Option<String>,
    /// The new value of the flag.
    pub orchestrator_only: Option<bool>,
}

/// The secrets manager's domain operations over one pool and one keyring.
///
/// Borrowed rather than owned, and built per request: the route constructs one
/// from `AppState` (`&state.pool`, `&state.keyring`) and drops it with the
/// response. It holds no state of its own, so two concurrent requests share
/// nothing but the pool.
pub struct SecretsService<'a> {
    pool: &'a PgPool,
    keyring: &'a SecretsKeyring,
}

impl<'a> SecretsService<'a> {
    /// Borrow the pool and the keyring for the lifetime of this service.
    pub fn new(pool: &'a PgPool, keyring: &'a SecretsKeyring) -> Self {
        Self { pool, keyring }
    }

    /// `POST /secrets` → the stored metadata (201; 409 if the name is taken).
    ///
    /// The order is the one that avoids doing work for a request that will be
    /// refused, and avoids refusing one for the wrong reason:
    ///
    /// 1. default `scope_id` for a `user` secret to the caller, which is the
    ///    only scope that has a default and the only defaulting this module
    ///    does,
    /// 2. [`authorize`], so creating a secret for somebody else is 403 rather
    ///    than a validation error,
    /// 3. pair the scope with its target through [`ScopeRef::new`], which is
    ///    where "a `global` secret has no `scope_id` and a `user` or `project`
    ///    one needs one" lives — once, for this module, the repository and the
    ///    table's `CHECK` (`docs/data-model.md`, `secrets`),
    /// 4. validate the name and the value, which is 400 before anything is
    ///    encrypted,
    /// 5. check the scope target exists, because `scope_id` has no foreign key
    ///    (`docs/data-model.md`, `secrets`),
    /// 6. seal under the row's own identity and insert, in one transaction.
    ///
    /// `created_by` is the caller. The value is sealed under a fresh data key
    /// and the plaintext is dropped here.
    #[instrument(skip_all, fields(scope = %request.scope, secret_name = %request.name))]
    pub async fn create(&self, actor: &Actor, request: CreateSecret) -> Result<SecretMeta> {
        // A user creating their own is the common case and needs no id; every
        // other scope means exactly what the caller sent, and `ScopeRef::new`
        // decides whether that is a pair at all.
        let scope_id = match request.scope {
            SecretScope::User => Some(request.scope_id.unwrap_or(actor.user_id)),
            _ => request.scope_id,
        };

        authorize(actor, request.scope, scope_id)?;

        let scope = ScopeRef::new(request.scope, scope_id)?;
        let name = SecretName::parse(&request.name)?;
        validate_secret_value(&request.value)?;

        let repository = SecretRepository::new(self.pool);
        if !repository.scope_exists(&scope).await? {
            return Err(Error::BadRequest(UNKNOWN_SCOPE_ID.into()));
        }

        let sealed = SealedSecret::seal(
            self.keyring,
            SecretIdentity::new(&scope, &name),
            request.value.as_bytes(),
        )?;

        let mut tx = self.pool.begin().await?;
        let inserted = repository
            .insert(
                &mut tx,
                &NewSecret {
                    id: Uuid::new_v4(),
                    sealed,
                    orchestrator_only: request.orchestrator_only,
                    created_by: Some(actor.user_id),
                },
            )
            .await?;
        tx.commit().await?;

        info!(secret_id = %inserted.id, "secret created");

        // A row that was inserted a statement ago has no uses, which is
        // exactly what `From<&Secret>` says, so this needs no second read.
        Ok(SecretMeta::from(&inserted))
    }

    /// `PUT /secrets/{id}` → the stored metadata, with a new value.
    ///
    /// The scope and the name do not move, so the identity is the one the row
    /// already had; what is new is the value, and with it a fresh data key and
    /// nonce under the newest master key
    /// ([`SealedSecret::seal`]). Nothing is decrypted: replacing a value never needs
    /// to read the old one, so a row whose master key is missing can still be
    /// overwritten.
    #[instrument(skip_all, fields(secret_id = %id))]
    pub async fn replace_value(
        &self,
        actor: &Actor,
        id: Uuid,
        value: Zeroizing<String>,
    ) -> Result<SecretMeta> {
        let repository = SecretRepository::new(self.pool);
        let mut tx = self.pool.begin().await?;

        let row = repository
            .find_for_update(&mut tx, id)
            .await?
            .ok_or(Error::NotFound)?;
        authorize(actor, row.scope, row.scope_id)?;
        validate_secret_value(&value)?;

        let sealed = SealedSecret::seal(self.keyring, SecretIdentity::of(&row), value.as_bytes())?;

        let updated = repository
            .update_value(&mut tx, id, &sealed)
            .await?
            .ok_or(Error::NotFound)?;
        tx.commit().await?;

        info!(secret_id = %id, "secret value replaced");

        Ok(updated)
    }

    /// `PATCH /secrets/{id}` → the stored metadata, renamed, re-flagged or
    /// both.
    ///
    /// Renaming is the one operation that decrypts: the name is part of the
    /// additional authenticated data, so the value is opened under the old
    /// identity and re-sealed under the new one, keeping its data key, and the
    /// name and the re-encrypted columns are written in one statement inside
    /// one transaction (`docs/data-model.md`, `secrets`). A rename to the name
    /// the row already has is not a rename and re-encrypts nothing; a patch
    /// that changes neither field writes nothing at all and answers the current
    /// metadata.
    ///
    /// The scope never moves — changing a secret's scope is a delete and a
    /// create, because the value would have to be re-encrypted under an
    /// identity that may already be taken — so the only part of the AAD that
    /// this can change is the name.
    #[instrument(skip_all, fields(secret_id = %id))]
    pub async fn patch(&self, actor: &Actor, id: Uuid, request: PatchSecret) -> Result<SecretMeta> {
        let repository = SecretRepository::new(self.pool);
        let mut tx = self.pool.begin().await?;

        let row = repository
            .find_for_update(&mut tx, id)
            .await?
            .ok_or(Error::NotFound)?;
        authorize(actor, row.scope, row.scope_id)?;

        // Validated even when it matches, so `PATCH {"name": "not a name"}` is
        // a 400 rather than a silent no-op.
        let rename = match &request.name {
            Some(raw) => {
                let parsed = SecretName::parse(raw)?;
                (parsed.as_str() != row.name).then_some(parsed)
            }
            None => None,
        };
        let reflag = request
            .orchestrator_only
            .filter(|flag| *flag != row.orchestrator_only);

        if rename.is_none() && reflag.is_none() {
            // Nothing to write, so nothing to commit; the read lock goes with
            // the rolled-back transaction. The metadata is read rather than
            // returned by a statement, because no statement ran.
            drop(tx);
            return repository.find_meta(id).await?.ok_or(Error::NotFound);
        }

        // Each write answers the row it wrote, so the last one to run is the
        // stored state and nothing has to be read back afterwards.
        let mut written = None;

        if let Some(name) = &rename {
            let resealed = row
                .sealed()
                .reseal(self.keyring, SecretIdentity::of(&row).renamed(name))
                .map_err(|err| unreadable(&row, err))?;

            written = Some(
                repository
                    .rename(&mut tx, id, &resealed)
                    .await?
                    .ok_or(Error::NotFound)?,
            );
        }

        if let Some(flag) = reflag {
            written = Some(
                repository
                    .set_orchestrator_only(&mut tx, id, flag)
                    .await?
                    .ok_or(Error::NotFound)?,
            );
        }

        tx.commit().await?;

        info!(
            secret_id = %id,
            renamed = rename.is_some(),
            reflagged = reflag.is_some(),
            "secret patched"
        );

        // `None` is unreachable: the early return above covered the patch that
        // writes nothing, so one of the two statements ran and answered a row.
        // Answering 404 rather than panicking keeps the `unwrap` out of a
        // request path (`CLAUDE.md`, "Backend conventions").
        written.ok_or(Error::NotFound)
    }

    /// `DELETE /secrets/{id}` → 204.
    ///
    /// The row is locked and authorized before it goes, so a caller who may
    /// not see it gets 403 rather than a deletion. The `secret_uses` rows
    /// cascade with it (`docs/data-model.md`, `secret_uses`).
    #[instrument(skip_all, fields(secret_id = %id))]
    pub async fn delete(&self, actor: &Actor, id: Uuid) -> Result<()> {
        let repository = SecretRepository::new(self.pool);
        let mut tx = self.pool.begin().await?;

        let row = repository
            .find_for_update(&mut tx, id)
            .await?
            .ok_or(Error::NotFound)?;
        authorize(actor, row.scope, row.scope_id)?;

        if !repository.delete(&mut tx, id).await? {
            return Err(Error::NotFound);
        }
        tx.commit().await?;

        info!(secret_id = %id, "secret deleted");

        Ok(())
    }

    /// `GET /secrets?scope=&scope_id=` → the metadata the caller may see.
    ///
    /// Both parameters are optional and they mean different things per scope
    /// (`SPEC.md`, "Secrets"):
    ///
    /// - `scope=global` takes no `scope_id` (400 if given).
    /// - `scope=project` needs one (400 if absent); every authenticated user
    ///   may list any project's secrets in v1.
    /// - `scope=user` without a `scope_id` lists the caller's own, for an
    ///   administrator too; with another user's it needs `admin` (403
    ///   otherwise).
    /// - No `scope` at all lists everything the caller may see: the global
    ///   secrets, every project's, and their own user-scoped ones —
    ///   every user's for an administrator.
    ///
    /// The pair is resolved into one [`ScopeRef`] before it reaches the
    /// repository, so the impossible combinations are refused by
    /// [`ScopeRef::new`] — the same rule, in the same place, as a create — and
    /// the filter cannot ask a question the table's `CHECK` forbids. A
    /// `scope_id` sent without a `scope` narrows nothing: a target means
    /// nothing without the population it is in.
    ///
    /// The visibility rule reaches the database as [`UserFilter`] rather than
    /// as a filter over the result, so a row the caller may not see is never
    /// read (`CLAUDE.md`, "Backend conventions").
    #[instrument(skip_all, fields(scope = ?scope))]
    pub async fn list(
        &self,
        actor: &Actor,
        scope: Option<SecretScope>,
        scope_id: Option<Uuid>,
    ) -> Result<Vec<SecretMeta>> {
        let scope = match scope {
            // "user scope returns the caller's; admins may select another user
            // with `scope_id`" — so an absent id means the caller, whoever
            // they are, rather than everybody.
            Some(SecretScope::User) => {
                let owner = scope_id.unwrap_or(actor.user_id);
                authorize(actor, SecretScope::User, Some(owner))?;
                Some(ScopeRef::user(owner))
            }
            Some(other) => Some(ScopeRef::new(other, scope_id)?),
            None => None,
        };

        // An administrator sees every user's; everybody else sees their own,
        // which is what makes an unscoped listing safe to serve unfiltered.
        let user_ids = if actor.admin {
            UserFilter::All
        } else {
            UserFilter::Only(vec![actor.user_id])
        };

        SecretRepository::new(self.pool)
            .list_meta_filtered(&SecretListFilter { scope, user_ids })
            .await
    }

    /// `GET /secrets/{id}/uses?limit=` → the most recent uses, newest first.
    ///
    /// The same ownership rule as a change: the audit of a user's secret says
    /// which of their sessions read it, which is theirs and an administrator's
    /// to see. `limit` defaults to [`DEFAULT_USES_LIMIT`] and is reduced to
    /// [`MAX_USES_LIMIT`]; an explicit `0` is the caller's mistake and a 400,
    /// because answering an empty list would read as a secret that has never
    /// been used.
    #[instrument(skip_all, fields(secret_id = %id))]
    pub async fn uses(
        &self,
        actor: &Actor,
        id: Uuid,
        limit: Option<u32>,
    ) -> Result<Vec<SecretUse>> {
        let limit = match limit {
            Some(0) => return Err(Error::BadRequest(LIMIT_TOO_SMALL.into())),
            Some(requested) => requested.min(MAX_USES_LIMIT),
            None => DEFAULT_USES_LIMIT,
        };

        let repository = SecretRepository::new(self.pool);
        let meta = repository.find_meta(id).await?.ok_or(Error::NotFound)?;
        authorize(actor, meta.scope, meta.scope_id)?;

        repository.list_uses(id, limit).await
    }
}

/// The ownership rule, once, for every operation.
///
/// "User-scoped secrets are listed, changed and deleted only by their owner or
/// an admin (403 otherwise); global and project secrets by any user"
/// (`SPEC.md`, "Secrets"). A `project` secret is open to every authenticated
/// user because v1 has no project membership to check against
/// (`ARCHITECTURE.md`, "Secrets"); when there is one, this is where it goes.
///
/// A `user` row whose `scope_id` is `None` cannot exist — the table's `CHECK`
/// forbids it — and is refused rather than treated as unowned.
pub fn authorize(actor: &Actor, scope: SecretScope, scope_id: Option<Uuid>) -> Result<()> {
    if scope == SecretScope::User && scope_id != Some(actor.user_id) && !actor.admin {
        return Err(Error::Forbidden(NOT_THE_OWNER.into()));
    }

    Ok(())
}

/// Log a stored row that will not open and answer a generic 500.
///
/// The one place a rename can fail for a reason that is nobody's request: the
/// master key that wrapped this row's data key is not in the environment any
/// more, which the startup check should have refused to boot with
/// (`ARCHITECTURE.md`, "Secrets", Keyring). `key_version` is what an operator
/// needs to put it back, so it is the field on the log line; the error's own
/// message names no key material either.
fn unreadable(row: &Secret, err: SecretsError) -> Error {
    error!(
        secret_id = %row.id,
        key_version = row.key_version,
        error = %err,
        "a stored secret could not be decrypted"
    );

    Error::Internal(UNREADABLE_ROW.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> Actor {
        Actor::new(Uuid::from_u128(1), false)
    }

    fn admin() -> Actor {
        Actor::new(Uuid::from_u128(2), true)
    }

    #[test]
    fn global_and_project_secrets_are_open_to_every_user() {
        assert!(authorize(&actor(), SecretScope::Global, None).is_ok());
        assert!(authorize(&actor(), SecretScope::Project, Some(Uuid::from_u128(9))).is_ok());
    }

    #[test]
    fn a_user_secret_is_the_owners_or_an_administrators() {
        let owner = actor();

        assert!(authorize(&owner, SecretScope::User, Some(owner.user_id)).is_ok());
        assert!(authorize(&admin(), SecretScope::User, Some(owner.user_id)).is_ok());

        let refused = authorize(&owner, SecretScope::User, Some(Uuid::from_u128(3)));
        assert!(matches!(refused, Err(Error::Forbidden(_))), "{refused:?}");

        // Impossible in the table, and refused rather than read as unowned.
        let refused = authorize(&owner, SecretScope::User, None);
        assert!(matches!(refused, Err(Error::Forbidden(_))), "{refused:?}");
    }

    #[test]
    fn the_uses_limits_are_the_documented_ones() {
        // `SPEC.md`, "Secrets": 50 by default, 500 at most.
        assert_eq!(DEFAULT_USES_LIMIT, 50);
        assert_eq!(MAX_USES_LIMIT, 500);
    }
}
