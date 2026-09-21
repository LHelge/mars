//! Resolving a profile's `secrets` list into the environment of one session
//! container (`ARCHITECTURE.md`, "Secrets", Resolution at launch; "Launch
//! sequence").
//!
//! The launcher hands over two lists of names — the ones the profile declares
//! and the agent credentials the backend declares — and the three things that
//! decide which rows they can see: the session, its project and the user who
//! created it. It gets back the environment to put in the container, the
//! `launch_warning` messages to append, the names that were deliberately not
//! injected and which credential won. Everything the resolver decides is
//! decided here: the repository reads every candidate row in one query and
//! filters nothing, because precedence and suppression are decisions about a
//! *group* of rows rather than about any one of them.
//!
//! **Two lists, two rules.** A profile name is its own slot: the rows carrying
//! that exact name compete and the most specific scope wins. The credential
//! names are *one* slot between them: the rows carrying any of them compete
//! and the most specific scope wins whatever name it carries, so exactly one
//! credential is injected and a launcher no longer has to refuse a session
//! that resolved two (ADR 0036; `ARCHITECTURE.md`, "Secrets", Agent
//! credentials). Which backend those names belong to is not known here and
//! deliberately so: they arrive as a list like any other, and the warning that
//! names the backend is the launcher's to write.
//!
//! **The same selection answers the preflight.** `GET
//! /projects/{pid}/agent-credentials` says which credential a launch by the
//! caller would use before anything is launched, and it does so by calling the
//! very function a launch calls — [`select_credential`], from
//! [`preview_credential`] — over the same rows read at the same three scopes,
//! decrypting nothing and writing no audit row. A second implementation of the
//! rule is the one way the two could ever disagree (`SPEC.md`, "Secrets";
//! ADR 0036).
//!
//! **Precedence, then the flag.** `global`, then `project`, then `user`, last
//! one found wins. The winner's `orchestrator_only` is only consulted once the
//! winner is known, which is what makes a suppressed name genuinely absent: a
//! lower-precedence row that happens to be injectable is not a fallback, it is
//! a row the user's or project's own value already overrode. Reading the flag
//! first and then continuing down the scopes would leak exactly the value the
//! flag exists to keep out of the container.
//!
//! **Warnings are not failures.** A name with no row anywhere is a profile
//! that outlived a secret, which the operator should see but which must not
//! stop a launch; it comes back as text for the launcher to append as a
//! `launch_warning` event (`SPEC.md`, "AgentEvent"). This function never takes
//! the session row lock and never writes an event, so the audit transaction
//! below and the event transaction the launcher runs stay independent and
//! there is no lock order to observe (ADR 0021).
//!
//! **The audit trail is all or nothing.** One `secret_uses` row per injected
//! secret, all in one transaction, committed only once every value has been
//! decrypted (`docs/data-model.md`, `secret_uses`). A value that will not open
//! fails the launch, and the rows that would have claimed the launch read the
//! others are rolled back with it.
//!
//! Values live in `Zeroizing` buffers from the moment they leave the cipher
//! and only names reach a log line or a span
//! (`ARCHITECTURE.md`, "Secrets", Credential handling and transcripts; rule 3).
//! They stay in those buffers all the way through the launch seam:
//! [`ResolvedSecrets::env`] is moved into
//! [`SessionSpecInput::secrets`](crate::engine::spec::SessionSpecInput::secrets)
//! and then into
//! [`ContainerSpec::secret_env`](crate::engine::ContainerSpec::secret_env)
//! unchanged, and the only copy into plain bytes is the `NAME=value` line
//! [`to_bollard`](crate::engine::spec::to_bollard) builds for the engine call
//! (`ARCHITECTURE.md`, "Secrets", Resolution at launch).

use std::collections::HashMap;

use uuid::Uuid;
use zeroize::Zeroizing;

use super::keyring::SecretsKeyring;
use crate::models::{Secret, SecretName, SecretScope, SecretUsePurpose};
use crate::prelude::*;
use crate::repositories::SecretRepository;

/// Which session the secrets are being resolved for.
///
/// The three fields are the three scopes of one launch: `project_id` selects
/// the `project` rows, `created_by` the `user` rows — `None` for a session
/// whose creator has been deleted, which then has no user scope at all — and
/// `session_id` is what the audit rows point at
/// (`docs/data-model.md`, `sessions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchScope {
    pub session_id: Uuid,
    pub project_id: Uuid,
    pub created_by: Option<Uuid>,
}

/// What one launch resolved to.
///
/// `env` is the environment to give the container, in the order the profile
/// listed the names and with each name at most once; the agent credential, if
/// one was resolved, is last, because it is the launcher's name and not the
/// profile's. `warnings` are the `launch_warning` messages the launcher
/// appends before starting the container, and `skipped` names the secrets that
/// resolved to an `orchestrator_only` row and are therefore absent from `env`
/// on purpose — the launcher has both so it can tell "never existed" from
/// "withheld".
///
/// `scopes` is the winning scope of each injected name, in the same order and
/// with the same names as `env`. It is a parallel list rather than a third
/// element of the `env` tuples so that `env` can still be moved into
/// [`SessionSpecInput::secrets`](crate::engine::spec::SessionSpecInput::secrets)
/// unchanged.
///
/// `credential` is the one row of the credential slot that was injected, by
/// name and scope. The launcher hands it to the translator as its
/// `InjectedCredential`, which names the credential *and* the scope it came
/// from so a failed authentication can say where to fix it
/// (`ARCHITECTURE.md`, "Claude Code invocation", Credentials); it is stated
/// here rather than re-derived from `env`, where the launcher would have to
/// know the names again. `None` means no row carried any of them, or that the
/// row that won was `orchestrator_only` and skipped.
///
/// `Debug` redacts the values: the struct exists to carry credentials, so a
/// `?` field on it anywhere would be the one way they reach a log line
/// (rule 3).
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedSecrets {
    pub env: Vec<(String, Zeroizing<String>)>,
    pub scopes: Vec<(String, SecretScope)>,
    pub warnings: Vec<String>,
    pub skipped: Vec<String>,
    pub credential: Option<ResolvedCredential>,
}

/// The agent credential one launch injected: a name and the scope it was
/// stored at, never a value (`ARCHITECTURE.md`, "Secrets", Agent credentials).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCredential {
    pub name: SecretName,
    pub scope: SecretScope,
}

impl ResolvedSecrets {
    /// The scope the row that supplied `name` was stored at, or `None` when
    /// this resolution did not inject that name.
    pub fn scope_of(&self, name: &str) -> Option<SecretScope> {
        self.scopes
            .iter()
            .find(|(injected, _)| injected == name)
            .map(|(_, scope)| *scope)
    }
}

impl std::fmt::Debug for ResolvedSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Names are safe and are the whole point of printing this; values are
        // not.
        let names: Vec<&str> = self.env.iter().map(|(name, _)| name.as_str()).collect();

        f.debug_struct("ResolvedSecrets")
            .field("env", &names)
            .field("values", &"<redacted>")
            .field("scopes", &self.scopes)
            .field("warnings", &self.warnings)
            .field("skipped", &self.skipped)
            .field("credential", &self.credential)
            .finish()
    }
}

/// The exact text of the warning a name with no row anywhere produces.
fn undefined_warning(name: &str) -> String {
    format!("secret {name} is not defined at any scope")
}

/// Where a scope sits in the resolution order: the higher number wins.
///
/// The order is the resolver's own (`ARCHITECTURE.md`, "Secrets", Resolution
/// at launch), deliberately not the declaration order of [`SecretScope`], so
/// that adding a variant to the enum cannot silently reorder a launch.
fn rank(scope: SecretScope) -> u8 {
    match scope {
        SecretScope::Global => 0,
        SecretScope::Project => 1,
        SecretScope::User => 2,
    }
}

/// What the credential slot resolved to, for a launch or for the preflight
/// that answers it without launching.
///
/// The three outcomes a caller has to tell apart: no row carries any of the
/// names, the winning row is injectable, or the winning row is
/// `orchestrator_only` and therefore withheld. [`select_credential`] is the
/// one place that decides between them, so the launch and
/// `GET /projects/{pid}/agent-credentials` cannot disagree about which
/// credential a launch uses (`SPEC.md`, "Secrets").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSlot {
    /// No row at any of the three scopes carries one of the names.
    Empty,
    /// The winning row, which is injected.
    Injectable(Secret),
    /// The winning row, which is `orchestrator_only` and so is not injected.
    /// No lower-precedence row takes its place: the more specific row already
    /// overrode them.
    Withheld(Secret),
}

/// The credential slot of one `(project, user)` pair, out of every row
/// carrying one of `credential_names`.
///
/// The names are one slot, so the comparison is the scope first: the most
/// specific wins whatever name it carries. Two rows at the *same* scope can
/// only be rows older than the one-credential-per-scope write rule
/// (`ARCHITECTURE.md`, "Secrets", Agent credentials); the tie is broken by the
/// backend's own order, which is the order `credential_names` arrived in, and
/// logged with the names and the scope so an operator can delete one. No
/// value, and no name that was not asked for, reaches that line (rule 3).
///
/// The `orchestrator_only` flag is read last and here rather than in the
/// callers, because "which credential would a launch use" has to have one
/// answer: [`resolve_for_launch`] and [`preview_credential`] both take it from
/// this function.
pub fn select_credential(credential_names: &[String], rows: Vec<Secret>) -> CredentialSlot {
    match pick_credential(credential_names, rows) {
        None => CredentialSlot::Empty,
        Some(winner) if winner.orchestrator_only => CredentialSlot::Withheld(winner),
        Some(winner) => CredentialSlot::Injectable(winner),
    }
}

/// The highest-precedence row of the slot, before the flag is consulted.
fn pick_credential(credential_names: &[String], rows: Vec<Secret>) -> Option<Secret> {
    // Lower is more preferred; a row whose name is not in the list cannot
    // reach here, and would sort last if it did.
    let preference = |row: &Secret| {
        credential_names
            .iter()
            .position(|name| *name == row.name)
            .unwrap_or(usize::MAX)
    };

    let winner = rows
        .iter()
        .min_by_key(|row| (std::cmp::Reverse(rank(row.scope)), preference(row)))?
        .clone();

    let contenders: Vec<&str> = rows
        .iter()
        .filter(|row| row.scope == winner.scope)
        .map(|row| row.name.as_str())
        .collect();
    if contenders.len() > 1 {
        warn!(
            secret_names = ?contenders,
            scope = %winner.scope,
            chosen = %winner.name,
            "more than one agent credential at one scope; keep exactly one"
        );
    }

    Some(winner)
}

/// Resolve `names` and the agent credential for one launch: the environment,
/// the warnings and the audit rows.
///
/// Called by the session launcher between the git checkout and the container
/// creation (`ARCHITECTURE.md`, "Launch sequence"). Duplicates in `names`
/// resolve once and `env` has each key at most once, so a profile listing a
/// name twice cannot produce two conflicting environment entries.
///
/// `names` are [`SecretName`]s, which the profile that declared them yields
/// ([`AgentProfile::secret_names`](crate::models::AgentProfile::secret_names)):
/// the pattern is checked once, where a profile is stored, and not again here.
/// A name that is not one therefore never reaches this function, and a name
/// that no row carries is a warning rather than a failure.
///
/// `credentials` are the names the session's backend authenticates with
/// ([`AgentBackend::credential_names`](crate::agent::AgentBackend::credential_names)),
/// and they are treated as one slot: the row at the most specific scope
/// carrying any of them is the credential, whichever of the names it carries,
/// and it alone is decrypted and injected. Nothing here knows which backend
/// they belong to, so a credential that resolves to nothing is reported as
/// `credential: None` and not as a warning — the message that names the
/// backend is the launcher's (ADR 0036; `ARCHITECTURE.md`, "Secrets", Agent
/// credentials). A name in both lists is resolved once, as a credential, so a
/// profile that still lists one cannot make it a second environment entry.
///
/// # Errors
///
/// [`Error::Internal`] if a winning row will not decrypt or does not hold
/// UTF-8. Either is a corrupt row or a master key that no longer matches it,
/// never a caller's mistake, so the launch fails rather than starting a
/// container with a silently missing credential.
#[instrument(
    skip_all,
    fields(
        session_id = %scope.session_id,
        project_id = %scope.project_id,
        requested = names.len(),
        credentials = credentials.len(),
    )
)]
pub async fn resolve_for_launch(
    pool: &PgPool,
    keyring: &SecretsKeyring,
    scope: LaunchScope,
    names: &[SecretName],
    credentials: &[SecretName],
) -> Result<ResolvedSecrets> {
    let mut resolved = ResolvedSecrets {
        env: Vec::new(),
        scopes: Vec::new(),
        warnings: Vec::new(),
        skipped: Vec::new(),
        credential: None,
    };

    // The credential slot first, so a name in both lists is a credential and
    // not a profile secret: one row, one environment entry, one audit row.
    let mut credential_names: Vec<String> = Vec::with_capacity(credentials.len());
    for name in credentials {
        let name = name.as_str();
        if credential_names.iter().any(|seen| seen == name) {
            continue;
        }
        credential_names.push(name.to_string());
    }

    // First occurrence wins the position; a repeated name is resolved once.
    // Nothing is validated here: the names arrive as `SecretName`s from the
    // profile that declared them, which is where the one name rule is applied
    // (`AgentProfile::secret_names`).
    let mut wanted: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        let name = name.as_str();
        if wanted.iter().any(|seen| seen == name)
            || credential_names.iter().any(|seen| seen == name)
        {
            continue;
        }
        wanted.push(name.to_string());
    }

    if wanted.is_empty() && credential_names.is_empty() {
        // Nothing to look up and nothing to record: a profile with no secrets
        // and a backend with no credential do not open a transaction.
        return Ok(resolved);
    }

    // One query for every wanted name at the three scopes, credentials
    // included: the group is what the rules are about, so it is read as one.
    let mut all_wanted = wanted.clone();
    all_wanted.extend(credential_names.iter().cloned());

    let repository = SecretRepository::new(pool);
    let candidates = repository
        .find_for_resolution(&all_wanted, scope.project_id, scope.created_by)
        .await?;

    // One entry per name, holding the highest-precedence row seen so far. The
    // query's order is not relied on: the rank decides.
    let mut winners: HashMap<String, Secret> = HashMap::with_capacity(wanted.len());
    let mut credential_rows: Vec<Secret> = Vec::new();
    for row in candidates {
        if credential_names.contains(&row.name) {
            credential_rows.push(row);
            continue;
        }

        match winners.get(&row.name) {
            Some(current) if rank(current.scope) >= rank(row.scope) => {}
            _ => {
                winners.insert(row.name.clone(), row);
            }
        }
    }

    let credential_slot = select_credential(&credential_names, credential_rows);

    // Decide every name before anything is opened or written, so the two
    // outcomes that produce no audit row — missing and suppressed — are
    // settled outside the transaction.
    let mut injected: Vec<Secret> = Vec::with_capacity(wanted.len() + 1);
    for name in &wanted {
        let Some(winner) = winners.remove(name) else {
            resolved.warnings.push(undefined_warning(name));
            continue;
        };

        if winner.orchestrator_only {
            info!(
                secret_name = %name,
                scope = %winner.scope,
                "orchestrator-only secret skipped at launch"
            );
            resolved.skipped.push(name.clone());
            continue;
        }

        injected.push(winner);
    }

    // The credential is decided the same way and last, so it sits at the end
    // of `env`. A winner that is `orchestrator_only` — only possible for a row
    // older than the write rule that forbids it — is skipped exactly like any
    // other suppressed name, and no lower-precedence credential takes its
    // place: the more specific row already overrode them.
    match credential_slot {
        CredentialSlot::Empty => {}
        CredentialSlot::Withheld(winner) => {
            info!(
                secret_name = %winner.name,
                scope = %winner.scope,
                "orchestrator-only secret skipped at launch"
            );
            resolved.skipped.push(winner.name.clone());
        }
        CredentialSlot::Injectable(winner) => match SecretName::parse(&winner.name) {
            Ok(name) => {
                resolved.credential = Some(ResolvedCredential {
                    name,
                    scope: winner.scope,
                });
                injected.push(winner);
            }
            Err(_) => {
                // Unreachable: the row was found by one of the names the
                // caller passed as a `SecretName`.
                error!(
                    secret_name = %winner.name,
                    "a credential row carries a name that is not a valid secret name"
                );
            }
        },
    }

    if injected.is_empty() {
        return Ok(resolved);
    }

    // The audit trail and the decryption share one transaction: a value that
    // will not open drops the transaction on the way out, so no row claims a
    // launch read a secret it never got (`docs/data-model.md`, `secret_uses`).
    let mut tx = pool.begin().await?;

    for winner in &injected {
        let plaintext = winner.sealed().open(keyring).map_err(|err| {
            // The error distinguishes a master key the environment no longer
            // carries from a row that will not verify, which is what an
            // operator reading this line needs; the client is told only that
            // the launch failed.
            error!(
                secret_name = %winner.name,
                key_version = winner.key_version,
                error = %err,
                "a secret could not be decrypted at launch"
            );
            Error::Internal("a secret could not be decrypted".to_string())
        })?;

        // `to_string` copies into a buffer that is zeroized in turn; the
        // plaintext this borrows from is zeroized when it drops at the end of
        // the iteration, so no readable copy outlives either.
        let value = match std::str::from_utf8(&plaintext) {
            Ok(text) => Zeroizing::new(text.to_string()),
            Err(_) => {
                error!(
                    secret_name = %winner.name,
                    key_version = winner.key_version,
                    "a secret is not valid UTF-8 and cannot become an environment variable"
                );
                return Err(Error::Internal("a secret is not valid UTF-8".to_string()));
            }
        };

        repository
            .insert_use(
                &mut tx,
                winner.id,
                Some(scope.session_id),
                scope.created_by,
                SecretUsePurpose::Launch,
            )
            .await?;

        resolved.scopes.push((winner.name.clone(), winner.scope));
        resolved.env.push((winner.name.clone(), value));
    }

    tx.commit().await?;

    info!(
        injected = resolved.env.len(),
        skipped = resolved.skipped.len(),
        missing = resolved.warnings.len(),
        "profile secrets resolved for launch"
    );

    Ok(resolved)
}

/// The credential a preflight reports: which row, under which name, at which
/// scope — never a value (`SPEC.md`, "Secrets", `AgentCredentialStatus`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialPreview {
    pub secret_id: Uuid,
    pub name: SecretName,
    pub scope: SecretScope,
}

/// Which credential a launch of `project_id` by `user_id` would be given, out
/// of `credentials`, without launching anything.
///
/// The preflight behind `GET /projects/{pid}/agent-credentials` (`SPEC.md`,
/// "Secrets"): the same query at the same three scopes and the same
/// [`select_credential`] as [`resolve_for_launch`], so the two cannot disagree
/// about which row wins. `user_id` is the *caller*, which is what makes the
/// answer "if I launch now" rather than "if anyone launches"; a user-scoped row
/// in it is therefore always the caller's own.
///
/// Nothing is decrypted, no `secret_uses` row is written and no value or
/// ciphertext leaves this function: a [`CredentialSlot::Withheld`] winner
/// answers `None`, exactly as a launch would inject nothing, and so does an
/// empty slot. Which backend the names belong to is the caller's knowledge,
/// like everywhere else in this module: the caller passes one backend's names
/// and labels the answer with it.
#[instrument(skip_all, fields(project_id = %project_id, credentials = credentials.len()))]
pub async fn preview_credential(
    pool: &PgPool,
    project_id: Uuid,
    user_id: Uuid,
    credentials: &[SecretName],
) -> Result<Option<CredentialPreview>> {
    let mut names: Vec<String> = Vec::with_capacity(credentials.len());
    for name in credentials {
        let name = name.as_str();
        if names.iter().any(|seen| seen == name) {
            continue;
        }
        names.push(name.to_string());
    }

    if names.is_empty() {
        // A backend whose image carries its own authentication declares no
        // name, which is an empty slot and not a query.
        return Ok(None);
    }

    let rows = SecretRepository::new(pool)
        .find_for_resolution(&names, project_id, Some(user_id))
        .await?;

    let CredentialSlot::Injectable(winner) = select_credential(&names, rows) else {
        return Ok(None);
    };

    let Ok(name) = SecretName::parse(&winner.name) else {
        // Unreachable: the row was found by one of the names the caller passed
        // as a `SecretName`.
        error!(
            secret_name = %winner.name,
            "a credential row carries a name that is not a valid secret name"
        );
        return Ok(None);
    };

    Ok(Some(CredentialPreview {
        secret_id: winner.id,
        name,
        scope: winner.scope,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_warning_names_the_secret_and_says_nothing_else() {
        assert_eq!(
            undefined_warning("DEPLOY_TOKEN"),
            "secret DEPLOY_TOKEN is not defined at any scope"
        );
    }

    #[test]
    fn the_user_scope_outranks_the_project_scope_and_both_outrank_global() {
        assert!(rank(SecretScope::User) > rank(SecretScope::Project));
        assert!(rank(SecretScope::Project) > rank(SecretScope::Global));
    }

    #[test]
    fn the_debug_of_a_resolution_carries_names_and_no_values() {
        let resolved = ResolvedSecrets {
            env: vec![(
                "DEPLOY_TOKEN".to_string(),
                Zeroizing::new("fake-token-value".to_string()),
            )],
            scopes: vec![("DEPLOY_TOKEN".to_string(), SecretScope::Project)],
            warnings: vec![undefined_warning("ABSENT")],
            skipped: vec!["WITHHELD".to_string()],
            credential: Some(ResolvedCredential {
                name: SecretName::parse("CLAUDE_CODE_OAUTH_TOKEN")
                    .expect("the credential name is valid"),
                scope: SecretScope::User,
            }),
        };

        let rendered = format!("{resolved:?}");

        assert!(rendered.contains("DEPLOY_TOKEN"), "{rendered}");
        assert!(rendered.contains("WITHHELD"), "{rendered}");
        assert!(!rendered.contains("fake-token-value"), "{rendered}");
    }
}
