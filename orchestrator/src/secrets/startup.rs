//! The boot check that the configured master keys still open what is stored
//! (`ARCHITECTURE.md`, "Secrets", Keyring).
//!
//! > At start the keyring verifies it can unwrap one row per `key_version`
//! > present and refuses to start otherwise.
//!
//! It lives here rather than on [`SecretsKeyring`] because it is the one part
//! of that sentence that is not cryptography: the keyring parses the
//! configuration, picks a version and wraps and unwraps data keys, and knows
//! nothing about a table. Reading the samples is the repository's, and putting
//! the two together is this function's.
//!
//! No key material and no ciphertext reaches the message or the log line: the
//! versions are the whole of it (`CLAUDE.md`, rule 3).

use crate::prelude::*;
use crate::repositories::SecretRepository;
use crate::secrets::{SecretsError, SecretsKeyring};

/// Check at startup that every `key_version` in `secrets` can be unwrapped.
///
/// One sampled row per distinct version, unwrapped with the key configured for
/// it. A missing key would otherwise only surface at a session launch, long
/// after the operator could connect it to the restart that caused it
/// (`ARCHITECTURE.md`, "Secrets", Keyring), so `main` treats a failure here as
/// fatal and runs it before any session is adopted (`ARCHITECTURE.md`,
/// "Restart procedure"). Every offending version is reported in one message
/// rather than the first one found, so adding the keys takes one restart.
///
/// An empty table passes trivially.
pub async fn verify_keyring_at_startup(pool: &PgPool, keyring: &SecretsKeyring) -> Result<()> {
    let samples = SecretRepository::new(pool).sample_per_key_version().await?;

    let mut problems = Vec::new();
    for sealed in &samples {
        match sealed.verify_unwrappable(keyring) {
            Ok(()) => {}
            Err(SecretsError::UnknownKeyVersion(version)) => problems.push(format!(
                "secrets: key version {version} has no configured master key"
            )),
            // A configured but wrong key, or a corrupt row: either way the
            // rows on this version cannot be read, and saying so is not
            // the same as saying the key is absent.
            Err(_) => problems.push(format!(
                "secrets: rows wrapped with key version {} cannot be unwrapped",
                sealed.wrapped.version
            )),
        }
    }

    if !problems.is_empty() {
        return Err(SecretsError::KeyVerification(problems.join("; ")).into());
    }

    debug!(
        versions = samples.len(),
        "every stored key version unwraps with the configured master keys"
    );

    Ok(())
}
