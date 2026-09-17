//! The master keyring: the versioned keys envelope encryption wraps data keys
//! under (`ARCHITECTURE.md`, "Secrets", Keyring).
//!
//! `SECRETS_MASTER_KEYS` holds one or more `<version>=<base64 32 bytes>`
//! entries, comma separated; `SECRETS_MASTER_KEY_FILE` points at a file with
//! the same content. The highest version is used for new rows.
//!
//! Nothing here formats, logs or `Debug`-prints key material: an error names
//! the version that is wrong and stops there (rule 3).

use std::collections::BTreeMap;
use std::fmt;

use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use zeroize::{Zeroize, Zeroizing};

use crate::prelude::*;

/// The length every master key has, fixed by AES-256-GCM.
pub const MASTER_KEY_LEN: usize = 32;

/// Anything that stops the secrets machinery working.
///
/// The envelope crypto epic adds its own variants (wrap, unwrap, missing key
/// version for a stored row); this task only needs the ones the keyring
/// itself produces.
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    /// `SECRETS_MASTER_KEYS` could not be read as a keyring. The message names
    /// the offending version or position and never the value.
    #[error("the master key configuration is invalid: {0}")]
    InvalidMasterKey(String),
    /// `SECRETS_MASTER_KEY_FILE` could not be read.
    #[error("the master key file could not be read: {0}")]
    MasterKeyFile(String),
}

impl SecretsError {
    /// The HTTP status this failure maps to. A caller never influences the
    /// keyring, so every failure here is an internal fault.
    pub fn status(&self) -> StatusCode {
        match self {
            SecretsError::InvalidMasterKey(_) | SecretsError::MasterKeyFile(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

/// The master keys, keyed by version.
///
/// Cloneable through an inner [`Arc`] so [`AppState`] can hold it by value and
/// still clone into every handler without copying key material.
#[derive(Clone)]
pub struct SecretsKeyring {
    inner: Arc<Inner>,
}

struct Inner {
    /// Sorted, so the highest version is the last entry.
    keys: BTreeMap<u32, Zeroizing<[u8; MASTER_KEY_LEN]>>,
    current: u32,
}

impl SecretsKeyring {
    /// Build a keyring from decoded keys.
    ///
    /// Rejects an empty set and duplicate versions. The caller's copies are
    /// zeroized on the way in.
    pub fn from_entries(
        mut entries: Vec<(u32, [u8; MASTER_KEY_LEN])>,
    ) -> std::result::Result<Self, SecretsError> {
        if entries.is_empty() {
            return Err(SecretsError::InvalidMasterKey(
                "no master key entries were given".to_string(),
            ));
        }

        let mut keys: BTreeMap<u32, Zeroizing<[u8; MASTER_KEY_LEN]>> = BTreeMap::new();
        for (version, key) in entries.iter_mut() {
            if keys.contains_key(version) {
                key.zeroize();
                return Err(SecretsError::InvalidMasterKey(format!(
                    "version {version} appears more than once"
                )));
            }
            keys.insert(*version, Zeroizing::new(*key));
            key.zeroize();
        }

        let current = *keys
            .keys()
            .next_back()
            .expect("a non-empty map has a last key");

        Ok(Self {
            inner: Arc::new(Inner { keys, current }),
        })
    }

    /// Parse the `<version>=<base64 32 bytes>` entries of
    /// `SECRETS_MASTER_KEYS` or the file `SECRETS_MASTER_KEY_FILE` points at.
    ///
    /// Entries are separated by commas or newlines — a file written one entry
    /// per line works — and whitespace around an entry is ignored. Both padded
    /// and unpadded standard base64 are accepted, so a value trimmed of its
    /// `=` padding still loads.
    pub fn parse(raw: &str) -> std::result::Result<Self, SecretsError> {
        let mut entries = Vec::new();

        for (index, entry) in raw
            .split([',', '\n', '\r'])
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .enumerate()
        {
            let position = index + 1;
            let (version, encoded) = entry.split_once('=').ok_or_else(|| {
                SecretsError::InvalidMasterKey(format!(
                    "entry {position} is not <version>=<base64 key>"
                ))
            })?;

            let version: u32 = version.trim().parse().map_err(|_| {
                SecretsError::InvalidMasterKey(format!(
                    "entry {position} has a non-numeric version"
                ))
            })?;

            let encoded = encoded.trim();
            let decoded = STANDARD
                .decode(encoded)
                .or_else(|_| STANDARD_NO_PAD.decode(encoded))
                .map_err(|_| {
                    SecretsError::InvalidMasterKey(format!(
                        "the key for version {version} is not valid base64"
                    ))
                })?;

            // `Zeroizing` clears the decoded buffer when it drops, on both the
            // success and the error path.
            let decoded = Zeroizing::new(decoded);
            let key: [u8; MASTER_KEY_LEN] = decoded.as_slice().try_into().map_err(|_| {
                SecretsError::InvalidMasterKey(format!(
                    "the key for version {version} decodes to {} bytes, not {MASTER_KEY_LEN}",
                    decoded.len()
                ))
            })?;

            entries.push((version, key));
        }

        if entries.is_empty() {
            return Err(SecretsError::InvalidMasterKey(
                "no <version>=<base64 key> entries were found".to_string(),
            ));
        }

        Self::from_entries(entries)
    }

    /// A keyring with one fixed, obviously fake key, for tests only (rule 3).
    #[cfg(feature = "integration-tests")]
    pub fn test_key() -> Self {
        let mut key = [0u8; MASTER_KEY_LEN];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = (index + 1) as u8;
        }

        Self::from_entries(vec![(1, key)]).expect("one entry is a valid keyring")
    }

    /// The highest version; new rows are wrapped under it.
    pub fn current_version(&self) -> u32 {
        self.inner.current
    }

    /// The key for one version, or `None` when the environment does not carry
    /// it. Rotation needs this for rows still on an older version.
    pub fn key(&self, version: u32) -> Option<&[u8; MASTER_KEY_LEN]> {
        self.inner.keys.get(&version).map(|key| &**key)
    }

    /// Every version present, ascending.
    pub fn versions(&self) -> Vec<u32> {
        self.inner.keys.keys().copied().collect()
    }
}

impl fmt::Debug for SecretsKeyring {
    /// Versions only: key material must never reach a log through a `{:?}` on
    /// a struct that holds a keyring (rule 3).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretsKeyring")
            .field("versions", &self.versions())
            .field("current_version", &self.current_version())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Obviously fake: 32 bytes of one repeated value (rule 3).
    fn fake_key(byte: u8) -> String {
        STANDARD.encode([byte; MASTER_KEY_LEN])
    }

    fn invalid_master_key(error: SecretsError) -> String {
        match error {
            SecretsError::InvalidMasterKey(message) => message,
            other => panic!("expected an invalid master key error, got {other:?}"),
        }
    }

    #[test]
    fn one_valid_entry_parses() {
        let keyring = SecretsKeyring::parse(&format!("1={}", fake_key(0xAA)))
            .expect("a single valid entry parses");

        assert_eq!(keyring.versions(), vec![1]);
        assert_eq!(keyring.current_version(), 1);
        assert_eq!(keyring.key(1), Some(&[0xAAu8; MASTER_KEY_LEN]));
        assert!(keyring.key(2).is_none());
    }

    #[test]
    fn the_highest_version_is_current_whatever_the_order() {
        let raw = format!(
            " 7={} , 2={} ,\n 11={} ",
            fake_key(0x07),
            fake_key(0x02),
            fake_key(0x0B)
        );
        let keyring = SecretsKeyring::parse(&raw).expect("three valid entries parse");

        assert_eq!(keyring.versions(), vec![2, 7, 11]);
        assert_eq!(keyring.current_version(), 11);
    }

    #[test]
    fn unpadded_base64_is_accepted() {
        let unpadded = STANDARD_NO_PAD.encode([0x5Au8; MASTER_KEY_LEN]);
        let keyring = SecretsKeyring::parse(&format!("1={unpadded}"))
            .expect("unpadded standard base64 parses");

        assert_eq!(keyring.key(1), Some(&[0x5Au8; MASTER_KEY_LEN]));
    }

    #[test]
    fn an_empty_value_is_rejected() {
        let message =
            invalid_master_key(SecretsKeyring::parse("").expect_err("an empty value is rejected"));
        assert!(message.contains("no <version>=<base64 key> entries"));

        let message = invalid_master_key(
            SecretsKeyring::parse("  \n ").expect_err("only whitespace is rejected"),
        );
        assert!(message.contains("no <version>=<base64 key> entries"));
    }

    #[test]
    fn a_key_of_the_wrong_length_is_rejected_naming_only_the_version() {
        let short = STANDARD.encode([0x01u8; 16]);
        let message = invalid_master_key(
            SecretsKeyring::parse(&format!("3={short}")).expect_err("16 bytes is not a master key"),
        );

        assert!(message.contains("version 3"), "{message}");
        assert!(message.contains("16 bytes"), "{message}");
        assert!(
            !message.contains(&short),
            "the message must not echo the key"
        );
    }

    #[test]
    fn bad_base64_is_rejected_naming_only_the_version() {
        let message = invalid_master_key(
            SecretsKeyring::parse("4=not-valid-base64!!").expect_err("bad base64 is rejected"),
        );

        assert!(message.contains("version 4"), "{message}");
        assert!(!message.contains("not-valid-base64"), "{message}");
    }

    #[test]
    fn a_non_numeric_version_is_rejected() {
        let message = invalid_master_key(
            SecretsKeyring::parse(&format!("v1={}", fake_key(0x01)))
                .expect_err("a non-numeric version is rejected"),
        );
        assert!(message.contains("non-numeric version"), "{message}");

        // A padded key on its own splits at its own `=`, so the version is
        // what is wrong with it; a value with no `=` at all has no shape.
        let message = invalid_master_key(
            SecretsKeyring::parse(&fake_key(0x01)).expect_err("a bare padded key is rejected"),
        );
        assert!(message.contains("non-numeric version"), "{message}");

        let message = invalid_master_key(
            SecretsKeyring::parse("no-separator-here")
                .expect_err("an entry with no `=` is rejected"),
        );
        assert!(message.contains("<version>=<base64 key>"), "{message}");
    }

    #[test]
    fn a_duplicate_version_is_rejected() {
        let raw = format!("1={},1={}", fake_key(0x01), fake_key(0x02));
        let message = invalid_master_key(
            SecretsKeyring::parse(&raw).expect_err("a duplicate version is rejected"),
        );

        assert!(message.contains("version 1"), "{message}");
        assert!(message.contains("more than once"), "{message}");
    }

    #[test]
    fn from_entries_rejects_an_empty_set() {
        let message = invalid_master_key(
            SecretsKeyring::from_entries(Vec::new()).expect_err("an empty keyring is rejected"),
        );
        assert!(message.contains("no master key entries"), "{message}");
    }

    #[test]
    fn debug_shows_versions_and_no_key_material() {
        let keyring = SecretsKeyring::from_entries(vec![(1, [0xABu8; MASTER_KEY_LEN])])
            .expect("one entry is a valid keyring");

        let rendered = format!("{keyring:?}");

        assert!(rendered.contains("versions"), "{rendered}");
        assert!(rendered.contains('1'), "{rendered}");
        assert!(!rendered.contains("171"), "{rendered}");
        assert!(!rendered.contains("ab"), "{rendered}");
    }

    #[test]
    fn cloning_shares_the_keys_rather_than_copying_them() {
        let keyring = SecretsKeyring::from_entries(vec![(1, [0x11u8; MASTER_KEY_LEN])])
            .expect("one entry is a valid keyring");
        let clone = keyring.clone();

        assert!(Arc::ptr_eq(&keyring.inner, &clone.inner));
    }

    #[cfg(feature = "integration-tests")]
    #[test]
    fn the_test_key_is_version_one_and_fixed() {
        let keyring = SecretsKeyring::test_key();

        assert_eq!(keyring.versions(), vec![1]);
        assert_eq!(keyring.current_version(), 1);

        let key = keyring.key(1).expect("version 1 is present");
        assert_eq!(key[0], 0x01);
        assert_eq!(key[MASTER_KEY_LEN - 1], 0x20);
    }
}
