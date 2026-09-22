//! Envelope crypto, secret resolution and injection into sessions.
//!
//! The master keyring ([`keyring`]) holds the versioned keys and wraps and
//! unwraps per-row data keys; [`startup`] is what checks at boot that every
//! `key_version` present in the table can still be unwrapped
//! (`ARCHITECTURE.md`, "Secrets"), which is the one part of that sentence that
//! needs a database rather than a cipher.
//! [`envelope`] is the layer above it, and the only one: a [`SealedSecret`] is
//! one row's encrypted identity, and sealing a value, opening it, re-sealing it
//! when the row is renamed and re-wrapping its data key when the master key
//! rotates are its four methods. The resolution order and the injection into a
//! session are the rest of the secrets epic.

pub mod envelope;
pub mod git_credential;
pub mod keyring;
pub mod resolve;
pub mod rotation;
pub mod service;
pub mod startup;

pub use envelope::{SealedSecret, SecretIdentity, VALUE_NONCE_LEN};
pub use git_credential::{
    GIT_CREDENTIAL_NAME, GitUseContext, has_project_git_credential, insert_project_git_credential,
    project_git_credential, set_project_git_credential,
};
pub use keyring::{
    DATA_KEY_LEN, MASTER_KEY_LEN, SecretsError, SecretsKeyring, WRAP_NONCE_LEN, WrappedKey,
};
pub use resolve::{
    CredentialPreview, LaunchScope, NO_UNATTENDED_CREDENTIAL,
    NO_UNATTENDED_CREDENTIAL_FOR_SCHEDULE, ResolvedCredential, ResolvedSecrets,
    has_unattended_credential, preview_credential, require_unattended_credential,
    require_unattended_credential_saying, resolve_for_launch,
};
pub use rotation::{ROTATION_BATCH, RotationReport, rewrap_outdated};
pub use service::{
    Actor, CreateSecret, DEFAULT_USES_LIMIT, MAX_USES_LIMIT, PatchSecret, SecretsService,
};
pub use startup::verify_keyring_at_startup;

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
            warn_on_broad_mode(path);
            SecretsKeyring::parse(&raw)
        }
    }
}

/// Warn when the master key file is readable by anyone but its owner.
///
/// A warning rather than a refusal: the file is often a mounted container
/// secret whose mode the orchestrator does not control, and refusing to start
/// over a mode it cannot change would only leave the deployment with no way
/// forward. The path is logged, never the content (rule 3).
fn warn_on_broad_mode(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    // A file that was just read but cannot be stat-ed is not worth failing
    // over; the keyring itself is what decides whether the start succeeds.
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };

    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        warn!(
            path = %path.display(),
            mode = format!("{mode:04o}"),
            "the master key file is readable beyond its owner; 0600 is expected"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    use super::*;

    /// Obviously fake: 32 bytes of one repeated value (rule 3).
    fn fake_entry(version: u32, byte: u8) -> String {
        format!("{version}={}", STANDARD.encode([byte; MASTER_KEY_LEN]))
    }

    /// A configuration whose only interesting field is the key source.
    /// Everything else is the required minimum, obviously fake (rule 3).
    fn config_with(name: &str, value: &str) -> Config {
        let mut vars: HashMap<&str, String> = [
            ("PUBLIC_URL", "https://mars.example.invalid".to_string()),
            ("JWT_SECRET", "not-a-real-signing-secret".to_string()),
            (
                "DATABASE_URL",
                "postgres://mars:fake@localhost:5432/mars".to_string(),
            ),
            (
                "DOCKER_HOST",
                "unix:///run/user/1000/podman/podman.sock".to_string(),
            ),
            ("DATA_DIR_HOST", "/srv/mars/data".to_string()),
            ("GIT_BOT_NAME", "Mars Bot".to_string()),
            ("GIT_BOT_EMAIL", "mars-bot@example.invalid".to_string()),
            (
                "SESSION_IMAGE_DEFAULT",
                "mars-session-claude:dev".to_string(),
            ),
        ]
        .into_iter()
        .collect();
        vars.insert(name, value.to_string());

        Config::from_vars(|name| vars.get(name).cloned()).expect("a complete required set loads")
    }

    /// Write `content` to a file with `mode` and hand back the directory that
    /// owns it, so the caller keeps it alive for the length of the test.
    fn key_file(content: &str, mode: u32) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("master-keys");

        let mut file = std::fs::File::create(&path).expect("the key file is created");
        file.write_all(content.as_bytes())
            .expect("the key file is written");
        drop(file);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("the mode is set");

        (dir, path)
    }

    #[test]
    fn the_inline_value_is_parsed() {
        let config = config_with("SECRETS_MASTER_KEYS", &fake_entry(1, 0xAA));
        let keyring = load_keyring(&config).expect("the inline entries parse");

        assert_eq!(keyring.versions(), vec![1]);
    }

    #[test]
    fn a_key_file_is_read_and_its_trailing_newline_ignored() {
        // Written the way a file naturally is: one entry per line, final
        // newline included.
        let content = format!("{}\n{}\n", fake_entry(1, 0x01), fake_entry(2, 0x02));
        let (_dir, path) = key_file(&content, 0o600);

        let config = config_with("SECRETS_MASTER_KEY_FILE", &path.display().to_string());
        let keyring = load_keyring(&config).expect("the file's entries parse");

        assert_eq!(keyring.versions(), vec![1, 2]);
        assert_eq!(keyring.current_version(), 2);
    }

    #[test]
    fn a_key_file_readable_by_others_still_loads() {
        // A mounted container secret's mode is not always the operator's to
        // set, so this warns and carries on rather than refusing to start.
        let (_dir, path) = key_file(&fake_entry(1, 0x01), 0o644);

        let config = config_with("SECRETS_MASTER_KEY_FILE", &path.display().to_string());
        let keyring = load_keyring(&config).expect("a broad mode is accepted");

        assert_eq!(keyring.versions(), vec![1]);
    }

    #[test]
    fn a_missing_key_file_names_the_path_and_nothing_else() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("absent");

        let config = config_with("SECRETS_MASTER_KEY_FILE", &path.display().to_string());
        let error = load_keyring(&config).expect_err("a missing file is an error");

        match error {
            SecretsError::MasterKeyFile(message) => {
                assert!(message.contains("absent"), "{message}")
            }
            other => panic!("expected a master key file error, got {other:?}"),
        }
    }
}
