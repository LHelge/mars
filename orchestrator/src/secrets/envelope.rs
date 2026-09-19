//! A secret's encrypted identity: one type that owns the additional
//! authenticated data and the wrapped data key (`ARCHITECTURE.md`, "Secrets";
//! ADR 0006).
//!
//! Each row carries its own data key. The value is AES-256-GCM encrypted under
//! that key with the row's identity as additional authenticated data, and the
//! data key is wrapped by the master keyring. [`SealedSecret`] is that pair —
//! a [`SecretIdentity`] and the five encrypted columns that belong to it — and
//! the four things anybody does with a secret's ciphertext are its methods:
//!
//! - [`SealedSecret::seal`] encrypts a value under a fresh data key bound to an
//!   identity,
//! - [`SealedSecret::open`] decrypts it again,
//! - [`SealedSecret::reseal`] moves it to a new identity, which is what a
//!   **rename** is: the identity changes, so the value is decrypted and
//!   re-encrypted under the new one while the data key and its wrapping are
//!   untouched (`docs/data-model.md`, `secrets`),
//! - [`SealedSecret::rewrap`] re-wraps the data key under the newest master
//!   key, which is what **rotation** is: the ciphertext is untouched
//!   (`ARCHITECTURE.md`, "Secrets", Rotation).
//!
//! [`SecretIdentity::aad`] is the one place the
//! `<scope>:<scope_id or empty>:<name>` format is written, and it is private:
//! a ciphertext moved to another row fails to decrypt only as long as every
//! caller derives the AAD the same way, so no other module — and no other
//! function here — formats that string or is handed one. The service, the
//! launch resolver, the git credential helpers and the rotation sweep all go
//! through the methods above and never see it.
//!
//! Plaintext and key material live in `Zeroizing` buffers and leave no copy
//! behind; nothing here logs, formats or `Debug`-prints either
//! (`ARCHITECTURE.md`, "Secrets", Credential handling and transcripts; rule 3).

use aes_gcm::aead::{Aead, KeyInit, Nonce, Payload};
use aes_gcm::{Aes256Gcm, Key};
use rand::Rng;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::keyring::{DATA_KEY_LEN, SecretsError, SecretsKeyring, WRAP_NONCE_LEN, WrappedKey};
use crate::models::{ScopeRef, Secret, SecretName, SecretScope};
#[allow(unused_imports)]
use crate::prelude::*;

/// The nonce width of a value's own encryption, and of `secrets.nonce`
/// (`docs/data-model.md`, `secrets`).
///
/// The same 12 bytes AES-GCM is specified for everywhere, so it is
/// [`WRAP_NONCE_LEN`]'s value under the name of the other column; both are
/// spelled out so a caller binding a column does not have to know that the two
/// happen to agree.
pub const VALUE_NONCE_LEN: usize = WRAP_NONCE_LEN;

/// Which row a ciphertext belongs to: the three columns the additional
/// authenticated data is built from.
///
/// `global::DEPLOY_TOKEN`, `project:<uuid>:DEPLOY_TOKEN`,
/// `user:<uuid>:DEPLOY_TOKEN` — the scope spelled as the Postgres enum stores
/// it, the id as a lowercase hyphenated UUID and the name exactly as entered,
/// which is what binds a ciphertext to its row (`docs/data-model.md`,
/// `secrets`; ADR 0006).
///
/// The parts are stored loose rather than as a [`ScopeRef`] and a
/// [`SecretName`] on purpose, and the two constructors are why:
/// [`SecretIdentity::new`] takes the validated pair the write paths hold, while
/// [`SecretIdentity::of`] takes a stored row *verbatim*. A row is bound to the
/// bytes it actually carries, so re-validating them on the way to the cipher
/// could only turn a readable row into an unreadable one — and a rotation
/// sweep, which never decrypts anything, would then fail on a row it is only
/// re-wrapping.
///
/// `Debug` is derived: an identity is the naming half of a row and is what a
/// log line is allowed to carry (rule 3).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretIdentity {
    scope: SecretScope,
    scope_id: Option<Uuid>,
    name: String,
}

impl SecretIdentity {
    /// The identity a create or a rename is about to write.
    pub fn new(scope: &ScopeRef, name: &SecretName) -> Self {
        Self {
            scope: scope.scope(),
            scope_id: scope.scope_id(),
            name: name.as_str().to_string(),
        }
    }

    /// The identity a stored row already has, column for column.
    pub fn of(row: &Secret) -> Self {
        Self {
            scope: row.scope,
            scope_id: row.scope_id,
            name: row.name.clone(),
        }
    }

    /// The same row under a new name: what a rename moves the value to.
    ///
    /// The scope never moves — changing a secret's scope is a delete and a
    /// create (`SPEC.md`, "Secrets") — so the name is the only part of an
    /// identity that a stored row can ever exchange.
    pub fn renamed(&self, name: &SecretName) -> Self {
        Self {
            scope: self.scope,
            scope_id: self.scope_id,
            name: name.as_str().to_string(),
        }
    }

    /// Which population this row belongs to.
    pub fn scope(&self) -> SecretScope {
        self.scope
    }

    /// The user or project id, or `None` for [`SecretScope::Global`].
    pub fn scope_id(&self) -> Option<Uuid> {
        self.scope_id
    }

    /// The name, ready to bind to the `TEXT` column.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The additional authenticated data for this row:
    /// `<scope>:<scope_id or empty>:<name>`.
    ///
    /// Private, and the only place that string exists. Nothing hands an AAD in
    /// or out: the cipher calls below build it from the identity they were
    /// given, so two call sites cannot disagree about its shape.
    fn aad(&self) -> String {
        match self.scope_id {
            Some(id) => format!("{}:{id}:{}", self.scope.as_str(), self.name),
            None => format!("{}::{}", self.scope.as_str(), self.name),
        }
    }
}

/// One row's encrypted half: the identity it is bound to, the value's
/// ciphertext and nonce, and the wrapped data key.
///
/// The five byte-and-version fields are exactly the five columns that store
/// them (`docs/data-model.md`, `secrets`), and they always move together:
/// replacing a value re-encrypts it under a new data key, renaming
/// re-encrypts it under a new identity, and rotating re-wraps the data key.
/// Carrying them as one value with the identity they belong to is what keeps a
/// caller from writing a ciphertext beside the wrong nonce, or beside a name
/// nothing could decrypt it under.
///
/// `Debug` shows the identity and the key version and redacts the rest: the
/// identity is what a log line needs and nothing else here is safe to print
/// (rule 3). There is no `Serialize`: no response ever carries a value
/// (`SPEC.md`, "Secrets").
#[derive(Clone, PartialEq, Eq)]
pub struct SealedSecret {
    /// The row this ciphertext is bound to.
    pub identity: SecretIdentity,
    /// AES-256-GCM ciphertext of the value, including the 16-byte tag.
    pub ciphertext: Vec<u8>,
    /// 12-byte nonce for `ciphertext`.
    pub nonce: Vec<u8>,
    /// The per-row data key as the master keyring wrapped it.
    pub wrapped: WrappedKey,
}

impl std::fmt::Debug for SealedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedSecret")
            .field("identity", &self.identity)
            .field("ciphertext", &"<redacted>")
            .field("nonce", &"<redacted>")
            .field("wrapped", &self.wrapped)
            .finish()
    }
}

impl SealedSecret {
    /// Encrypt `plaintext` under a fresh data key bound to `identity`.
    ///
    /// A new 32-byte data key and a new 12-byte nonce per call, and the data
    /// key wrapped under the keyring's highest version: the result is the five
    /// encrypted columns of a row, ready to bind into an `INSERT` or an
    /// `UPDATE`.
    ///
    /// An empty `plaintext` is legal here — AES-GCM produces the bare tag for
    /// it and it round-trips — because rejecting empty values is the model's
    /// rule and is stated once, in
    /// [`validate_secret_value`](crate::models::validate_secret_value).
    pub fn seal(
        keyring: &SecretsKeyring,
        identity: SecretIdentity,
        plaintext: &[u8],
    ) -> std::result::Result<Self, SecretsError> {
        let mut data_key = Zeroizing::new([0u8; DATA_KEY_LEN]);
        rand::rng().fill_bytes(data_key.as_mut_slice());

        let mut nonce = [0u8; VALUE_NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce);

        let ciphertext = encrypt(&data_key, &nonce, &identity.aad(), plaintext)?;
        let wrapped = keyring.wrap_data_key(&data_key)?;

        Ok(Self {
            identity,
            ciphertext,
            nonce: nonce.to_vec(),
            wrapped,
        })
    }

    /// Decrypt this value under the identity it is bound to.
    ///
    /// [`SecretsError::UnknownKeyVersion`] when the environment no longer
    /// carries the master key this row was wrapped with, and
    /// [`SecretsError::Decrypt`] for everything else — a tampered ciphertext, a
    /// nonce of the wrong width, or an identity that is not the one the value
    /// was sealed under. The distinction is kept rather than flattened because
    /// it is the difference between "the operator dropped a key" and "this row
    /// is not what it claims to be", and an operator reading the log line for a
    /// failed launch needs the first to say so. It is not an oracle: every
    /// caller answers the request with the same generic internal message and
    /// logs the detail (`ARCHITECTURE.md`, "Secrets", Keyring).
    pub fn open(
        &self,
        keyring: &SecretsKeyring,
    ) -> std::result::Result<Zeroizing<Vec<u8>>, SecretsError> {
        let data_key = self.data_key(keyring)?;

        decrypt(
            &data_key,
            &self.nonce,
            &self.identity.aad(),
            &self.ciphertext,
        )
    }

    /// Re-encrypt this value under a new identity, keeping its data key.
    ///
    /// What a rename needs: the row's identity changes, so the ciphertext has
    /// to be bound to the new one, but the data key and its wrapping are
    /// unaffected (`docs/data-model.md`, `secrets`). The wrapped key comes
    /// across unchanged and only `ciphertext` and `nonce` move, so the caller
    /// updates the row in one statement.
    ///
    /// The nonce is always fresh: re-encrypting under the same data key with
    /// the old nonce would be the one thing AES-GCM does not survive.
    ///
    /// Consumes `self`, because after a rename the old identity is not a thing
    /// that exists any more: a caller holding both would be holding one row in
    /// two contradictory shapes.
    pub fn reseal(
        self,
        keyring: &SecretsKeyring,
        identity: SecretIdentity,
    ) -> std::result::Result<Self, SecretsError> {
        let data_key = self.data_key(keyring)?;
        let plaintext = decrypt(
            &data_key,
            &self.nonce,
            &self.identity.aad(),
            &self.ciphertext,
        )?;

        let mut nonce = [0u8; VALUE_NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce);
        let ciphertext = encrypt(&data_key, &nonce, &identity.aad(), &plaintext)?;

        Ok(Self {
            identity,
            ciphertext,
            nonce: nonce.to_vec(),
            wrapped: self.wrapped,
        })
    }

    /// Re-wrap the data key under the newest master key, or `None` when this
    /// row is already on it.
    ///
    /// The rotation step: `ciphertext` and `nonce` come across byte-identical
    /// and only the wrapping moves, so no value is ever decrypted to move a row
    /// onto a new master key (`ARCHITECTURE.md`, "Secrets", Rotation). `None`
    /// is a row the sweep has nothing to do to, which it counts as skipped.
    ///
    /// Unlike [`SealedSecret::open`] this never decrypts the value, so the only
    /// failures are the keyring's own; [`SecretsError::UnknownKeyVersion`]
    /// naming the key an operator has to put back is the whole point of the
    /// error, and the sweep treats it as a skip rather than an abort.
    pub fn rewrap(
        &self,
        keyring: &SecretsKeyring,
    ) -> std::result::Result<Option<Self>, SecretsError> {
        // Compared in `u32`, the keyring's own numbering: a negative or
        // otherwise impossible stored version is simply not the current one,
        // and falls through to the unwrap that reports it.
        if u32::try_from(self.wrapped.version)
            .is_ok_and(|version| version == keyring.current_version())
        {
            return Ok(None);
        }

        let data_key = self.data_key(keyring)?;
        let wrapped = keyring.wrap_data_key(&data_key)?;

        Ok(Some(Self {
            wrapped,
            ..self.clone()
        }))
    }

    /// Check that the configured keyring can still unwrap this row's data key.
    ///
    /// The question the startup check asks of one sampled row per stored
    /// `key_version` ([`verify_keyring_at_startup`](crate::secrets::verify_keyring_at_startup);
    /// `ARCHITECTURE.md`, "Secrets", Keyring): it is about the wrapping alone,
    /// so the value is never decrypted and the data key is dropped —
    /// `Zeroizing` wipes it — the moment it has been shown to exist. The
    /// errors are the keyring's own, so
    /// [`SecretsError::UnknownKeyVersion`] still distinguishes a key the
    /// operator did not configure from one that is configured but wrong.
    pub fn verify_unwrappable(
        &self,
        keyring: &SecretsKeyring,
    ) -> std::result::Result<(), SecretsError> {
        self.data_key(keyring).map(|_| ())
    }

    /// Unwrap this row's data key.
    ///
    /// Private: a data key is never handed to a caller outside this module, and
    /// the three wrapping columns are always read together.
    fn data_key(
        &self,
        keyring: &SecretsKeyring,
    ) -> std::result::Result<Zeroizing<[u8; DATA_KEY_LEN]>, SecretsError> {
        keyring.unwrap_data_key(
            &self.wrapped.wrapped,
            &self.wrapped.nonce,
            self.wrapped.version,
        )
    }
}

impl Secret {
    /// The encrypted half of this row, bound to the identity the row has.
    ///
    /// The one conversion from a stored row to something that can be opened,
    /// resealed or re-wrapped: the resolver, the git credential provider, the
    /// secrets service and the rotation sweep all start here rather than each
    /// gathering five columns and an identity of their own.
    ///
    /// Defined beside [`SealedSecret`] rather than in `models/secret.rs`, so
    /// that the model still holds no cryptography at all: it is the envelope
    /// that knows which columns belong to it.
    pub fn sealed(&self) -> SealedSecret {
        SealedSecret {
            identity: SecretIdentity::of(self),
            ciphertext: self.ciphertext.clone(),
            nonce: self.nonce.clone(),
            wrapped: WrappedKey {
                wrapped: self.data_key_wrapped.clone(),
                nonce: self.data_key_nonce.clone(),
                version: self.key_version,
            },
        }
    }
}

/// The AES-256-GCM cipher for one data key.
fn cipher_for(data_key: &[u8; DATA_KEY_LEN]) -> Aes256Gcm {
    Aes256Gcm::new(&Key::<Aes256Gcm>::from(*data_key))
}

/// Encrypt under a data key with the row AAD, tag appended.
///
/// The only failure AES-GCM has here is a message beyond its length limit
/// (tens of gigabytes), which no path reaches; it is reported as
/// [`SecretsError::Wrap`] rather than `expect`-ed, so the sealing path has no
/// panic in it.
fn encrypt(
    data_key: &[u8; DATA_KEY_LEN],
    nonce: &[u8; VALUE_NONCE_LEN],
    aad: &str,
    plaintext: &[u8],
) -> std::result::Result<Vec<u8>, SecretsError> {
    cipher_for(data_key)
        .encrypt(
            &Nonce::<Aes256Gcm>::from(*nonce),
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| SecretsError::Wrap)
}

/// Decrypt under a data key with the row AAD.
///
/// The nonce arrives as the column's bytes, so its width is checked rather
/// than assumed: a row with a nonce of the wrong length is a corrupt row and
/// reads exactly like a failed tag, never a panic. A ciphertext shorter than
/// the 16-byte tag fails inside the cipher the same way.
fn decrypt(
    data_key: &[u8; DATA_KEY_LEN],
    nonce: &[u8],
    aad: &str,
    ciphertext: &[u8],
) -> std::result::Result<Zeroizing<Vec<u8>>, SecretsError> {
    let nonce: [u8; VALUE_NONCE_LEN] = nonce.try_into().map_err(|_| SecretsError::Decrypt)?;

    let plaintext = cipher_for(data_key)
        .decrypt(
            &Nonce::<Aes256Gcm>::from(nonce),
            Payload {
                msg: ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| SecretsError::Decrypt)?;

    Ok(Zeroizing::new(plaintext))
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    use super::*;
    use crate::secrets::keyring::MASTER_KEY_LEN;

    /// Two versions, both obviously fake: 32 bytes of one repeated value
    /// (rule 3). Version 2 is the current one, so a row on version 1 is what a
    /// rotation sweep would pick up.
    fn two_version_keyring() -> SecretsKeyring {
        SecretsKeyring::from_entries(vec![
            (1, [0x11; MASTER_KEY_LEN]),
            (2, [0x22; MASTER_KEY_LEN]),
        ])
        .expect("two distinct fake versions are a valid keyring")
    }

    /// A keyring holding version 1 only, so a row on version 2 has no key.
    fn version_one_keyring() -> SecretsKeyring {
        SecretsKeyring::from_entries(vec![(1, [0x11; MASTER_KEY_LEN])])
            .expect("one fake version is a valid keyring")
    }

    fn a_uuid() -> Uuid {
        Uuid::parse_str("11111111-2222-3333-4444-555555555555").expect("a literal UUID parses")
    }

    fn name(raw: &str) -> SecretName {
        SecretName::parse(raw).expect("a valid name")
    }

    /// An identity from the validated pair every write path holds.
    fn identity(scope: ScopeRef, raw: &str) -> SecretIdentity {
        SecretIdentity::new(&scope, &name(raw))
    }

    #[test]
    fn the_aad_has_one_form_per_scope() {
        let id = a_uuid();

        assert_eq!(
            identity(ScopeRef::global(), "DEPLOY_TOKEN").aad(),
            "global::DEPLOY_TOKEN"
        );
        assert_eq!(
            identity(ScopeRef::project(id), "DEPLOY_TOKEN").aad(),
            "project:11111111-2222-3333-4444-555555555555:DEPLOY_TOKEN"
        );
        assert_eq!(
            identity(ScopeRef::user(id), "DEPLOY_TOKEN").aad(),
            "user:11111111-2222-3333-4444-555555555555:DEPLOY_TOKEN"
        );
    }

    #[test]
    fn a_renamed_identity_keeps_the_scope_and_nothing_else() {
        let id = a_uuid();
        let before = identity(ScopeRef::project(id), "OLD_NAME");
        let after = before.renamed(&name("NEW_NAME"));

        assert_eq!(after.scope(), SecretScope::Project);
        assert_eq!(after.scope_id(), Some(id));
        assert_eq!(after.name(), "NEW_NAME");
        assert_ne!(after.aad(), before.aad());
    }

    #[test]
    fn a_sealed_value_opens_again() {
        let keyring = two_version_keyring();
        let identity = identity(ScopeRef::global(), "DEPLOY_TOKEN");

        let sealed =
            SealedSecret::seal(&keyring, identity, b"not-a-real-token").expect("sealing succeeds");

        assert_eq!(sealed.nonce.len(), VALUE_NONCE_LEN);
        assert_eq!(sealed.wrapped.nonce.len(), WRAP_NONCE_LEN);
        assert_eq!(sealed.wrapped.wrapped.len(), DATA_KEY_LEN + 16);
        assert_eq!(
            sealed.wrapped.version, 2,
            "new rows use the highest version"
        );
        assert!(
            !sealed.ciphertext.windows(4).any(|window| window == b"not-"),
            "the plaintext must not be recognisable in the ciphertext"
        );

        let opened = sealed
            .open(&keyring)
            .expect("opening under the same identity succeeds");
        assert_eq!(opened.as_slice(), b"not-a-real-token");
    }

    #[test]
    fn two_seals_of_one_value_share_no_material() {
        let keyring = two_version_keyring();

        let first = SealedSecret::seal(
            &keyring,
            identity(ScopeRef::global(), "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");
        let second = SealedSecret::seal(
            &keyring,
            identity(ScopeRef::global(), "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");

        assert_ne!(first.nonce, second.nonce, "every seal draws a fresh nonce");
        assert_ne!(first.ciphertext, second.ciphertext);
        assert_ne!(
            first.wrapped.wrapped, second.wrapped.wrapped,
            "every seal draws a fresh data key"
        );
    }

    #[test]
    fn an_empty_value_round_trips() {
        // The model rejects an empty value before it ever reaches a cipher
        // (`validate_secret_value`); the envelope has no opinion about it.
        let keyring = two_version_keyring();

        let sealed = SealedSecret::seal(&keyring, identity(ScopeRef::global(), "EMPTY"), b"")
            .expect("an empty plaintext is legal here");
        assert_eq!(sealed.ciphertext.len(), 16, "the bare tag");

        let opened = sealed.open(&keyring).expect("an empty value opens");
        assert!(opened.is_empty());
    }

    #[test]
    fn a_large_value_round_trips() {
        let keyring = two_version_keyring();
        let plaintext: Vec<u8> = (0..64 * 1024).map(|index| (index % 251) as u8).collect();

        let sealed = SealedSecret::seal(&keyring, identity(ScopeRef::global(), "BIG"), &plaintext)
            .expect("64 KiB seals");
        let opened = sealed.open(&keyring).expect("64 KiB opens");

        assert_eq!(opened.as_slice(), plaintext.as_slice());
    }

    #[test]
    fn opening_under_another_rows_identity_fails() {
        let keyring = two_version_keyring();
        let id = a_uuid();
        let other =
            Uuid::parse_str("99999999-8888-7777-6666-555555555555").expect("a literal UUID");
        let sealed = SealedSecret::seal(
            &keyring,
            identity(ScopeRef::project(id), "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");

        for wrong in [
            identity(ScopeRef::project(id), "OTHER_TOKEN"),
            identity(ScopeRef::user(id), "DEPLOY_TOKEN"),
            identity(ScopeRef::project(other), "DEPLOY_TOKEN"),
            identity(ScopeRef::global(), "DEPLOY_TOKEN"),
        ] {
            let moved = SealedSecret {
                identity: wrong.clone(),
                ..sealed.clone()
            };
            assert!(
                matches!(moved.open(&keyring), Err(SecretsError::Decrypt)),
                "a ciphertext must not open as {wrong:?}"
            );
        }
    }

    #[test]
    fn a_tampered_ciphertext_fails() {
        let keyring = two_version_keyring();
        let sealed = SealedSecret::seal(
            &keyring,
            identity(ScopeRef::global(), "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");

        let mut flipped = sealed.clone();
        flipped.ciphertext[0] ^= 0x01;
        assert!(matches!(flipped.open(&keyring), Err(SecretsError::Decrypt)));

        let mut flipped = sealed.clone();
        flipped.nonce[0] ^= 0x01;
        assert!(matches!(flipped.open(&keyring), Err(SecretsError::Decrypt)));

        let mut flipped = sealed;
        flipped.wrapped.wrapped[0] ^= 0x01;
        assert!(matches!(flipped.open(&keyring), Err(SecretsError::Decrypt)));
    }

    #[test]
    fn a_row_of_the_wrong_shape_fails_without_panicking() {
        let keyring = two_version_keyring();
        let sealed = SealedSecret::seal(
            &keyring,
            identity(ScopeRef::global(), "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");

        // Tag-length and shorter ciphertexts, a truncated and an overlong
        // nonce, and a wrapped key that is not one.
        let mut short = sealed.clone();
        short.ciphertext = vec![0u8; 16];
        assert!(matches!(short.open(&keyring), Err(SecretsError::Decrypt)));

        let mut empty = sealed.clone();
        empty.ciphertext = Vec::new();
        assert!(matches!(empty.open(&keyring), Err(SecretsError::Decrypt)));

        let mut truncated = sealed.clone();
        truncated.nonce.truncate(VALUE_NONCE_LEN - 1);
        assert!(matches!(
            truncated.open(&keyring),
            Err(SecretsError::Decrypt)
        ));

        let mut long = sealed.clone();
        long.wrapped.nonce.push(0x00);
        assert!(matches!(long.open(&keyring), Err(SecretsError::Decrypt)));

        let mut stub = sealed;
        stub.wrapped.wrapped = vec![0u8; 3];
        assert!(matches!(stub.open(&keyring), Err(SecretsError::Decrypt)));
    }

    #[test]
    fn an_unconfigured_key_version_surfaces_as_itself() {
        let sealed = SealedSecret::seal(
            &two_version_keyring(),
            identity(ScopeRef::global(), "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");
        assert_eq!(sealed.wrapped.version, 2);

        // The operator dropped version 2 from the environment: the row is not
        // corrupt and its identity is right, and the error says which key to
        // put back rather than pretending the ciphertext is wrong.
        assert!(
            matches!(
                sealed.open(&version_one_keyring()),
                Err(SecretsError::UnknownKeyVersion(2))
            ),
            "a missing master key is not a decrypt failure"
        );
    }

    #[test]
    fn resealing_moves_a_value_to_a_new_identity() {
        let keyring = two_version_keyring();
        let id = a_uuid();
        let old = identity(ScopeRef::project(id), "OLD_NAME");
        let new = old.renamed(&name("NEW_NAME"));

        let sealed = SealedSecret::seal(&keyring, old.clone(), b"not-a-real-token")
            .expect("sealing succeeds");
        let resealed = sealed
            .clone()
            .reseal(&keyring, new)
            .expect("resealing succeeds");

        assert_eq!(
            resealed
                .open(&keyring)
                .expect("the value opens under the new identity")
                .as_slice(),
            b"not-a-real-token"
        );

        // The old identity over the new ciphertext is what a caller that kept
        // the stale name would produce, and it must not open.
        let stale = SealedSecret {
            identity: old,
            ..resealed.clone()
        };
        assert!(
            matches!(stale.open(&keyring), Err(SecretsError::Decrypt)),
            "the old identity must no longer open the row"
        );

        assert_eq!(
            resealed.wrapped, sealed.wrapped,
            "a rename does not change the data key or its wrapping"
        );
        assert_ne!(resealed.nonce, sealed.nonce, "with a fresh value nonce");
        assert_ne!(resealed.ciphertext, sealed.ciphertext);
    }

    #[test]
    fn resealing_a_row_bound_to_another_identity_fails() {
        let keyring = two_version_keyring();
        let sealed = SealedSecret::seal(
            &keyring,
            identity(ScopeRef::global(), "OLD_NAME"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");

        // A row whose identity has already been changed under it: the
        // ciphertext no longer matches what it claims to be.
        let mistaken = SealedSecret {
            identity: identity(ScopeRef::global(), "NEW_NAME"),
            ..sealed
        };

        assert!(matches!(
            mistaken.reseal(&keyring, identity(ScopeRef::global(), "NEWER_NAME")),
            Err(SecretsError::Decrypt)
        ));
    }

    #[test]
    fn rewrapping_moves_the_data_key_and_leaves_the_ciphertext() {
        let old_only = version_one_keyring();
        let sealed = SealedSecret::seal(
            &old_only,
            identity(ScopeRef::global(), "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");
        assert_eq!(sealed.wrapped.version, 1);

        let keyring = two_version_keyring();
        let rewrapped = sealed
            .rewrap(&keyring)
            .expect("rewrapping succeeds")
            .expect("a row behind the newest key moves");

        assert_eq!(rewrapped.wrapped.version, 2);
        assert_eq!(
            rewrapped.ciphertext, sealed.ciphertext,
            "rotation never touches a ciphertext"
        );
        assert_eq!(rewrapped.nonce, sealed.nonce);
        assert_eq!(rewrapped.identity, sealed.identity);
        assert_ne!(rewrapped.wrapped.wrapped, sealed.wrapped.wrapped);
        assert_ne!(rewrapped.wrapped.nonce, sealed.wrapped.nonce);

        assert_eq!(
            rewrapped
                .open(&keyring)
                .expect("the rewrapped row still opens")
                .as_slice(),
            b"not-a-real-token"
        );
    }

    #[test]
    fn rewrapping_a_current_row_is_nothing_to_do() {
        let keyring = two_version_keyring();
        let sealed = SealedSecret::seal(
            &keyring,
            identity(ScopeRef::global(), "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");

        assert!(
            sealed
                .rewrap(&keyring)
                .expect("a current row is not a failure")
                .is_none(),
            "a row already on the newest key is a skip, not a write"
        );
    }

    #[test]
    fn rewrapping_a_row_whose_version_is_gone_names_the_version() {
        let old_only = version_one_keyring();
        let sealed = SealedSecret::seal(
            &old_only,
            identity(ScopeRef::global(), "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");

        // A keyring that has moved on without keeping version 1: rotation is
        // the operator's own operation, so it is told which key is missing.
        let newer = SecretsKeyring::from_entries(vec![(2, [0x22; MASTER_KEY_LEN])])
            .expect("one fake version is a valid keyring");

        assert!(matches!(
            sealed.rewrap(&newer),
            Err(SecretsError::UnknownKeyVersion(1))
        ));
    }

    #[test]
    fn a_sealed_secret_debug_carries_the_identity_and_no_material() {
        let keyring = two_version_keyring();
        let sealed = SealedSecret::seal(
            &keyring,
            identity(ScopeRef::global(), "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");

        let rendered = format!("{sealed:?}");

        assert!(rendered.contains("DEPLOY_TOKEN"), "{rendered}");
        assert!(rendered.contains("version: 2"), "{rendered}");
        assert!(
            rendered.contains(r#"ciphertext: "<redacted>""#),
            "{rendered}"
        );
        assert!(rendered.contains(r#"nonce: "<redacted>""#), "{rendered}");
    }

    /// A row sealed before the sealed-envelope type existed still opens.
    ///
    /// The vector below was produced by the previous `crypto::seal` and is the
    /// on-disk format this refactor must not change: the same AAD bytes, the
    /// same 12-byte nonces and the same wrapped-key layout. The master key, the
    /// data key and the "value" are all obviously fake (rule 3) and no value is
    /// logged anywhere.
    #[test]
    fn an_envelope_sealed_by_the_previous_implementation_still_opens() {
        // 32 bytes of 0x11, the same fake version-1 master key as above.
        let keyring = version_one_keyring();

        let sealed = SealedSecret {
            identity: identity(ScopeRef::global(), "DEPLOY_TOKEN"),
            ciphertext: STANDARD
                .decode(KNOWN_CIPHERTEXT)
                .expect("a literal base64 vector decodes"),
            nonce: STANDARD
                .decode(KNOWN_VALUE_NONCE)
                .expect("a literal base64 vector decodes"),
            wrapped: WrappedKey {
                wrapped: STANDARD
                    .decode(KNOWN_WRAPPED_DATA_KEY)
                    .expect("a literal base64 vector decodes"),
                nonce: STANDARD
                    .decode(KNOWN_WRAP_NONCE)
                    .expect("a literal base64 vector decodes"),
                version: 1,
            },
        };

        let opened = sealed
            .open(&keyring)
            .expect("a row sealed by the previous implementation still opens");
        assert_eq!(opened.as_slice(), b"not-a-real-token");
    }

    /// `not-a-real-token` sealed under `global::DEPLOY_TOKEN`, the fake
    /// version-1 master key and a fake data key, by the implementation that
    /// preceded [`SealedSecret`]. Never regenerate these: a change that makes
    /// them fail is a change that makes every stored row unreadable.
    const KNOWN_CIPHERTEXT: &str = "2V9gM9S5oKtoClob3GjLYP51m3yu/WUVTJnsITxhirE=";
    const KNOWN_VALUE_NONCE: &str = "3eFiG/9ep5kzis/9";
    const KNOWN_WRAPPED_DATA_KEY: &str =
        "NHB6CCpZmLLYbwrVJd1jCEMKNQdFMT95vQCJkFsxvAqMtVmk6YbOiNhX9x5ozUvl";
    const KNOWN_WRAP_NONCE: &str = "p6uNXoUDf1oPEbwu";
}
