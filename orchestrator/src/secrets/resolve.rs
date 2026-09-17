//! Resolving a profile's `secrets` list into the environment of one session
//! container (`ARCHITECTURE.md`, "Secrets", Resolution at launch; "Launch
//! sequence").
//!
//! The launcher hands over the names the profile declares and the three things
//! that decide which rows they can see — the session, its project and the user
//! who created it — and gets back the environment to put in the container, the
//! `launch_warning` messages to append and the names that were deliberately
//! not injected. Everything the resolver decides is decided here: the
//! repository reads every candidate row in one query and filters nothing,
//! because precedence and suppression are decisions about a *group* of rows
//! rather than about any one of them.
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

use std::collections::HashMap;

use uuid::Uuid;
use zeroize::Zeroizing;

use super::crypto::{aad, open};
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
/// listed the names and with each name at most once. `warnings` are the
/// `launch_warning` messages the launcher appends before starting the
/// container, and `skipped` names the secrets that resolved to an
/// `orchestrator_only` row and are therefore absent from `env` on purpose —
/// the launcher has both so it can tell "never existed" from "withheld".
///
/// `Debug` redacts the values: the struct exists to carry credentials, so a
/// `?` field on it anywhere would be the one way they reach a log line
/// (rule 3).
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedSecrets {
    pub env: Vec<(String, Zeroizing<String>)>,
    pub warnings: Vec<String>,
    pub skipped: Vec<String>,
}

impl std::fmt::Debug for ResolvedSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Names are safe and are the whole point of printing this; values are
        // not.
        let names: Vec<&str> = self.env.iter().map(|(name, _)| name.as_str()).collect();

        f.debug_struct("ResolvedSecrets")
            .field("env", &names)
            .field("values", &"<redacted>")
            .field("warnings", &self.warnings)
            .field("skipped", &self.skipped)
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

/// Resolve `names` for one launch: the environment, the warnings and the audit
/// rows.
///
/// Called by the session launcher between the git checkout and the container
/// creation (`ARCHITECTURE.md`, "Launch sequence"). Duplicates in `names`
/// resolve once and `env` has each key at most once, so a profile listing a
/// name twice cannot produce two conflicting environment entries.
///
/// The refusal to inject `ANTHROPIC_API_KEY` and `CLAUDE_CODE_OAUTH_TOKEN`
/// together is the launcher's rule applied to `env` afterwards, not this
/// function's: it is about what the CLI tolerates, not about what the caller
/// is allowed to read.
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
    )
)]
pub async fn resolve_for_launch(
    pool: &PgPool,
    keyring: &SecretsKeyring,
    scope: LaunchScope,
    names: &[String],
) -> Result<ResolvedSecrets> {
    let mut resolved = ResolvedSecrets {
        env: Vec::new(),
        warnings: Vec::new(),
        skipped: Vec::new(),
    };

    // First occurrence wins the position; a repeated name is resolved once.
    // The malformed ones are dropped here rather than sent to the query: a
    // profile that passed validation cannot contain one, and a row could not
    // exist under such a name anyway, so it is reported as missing.
    let mut seen: Vec<&String> = Vec::with_capacity(names.len());
    let mut wanted: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);

        if SecretName::parse(name).is_err() {
            warn!(
                secret_name = %name,
                "a profile secret name is not a valid secret name"
            );
            resolved.warnings.push(undefined_warning(name));
            continue;
        }
        wanted.push(name.clone());
    }

    if wanted.is_empty() {
        // Nothing to look up and nothing to record: a profile with no secrets
        // does not open a transaction.
        return Ok(resolved);
    }

    let repository = SecretRepository::new(pool);
    let candidates = repository
        .find_for_resolution(&wanted, scope.project_id, scope.created_by)
        .await?;

    // One entry per name, holding the highest-precedence row seen so far. The
    // query's order is not relied on: the rank decides.
    let mut winners: HashMap<String, Secret> = HashMap::with_capacity(wanted.len());
    for row in candidates {
        match winners.get(&row.name) {
            Some(current) if rank(current.scope) >= rank(row.scope) => {}
            _ => {
                winners.insert(row.name.clone(), row);
            }
        }
    }

    // Decide every name before anything is opened or written, so the two
    // outcomes that produce no audit row — missing and suppressed — are
    // settled outside the transaction.
    let mut injected: Vec<Secret> = Vec::with_capacity(wanted.len());
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

    if injected.is_empty() {
        return Ok(resolved);
    }

    // The audit trail and the decryption share one transaction: a value that
    // will not open drops the transaction on the way out, so no row claims a
    // launch read a secret it never got (`docs/data-model.md`, `secret_uses`).
    let mut tx = pool.begin().await?;

    for winner in &injected {
        let plaintext = open(
            keyring,
            &aad(winner.scope, winner.scope_id, &winner.name),
            &winner.encrypted_value(),
        )
        .map_err(|_| {
            error!(
                secret_name = %winner.name,
                key_version = winner.key_version,
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
            warnings: vec![undefined_warning("ABSENT")],
            skipped: vec!["WITHHELD".to_string()],
        };

        let rendered = format!("{resolved:?}");

        assert!(rendered.contains("DEPLOY_TOKEN"), "{rendered}");
        assert!(rendered.contains("WITHHELD"), "{rendered}");
        assert!(!rendered.contains("fake-token-value"), "{rendered}");
    }
}
