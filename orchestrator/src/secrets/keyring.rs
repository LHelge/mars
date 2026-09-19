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

use aes_gcm::aead::{Aead, KeyInit, Nonce};
use aes_gcm::{Aes256Gcm, Key};
use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use rand::Rng;
use zeroize::{Zeroize, Zeroizing};

use crate::prelude::*;
use crate::repositories::SecretRepository;
use crate::secrets::envelope::KeyVersionSample;

/// The length every master key has, fixed by AES-256-GCM.
pub const MASTER_KEY_LEN: usize = 32;

/// The length of a per-row data key: the same cipher, so the same 32 bytes.
pub const DATA_KEY_LEN: usize = 32;

/// The nonce length AES-GCM is specified for, and the width of
/// `secrets.data_key_nonce` (`docs/data-model.md`, `secrets`).
pub const WRAP_NONCE_LEN: usize = 12;

/// Anything that stops the secrets machinery working.
///
/// The value-level seal/open of the envelope-crypto task adds its own
/// variants; these are the ones the keyring itself produces.
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    /// `SECRETS_MASTER_KEYS` could not be read as a keyring. The message names
    /// the offending version or position and never the value.
    #[error("the master key configuration is invalid: {0}")]
    InvalidMasterKey(String),
    /// `SECRETS_MASTER_KEY_FILE` could not be read.
    #[error("the master key file could not be read: {0}")]
    MasterKeyFile(String),
    /// A row asks for a master key version the environment does not carry.
    #[error("secrets: key version {0} has no configured master key")]
    UnknownKeyVersion(i32),
    /// The tag did not verify: the stored bytes and the configured key for
    /// that version do not belong together.
    #[error("secrets: a wrapped data key could not be decrypted")]
    Decrypt,
    /// Wrapping a fresh data key failed. Unreachable in practice — AES-GCM
    /// refuses only absurd lengths — and kept so the path has no `expect`.
    #[error("secrets: a data key could not be wrapped")]
    Wrap,
    /// The startup check found stored rows the configured keys cannot open.
    ///
    /// The message is the list of offending versions, each already saying what
    /// is wrong with it, so the caller can log it beside its own sentence
    /// without repeating one. No key material is in it.
    #[error("{0}")]
    KeyVerification(String),
}

impl SecretsError {
    /// The HTTP status this failure maps to. A caller never influences the
    /// keyring — not the configuration, not a row's `key_version` — so every
    /// failure here is an internal fault.
    pub fn status(&self) -> StatusCode {
        match self {
            SecretsError::InvalidMasterKey(_)
            | SecretsError::MasterKeyFile(_)
            | SecretsError::UnknownKeyVersion(_)
            | SecretsError::Decrypt
            | SecretsError::Wrap
            | SecretsError::KeyVerification(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// A data key wrapped under one master key version.
///
/// The three fields are exactly the three columns that store them:
/// `data_key_wrapped`, `data_key_nonce` and `key_version`
/// (`docs/data-model.md`, `secrets`). `wrapped` is ciphertext with its
/// 16-byte tag appended, so it is [`DATA_KEY_LEN`] + 16 bytes long.
///
/// Both byte fields are `Vec<u8>` rather than fixed-width arrays because this
/// type is also how a *stored* row's wrapping is carried
/// ([`crate::secrets::SealedSecret`]): every wrap this module produces has a
/// [`WRAP_NONCE_LEN`] nonce, but a corrupt row can hold any width, and a type
/// that could not represent one would have to panic or truncate on the way out
/// of the database instead of failing the unwrap.
///
/// `Debug` shows the version only: the wrapping is not a plaintext key, but it
/// is still key material and nothing in it belongs in a log line (rule 3).
#[derive(Clone, PartialEq, Eq)]
pub struct WrappedKey {
    /// The wrapped data key, tag included.
    pub wrapped: Vec<u8>,
    /// The nonce the wrap used; unique per wrap, [`WRAP_NONCE_LEN`] bytes for
    /// anything this module wrapped.
    pub nonce: Vec<u8>,
    /// The master key version that wrapped it, as `key_version` stores it.
    pub version: i32,
}

impl fmt::Debug for WrappedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WrappedKey")
            .field("wrapped", &"<redacted>")
            .field("nonce", &"<redacted>")
            .field("version", &self.version)
            .finish()
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
    /// The same version as `key_version` stores it. Computed once, at
    /// construction, where the range check that makes it infallible lives.
    current_column: i32,
}

impl SecretsKeyring {
    /// Build a keyring from decoded keys.
    ///
    /// Rejects an empty set, duplicate versions and any version that
    /// `key_version` could not store: the column is `INTEGER`, so a version is
    /// a positive value that fits an `i32`. The caller's copies are zeroized
    /// on the way in.
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
            if let Err(err) = column_version(*version) {
                key.zeroize();
                return Err(err);
            }
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
        // Checked for every entry above, so this cannot fail; it is still
        // written as a conversion rather than a cast, so a later change to the
        // range check cannot silently wrap it.
        let current_column = column_version(current)?;

        Ok(Self {
            inner: Arc::new(Inner {
                keys,
                current,
                current_column,
            }),
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

    /// Wrap a fresh data key under the highest configured version.
    ///
    /// AES-256-GCM with a fresh 12-byte nonce and no additional authenticated
    /// data: what binds a *value* to its row is the AAD of the value's own
    /// encryption, and a wrapped data key carries nothing a row identity could
    /// be checked against (`ARCHITECTURE.md`, "Secrets"; ADR 0006). The three
    /// fields of the result are the three columns that store it.
    pub fn wrap_data_key(
        &self,
        data_key: &[u8; DATA_KEY_LEN],
    ) -> std::result::Result<WrappedKey, SecretsError> {
        let version = self.inner.current_column;
        // Always present: `current` is a key of the same map.
        let key = self
            .inner
            .keys
            .get(&self.inner.current)
            .ok_or(SecretsError::UnknownKeyVersion(version))?;
        let cipher = cipher_for(key);

        // A nonce is never reused under one key: 96 random bits per wrap, and
        // a rotation re-wraps with a new one rather than keeping the old.
        let mut nonce = [0u8; WRAP_NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce);

        let wrapped = cipher
            .encrypt(&Nonce::<Aes256Gcm>::from(nonce), data_key.as_slice())
            .map_err(|_| SecretsError::Wrap)?;

        Ok(WrappedKey {
            wrapped,
            nonce: nonce.to_vec(),
            version,
        })
    }

    /// Unwrap a stored data key under the version it was wrapped with.
    ///
    /// The three arguments are the three columns as they come out of a row.
    /// A version the environment does not carry is
    /// [`SecretsError::UnknownKeyVersion`] — the operator has to add the key —
    /// and a tag that does not verify is [`SecretsError::Decrypt`], which
    /// means the configured key for that version is not the one that wrapped
    /// the row. Neither message carries a byte of either.
    pub fn unwrap_data_key(
        &self,
        wrapped: &[u8],
        nonce: &[u8],
        version: i32,
    ) -> std::result::Result<Zeroizing<[u8; DATA_KEY_LEN]>, SecretsError> {
        let key = u32::try_from(version)
            .ok()
            .and_then(|version| self.inner.keys.get(&version))
            .ok_or(SecretsError::UnknownKeyVersion(version))?;

        // A stored nonce of the wrong width is a corrupt row, not a
        // configuration fault: it is reported exactly like a failed tag.
        let nonce: [u8; WRAP_NONCE_LEN] = nonce.try_into().map_err(|_| SecretsError::Decrypt)?;

        let cipher = cipher_for(key);
        let plain = Zeroizing::new(
            cipher
                .decrypt(&Nonce::<Aes256Gcm>::from(nonce), wrapped)
                .map_err(|_| SecretsError::Decrypt)?,
        );

        let key: [u8; DATA_KEY_LEN] = plain
            .as_slice()
            .try_into()
            .map_err(|_| SecretsError::Decrypt)?;

        Ok(Zeroizing::new(key))
    }

    /// Check at startup that every `key_version` in `secrets` can be unwrapped.
    ///
    /// One sampled row per distinct version, unwrapped with the key configured
    /// for it. A missing key would otherwise only surface at a session launch,
    /// long after the operator could connect it to the restart that caused it
    /// (`ARCHITECTURE.md`, "Secrets", Keyring), so `main` treats a failure here
    /// as fatal. Every offending version is reported in one message rather than
    /// the first one found, so adding the keys takes one restart.
    ///
    /// An empty table passes trivially.
    pub async fn verify_against_db(&self, pool: &PgPool) -> Result<()> {
        let samples = SecretRepository::new(pool)
            .distinct_key_version_samples()
            .await?;

        let mut problems = Vec::new();
        for KeyVersionSample {
            key_version,
            data_key_wrapped,
            data_key_nonce,
        } in &samples
        {
            match self.unwrap_data_key(data_key_wrapped, data_key_nonce, *key_version) {
                Ok(_) => {}
                Err(SecretsError::UnknownKeyVersion(version)) => problems.push(format!(
                    "secrets: key version {version} has no configured master key"
                )),
                // A configured but wrong key, or a corrupt row: either way the
                // rows on this version cannot be read, and saying so is not
                // the same as saying the key is absent.
                Err(_) => problems.push(format!(
                    "secrets: rows wrapped with key version {key_version} cannot be unwrapped"
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
}

/// The AES-256-GCM cipher for one master key.
///
/// Private, and takes the key rather than a version: the key material never
/// leaves this module, and every caller has already resolved the version it
/// wants.
fn cipher_for(key: &[u8; MASTER_KEY_LEN]) -> Aes256Gcm {
    Aes256Gcm::new(&Key::<Aes256Gcm>::from(*key))
}

/// One version as `secrets.key_version` stores it.
///
/// The column is `INTEGER` (`docs/data-model.md`, `secrets`), so a usable
/// version is positive and fits an `i32`. Rejecting the rest at construction
/// is what lets [`SecretsKeyring::wrap_data_key`] hand out an `i32` without a
/// cast that could wrap.
fn column_version(version: u32) -> std::result::Result<i32, SecretsError> {
    if version == 0 {
        return Err(SecretsError::InvalidMasterKey(
            "version 0 is not a key version; versions start at 1".to_string(),
        ));
    }

    i32::try_from(version).map_err(|_| {
        SecretsError::InvalidMasterKey(format!(
            "version {version} is larger than the key_version column can hold"
        ))
    })
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

    #[test]
    fn a_zero_version_is_rejected() {
        let message = invalid_master_key(
            SecretsKeyring::parse(&format!("0={}", fake_key(0x01)))
                .expect_err("version 0 is rejected"),
        );
        assert!(message.contains("versions start at 1"), "{message}");

        let message = invalid_master_key(
            SecretsKeyring::from_entries(vec![(0, [0x01u8; MASTER_KEY_LEN])])
                .expect_err("version 0 is rejected by the constructor too"),
        );
        assert!(message.contains("versions start at 1"), "{message}");
    }

    #[test]
    fn a_negative_version_is_rejected() {
        let message = invalid_master_key(
            SecretsKeyring::parse(&format!("-1={}", fake_key(0x01)))
                .expect_err("a negative version is rejected"),
        );
        assert!(message.contains("non-numeric version"), "{message}");
    }

    #[test]
    fn a_version_wider_than_the_column_is_rejected() {
        let too_wide = i32::MAX as u32 + 1;
        let message = invalid_master_key(
            SecretsKeyring::from_entries(vec![(too_wide, [0x01u8; MASTER_KEY_LEN])])
                .expect_err("a version the column cannot hold is rejected"),
        );
        assert!(message.contains("key_version column"), "{message}");
    }

    #[test]
    fn versions_are_compared_numerically_not_lexically() {
        let raw = format!(
            "2={},10={},9={}",
            fake_key(0x02),
            fake_key(0x0A),
            fake_key(0x09)
        );
        let keyring = SecretsKeyring::parse(&raw).expect("three valid entries parse");

        assert_eq!(keyring.versions(), vec![2, 9, 10]);
        assert_eq!(keyring.current_version(), 10);
        assert_eq!(
            keyring
                .wrap_data_key(&[0x11u8; DATA_KEY_LEN])
                .expect("wrapping succeeds")
                .version,
            10
        );
    }

    #[test]
    fn a_wrapped_data_key_unwraps_to_itself() {
        let keyring = SecretsKeyring::from_entries(vec![
            (1, [0x01u8; MASTER_KEY_LEN]),
            (2, [0x02u8; MASTER_KEY_LEN]),
        ])
        .expect("two entries are a valid keyring");

        let data_key = [0x42u8; DATA_KEY_LEN];
        let wrapped = keyring.wrap_data_key(&data_key).expect("the wrap succeeds");

        // The newest version, a 12-byte nonce and the data key plus a 16-byte
        // tag: the three columns the row stores.
        assert_eq!(wrapped.version, 2);
        assert_eq!(wrapped.nonce.len(), WRAP_NONCE_LEN);
        assert_eq!(wrapped.wrapped.len(), DATA_KEY_LEN + 16);
        assert_ne!(wrapped.wrapped.as_slice(), data_key.as_slice());

        let unwrapped = keyring
            .unwrap_data_key(&wrapped.wrapped, &wrapped.nonce, wrapped.version)
            .expect("the unwrap succeeds");
        assert_eq!(*unwrapped, data_key);
    }

    #[test]
    fn each_wrap_uses_a_fresh_nonce() {
        let keyring = SecretsKeyring::from_entries(vec![(1, [0x01u8; MASTER_KEY_LEN])])
            .expect("one entry is a valid keyring");
        let data_key = [0x42u8; DATA_KEY_LEN];

        let first = keyring.wrap_data_key(&data_key).expect("the wrap succeeds");
        let second = keyring.wrap_data_key(&data_key).expect("the wrap succeeds");

        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.wrapped, second.wrapped);
    }

    #[test]
    fn unwrapping_under_an_unconfigured_version_names_it() {
        let keyring = SecretsKeyring::from_entries(vec![(1, [0x01u8; MASTER_KEY_LEN])])
            .expect("one entry is a valid keyring");
        let wrapped = keyring
            .wrap_data_key(&[0x42u8; DATA_KEY_LEN])
            .expect("the wrap succeeds");

        for version in [7, 0, -3] {
            match keyring.unwrap_data_key(&wrapped.wrapped, &wrapped.nonce, version) {
                Err(SecretsError::UnknownKeyVersion(named)) => assert_eq!(named, version),
                other => panic!("expected an unknown key version, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_tampered_wrapping_fails_the_tag() {
        let keyring = SecretsKeyring::from_entries(vec![(1, [0x01u8; MASTER_KEY_LEN])])
            .expect("one entry is a valid keyring");
        let wrapped = keyring
            .wrap_data_key(&[0x42u8; DATA_KEY_LEN])
            .expect("the wrap succeeds");

        let mut tampered = wrapped.wrapped.clone();
        tampered[0] ^= 0xFF;
        assert!(matches!(
            keyring.unwrap_data_key(&tampered, &wrapped.nonce, wrapped.version),
            Err(SecretsError::Decrypt)
        ));

        // The tag itself, at the end, and the nonce are equally authenticated.
        let mut tampered = wrapped.wrapped.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        assert!(matches!(
            keyring.unwrap_data_key(&tampered, &wrapped.nonce, wrapped.version),
            Err(SecretsError::Decrypt)
        ));

        let mut nonce = wrapped.nonce.clone();
        nonce[0] ^= 0xFF;
        assert!(matches!(
            keyring.unwrap_data_key(&wrapped.wrapped, &nonce, wrapped.version),
            Err(SecretsError::Decrypt)
        ));
    }

    #[test]
    fn a_configured_but_wrong_key_fails_the_tag_rather_than_succeeding() {
        let wrapping = SecretsKeyring::from_entries(vec![(4, [0x04u8; MASTER_KEY_LEN])])
            .expect("one entry is a valid keyring");
        let wrapped = wrapping
            .wrap_data_key(&[0x42u8; DATA_KEY_LEN])
            .expect("the wrap succeeds");

        // The same version, a different key: the operator replaced the value
        // instead of adding a version.
        let other = SecretsKeyring::from_entries(vec![(4, [0x05u8; MASTER_KEY_LEN])])
            .expect("one entry is a valid keyring");

        assert!(matches!(
            other.unwrap_data_key(&wrapped.wrapped, &wrapped.nonce, wrapped.version),
            Err(SecretsError::Decrypt)
        ));
    }

    #[test]
    fn a_nonce_of_the_wrong_width_is_a_decrypt_failure() {
        let keyring = SecretsKeyring::from_entries(vec![(1, [0x01u8; MASTER_KEY_LEN])])
            .expect("one entry is a valid keyring");
        let wrapped = keyring
            .wrap_data_key(&[0x42u8; DATA_KEY_LEN])
            .expect("the wrap succeeds");

        assert!(matches!(
            keyring.unwrap_data_key(&wrapped.wrapped, &wrapped.nonce[..8], wrapped.version),
            Err(SecretsError::Decrypt)
        ));
    }

    #[test]
    fn a_wrapped_key_debug_shows_the_version_only() {
        let keyring = SecretsKeyring::from_entries(vec![(3, [0x01u8; MASTER_KEY_LEN])])
            .expect("one entry is a valid keyring");
        let wrapped = keyring
            .wrap_data_key(&[0x42u8; DATA_KEY_LEN])
            .expect("the wrap succeeds");

        let rendered = format!("{wrapped:?}");

        assert!(rendered.contains("version: 3"), "{rendered}");
        assert!(rendered.contains(r#"wrapped: "<redacted>""#), "{rendered}");
        assert!(rendered.contains(r#"nonce: "<redacted>""#), "{rendered}");
        // Neither byte buffer is rendered, so no byte list appears at all.
        assert!(!rendered.contains('['), "{rendered}");
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
