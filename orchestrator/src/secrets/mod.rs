//! Envelope crypto, secret resolution and injection into sessions.
//!
//! Only the master keyring exists yet ([`keyring`]); the secrets epic adds the
//! data-key wrapping, the resolution order and the startup check that every
//! `key_version` present in the table can still be unwrapped
//! (`ARCHITECTURE.md`, "Secrets").

pub mod keyring;

pub use keyring::{MASTER_KEY_LEN, SecretsError, SecretsKeyring};

use crate::prelude::*;

/// Build the keyring from whichever master key source the configuration
/// chose.
///
/// `SECRETS_MASTER_KEYS` carries the entries directly;
/// `SECRETS_MASTER_KEY_FILE` points at a file with the same content
/// (`README.md`, "Configuration"). Neither the value nor the file's content
/// ever reaches an error message (rule 3).
pub fn load_keyring(config: &Config) -> std::result::Result<SecretsKeyring, SecretsError> {
    match &config.secrets_master_keys {
        SecretsMasterKeySource::Inline(raw) => SecretsKeyring::parse(raw),
        SecretsMasterKeySource::File(path) => {
            // The path is not a secret; the file's content is, and never
            // appears here.
            let raw = std::fs::read_to_string(path)
                .map_err(|err| SecretsError::MasterKeyFile(format!("{}: {err}", path.display())))?;
            SecretsKeyring::parse(&raw)
        }
    }
}
