//! The fixed-name `GIT_CREDENTIAL` project secret.
//!
//! The remote credential of a project is not a column: it is a
//! project-scoped, orchestrator-only secret named `GIT_CREDENTIAL`, and the
//! git credential provider looks it up by that fixed name
//! (`docs/data-model.md`, `projects`; ADR 0002). This module is that lookup,
//! and its two write-side companions — storing the value a project was created
//! with, and answering `has_credential` for the `Project` DTO (`SPEC.md`,
//! "Projects (`/api/projects`)").
//!
//! It exists so that the name, the envelope crypto and the audit row live in
//! one place rather than three: `git/` builds an `Authorization: Basic` header
//! out of whatever it is handed (`ARCHITECTURE.md`, "Git model",
//! Credentials) and `routes/projects.rs` moves a request field, and neither
//! has to know what the secret is called or how it is encrypted.
//!
//! **Every read is audited.** Decryption happens only at session launch and
//! inside the git credential provider (ADR 0006), and the provider's half is
//! here: each successful read writes one `secret_uses` row with
//! `purpose = 'git'` and whichever of `session_id` (an MCP tool) or `user_id`
//! (a REST request) asked, in the same transaction as the read, so a
//! credential cannot be read without leaving the trace
//! (`docs/data-model.md`, `secret_uses`). A read that fails to decrypt writes
//! no row: the transaction is dropped before the insert is reached.
//!
//! Nothing here logs, formats or returns the value. The plaintext lives in a
//! `Zeroizing` buffer from the moment it leaves the cipher, and the only
//! fields any line in this module carries are `project_id`, `purpose` and a
//! `key_version` (`CLAUDE.md`, rule 3).

use sqlx::PgConnection;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::crypto::{aad, aad_for, open, seal};
use super::keyring::SecretsKeyring;
use crate::models::{
    NewSecret, ScopeRef, Secret, SecretName, SecretUsePurpose, validate_secret_value,
};
use crate::prelude::*;
use crate::repositories::SecretRepository;

/// The name every project's remote credential is stored under
/// (`docs/data-model.md`, `projects`).
pub const GIT_CREDENTIAL_NAME: &str = "GIT_CREDENTIAL";

/// Who asked for the credential, as `secret_uses` records it.
///
/// The three variants are exactly the three documented `(session_id, user_id)`
/// combinations: an MCP tool acting for a session, a REST request acting for a
/// user, and the orchestrator's own background work — the mirror-fetch job —
/// which has neither (`docs/data-model.md`, `secret_uses`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GitUseContext {
    /// An MCP tool, on behalf of this session.
    Session(Uuid),
    /// A REST request, on behalf of this user.
    User(Uuid),
    /// The orchestrator itself; neither id is set.
    System,
}

impl GitUseContext {
    /// The `(session_id, user_id)` pair this context writes.
    fn ids(self) -> (Option<Uuid>, Option<Uuid>) {
        match self {
            GitUseContext::Session(session_id) => (Some(session_id), None),
            GitUseContext::User(user_id) => (None, Some(user_id)),
            GitUseContext::System => (None, None),
        }
    }
}

/// The validated form of [`GIT_CREDENTIAL_NAME`].
///
/// The constant is checked against the documented pattern by a unit test
/// below, so the parse cannot fail; the `expect` states that rather than
/// pushing a `Result` no caller could act on through three signatures (the
/// same reason `Secret::scope_ref` has one).
fn credential_name() -> SecretName {
    SecretName::parse(GIT_CREDENTIAL_NAME).expect("GIT_CREDENTIAL matches the secret name pattern")
}

/// This project's remote credential, decrypted, or `None` when it has none.
///
/// The value is the raw stored one — a PAT in v1. Turning it into
/// `Authorization: Basic base64("x-access-token:" + PAT)` is the git
/// wrapper's, which is the only thing that should ever see it in a header
/// (`ARCHITECTURE.md`, "Git model", Credentials; ADR 0002).
///
/// A project with no `GIT_CREDENTIAL` row answers `None` and writes nothing:
/// a public remote is not a use. Every other outcome — the value, or a failure
/// — has already written or rolled back its audit row by the time it returns.
///
/// A row that cannot be decrypted is an operator's problem, not a caller's: it
/// means the master key for its version is gone or the row is corrupt, so it
/// answers [`Error::Internal`] and logs the project and the version the row
/// asks for, which is what names the missing key.
#[instrument(skip_all, fields(project_id = %project_id, purpose = "git"))]
pub async fn project_git_credential(
    pool: &PgPool,
    keyring: &SecretsKeyring,
    project_id: Uuid,
    ctx: GitUseContext,
) -> Result<Option<Zeroizing<String>>> {
    let repository = SecretRepository::new(pool);
    let scope = ScopeRef::project(project_id);
    let name = credential_name();

    // Cheap outside the transaction: it decides whether there is anything to
    // audit at all, and the row is read again under the lock below.
    let Some(found) = repository.find_by_name(&scope, &name).await? else {
        debug!("the project has no git credential");
        return Ok(None);
    };

    let mut tx = pool.begin().await?;

    // The read the use row belongs to. A row deleted between the two reads is
    // the same answer as one that was never there, rather than a foreign key
    // violation on the audit insert.
    let Some(row) = repository.find_for_update(&mut tx, found.id).await? else {
        debug!("the project's git credential was removed during the lookup");
        return Ok(None);
    };

    let value = decrypt(keyring, project_id, &row)?;

    let (session_id, user_id) = ctx.ids();
    repository
        .insert_use(&mut tx, row.id, session_id, user_id, SecretUsePurpose::Git)
        .await?;

    tx.commit().await?;

    Ok(Some(value))
}

/// Store this project's remote credential, replacing any value it already has.
///
/// What `POST /api/projects` does with `credential`: the value is stored as the
/// project-scoped orchestrator-only secret `GIT_CREDENTIAL` and is never
/// returned (`SPEC.md`, "Projects (`/api/projects`)"). A new row is
/// `orchestrator_only`, so the launch resolver never injects it into a
/// container; a row that already exists keeps whatever flag it carries,
/// because a user who deliberately cleared it through the secrets API is
/// making a choice this path has no business reversing.
///
/// A replacement draws a fresh data key and a fresh nonce rather than
/// re-encrypting under the old one, so the new value shares nothing with the
/// value it replaced.
#[instrument(skip_all, fields(project_id = %project_id))]
pub async fn set_project_git_credential(
    pool: &PgPool,
    keyring: &SecretsKeyring,
    project_id: Uuid,
    value: Zeroizing<String>,
    created_by: Option<Uuid>,
) -> Result<()> {
    // The same check, and so the same 400 text, as a value arriving through
    // `POST /api/secrets`: a credential is a secret value and there is one
    // rule for what one may be (`CLAUDE.md`, "Backend conventions").
    validate_secret_value(&value)?;

    let scope = ScopeRef::project(project_id);
    let name = credential_name();

    if replace_value(pool, keyring, &scope, &name, &value).await? {
        return Ok(());
    }

    let new = seal_credential(keyring, project_id, &value, created_by)?;

    let mut tx = pool.begin().await?;
    match SecretRepository::new(pool).insert(&mut tx, &new).await {
        Ok(_) => {
            tx.commit().await?;
            info!("the project's git credential was stored");
            Ok(())
        }
        // A user's `POST /api/secrets` of the same name landed between the
        // lookup and the insert. The name is the contract, so the intent is
        // still "this project's credential is now that value": retry once as
        // the replacement it would have been a moment earlier.
        Err(Error::Conflict(_)) => {
            drop(tx);
            if replace_value(pool, keyring, &scope, &name, &value).await? {
                Ok(())
            } else {
                // Created and deleted again while this call was in flight.
                // Nothing to replace and nothing to insert into; the caller
                // asked for a state the database no longer has a row for.
                Err(Error::Conflict(
                    "the project's git credential changed during the write".into(),
                ))
            }
        }
        Err(error) => Err(error),
    }
}

/// Store a brand-new project's remote credential inside the caller's
/// transaction.
///
/// The creation half of [`set_project_git_credential`]:
/// [`crate::projects::create_project`] writes the project row, its task
/// states, its default profile and this secret in one `BEGIN … COMMIT`, so a
/// keyring failure here rolls the project back and a duplicate project name
/// leaves no orphaned credential behind (`SPEC.md`, "Projects";
/// `docs/data-model.md`, `secrets`).
///
/// Insert-only, unlike its pool-taking sibling, and that is the whole
/// difference: the project is being created in this very transaction, so it
/// cannot already have a `GIT_CREDENTIAL` row to replace. A conflict is
/// therefore a genuine surprise and is the caller's to answer, which for the
/// creation path means rolling everything back.
///
/// `pool` is only what the repository borrows; every statement runs on `tx`.
#[instrument(skip_all, fields(project_id = %project_id))]
pub async fn insert_project_git_credential(
    pool: &PgPool,
    tx: &mut PgConnection,
    keyring: &SecretsKeyring,
    project_id: Uuid,
    value: &Zeroizing<String>,
    created_by: Option<Uuid>,
) -> Result<()> {
    // The same check, and so the same 400 text, as a value arriving through
    // `POST /api/secrets` (`CLAUDE.md`, "Backend conventions").
    validate_secret_value(value)?;

    let new = seal_credential(keyring, project_id, value, created_by)?;
    SecretRepository::new(pool).insert(&mut *tx, &new).await?;

    // `debug!` where the pool-taking path says `info!`: the row is written but
    // not committed, and the caller's transaction may still roll it back, so
    // an "it was stored" line here could outlive the row it describes. The
    // creation that commits it logs once (`crate::projects::create_project`).
    debug!("the project's git credential was inserted");

    Ok(())
}

/// The `secrets` row a project's credential is stored as.
///
/// Both write paths come through here, so the fixed name, the fresh data key
/// bound to that name and scope, and the `orchestrator_only` flag that keeps
/// the launch resolver from ever injecting it are decided in one place rather
/// than twice (`docs/data-model.md`, `secrets`; ADR 0006). The plaintext is
/// borrowed, sealed and left behind: nothing here copies, formats or returns
/// it (rule 3).
fn seal_credential(
    keyring: &SecretsKeyring,
    project_id: Uuid,
    value: &Zeroizing<String>,
    created_by: Option<Uuid>,
) -> Result<NewSecret> {
    let scope = ScopeRef::project(project_id);
    let name = credential_name();

    let sealed = seal(keyring, &aad_for(&scope, &name), value.as_bytes())?;
    let mut new = NewSecret::new(scope, name, sealed);
    new.orchestrator_only = true;
    new.created_by = created_by;

    Ok(new)
}

/// Whether this project has a remote credential at all.
///
/// A boolean from an `EXISTS`, so no ciphertext is moved and nothing is
/// decrypted to answer it.
///
/// Rendering a project does not come through here: `Project.has_credential`
/// is computed by the same `EXISTS` inside every statement of
/// [`crate::repositories::ProjectRepository`], which keeps a listing to one
/// round trip (`SPEC.md`, "Projects (`/api/projects`)"). This is the standalone
/// question — "does this project have one at all?" — for callers that hold no
/// project row, such as the credential provider deciding whether to prompt.
pub async fn has_project_git_credential(pool: &PgPool, project_id: Uuid) -> Result<bool> {
    SecretRepository::new(pool)
        .exists_by_name(&ScopeRef::project(project_id), &credential_name())
        .await
}

/// Open one `GIT_CREDENTIAL` row, under its own row identity.
///
/// The AAD is built from the stored columns rather than from the scope the
/// caller asked with, so the value is bound to the row it actually came out of
/// (ADR 0006).
fn decrypt(keyring: &SecretsKeyring, project_id: Uuid, row: &Secret) -> Result<Zeroizing<String>> {
    let aad = aad(row.scope, row.scope_id, &row.name);
    let value = row.encrypted_value();

    let plaintext = open(keyring, &aad, &value).map_err(|_| {
        error!(
            project_id = %project_id,
            key_version = row.key_version,
            "the project's git credential could not be decrypted"
        );
        Error::Internal("the git credential could not be read".into())
    })?;

    // A credential is text: it becomes an `Authorization` header. Bytes that
    // are not are a corrupt row, and read like any other unreadable one.
    let text = std::str::from_utf8(&plaintext).map_err(|_| {
        error!(
            project_id = %project_id,
            key_version = row.key_version,
            "the project's git credential is not valid UTF-8"
        );
        Error::Internal("the git credential could not be read".into())
    })?;

    Ok(Zeroizing::new(text.to_string()))
}

/// Replace the value of an existing `GIT_CREDENTIAL` row, if there is one.
///
/// `false` means there was no row to replace, which is the caller's signal to
/// insert one. The lookup and the write share a transaction and the row is
/// locked between them, so a concurrent rename or rotation cannot land between
/// reading the row's identity and writing a value bound to it.
async fn replace_value(
    pool: &PgPool,
    keyring: &SecretsKeyring,
    scope: &ScopeRef,
    name: &SecretName,
    value: &Zeroizing<String>,
) -> Result<bool> {
    let repository = SecretRepository::new(pool);

    let Some(found) = repository.find_by_name(scope, name).await? else {
        return Ok(false);
    };

    let mut tx = pool.begin().await?;
    let Some(row) = repository.find_for_update(&mut tx, found.id).await? else {
        return Ok(false);
    };

    let sealed = seal(
        keyring,
        &aad(row.scope, row.scope_id, &row.name),
        value.as_bytes(),
    )?;
    repository.update_value(&mut tx, row.id, &sealed).await?;
    tx.commit().await?;

    info!("the project's git credential was replaced");

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixed_name_is_a_valid_secret_name() {
        assert_eq!(credential_name().as_str(), GIT_CREDENTIAL_NAME);
    }

    #[test]
    fn a_context_maps_to_its_documented_id_pair() {
        let id = Uuid::new_v4();

        assert_eq!(GitUseContext::Session(id).ids(), (Some(id), None));
        assert_eq!(GitUseContext::User(id).ids(), (None, Some(id)));
        assert_eq!(GitUseContext::System.ids(), (None, None));
    }
}
