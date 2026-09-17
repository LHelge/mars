//! Value-level envelope crypto: [`seal`], [`open`], [`reseal`] and [`rewrap`]
//! over the columns of one `secrets` row (`ARCHITECTURE.md`, "Secrets";
//! ADR 0006).
//!
//! Each row carries its own data key. The value is AES-256-GCM encrypted under
//! that key with the row's identity as additional authenticated data, and the
//! data key is wrapped by the master keyring. That split is what makes the two
//! maintenance operations cheap and independent:
//!
//! - **Renaming** a secret changes the AAD, so the value is decrypted and
//!   re-encrypted under the new one ([`reseal`]); the data key and its
//!   wrapping are untouched (`docs/data-model.md`, `secrets`).
//! - **Rotating** the master key re-wraps the data key ([`rewrap`]); the
//!   ciphertext is untouched (`ARCHITECTURE.md`, "Secrets", Rotation).
//!
//! [`aad`] is the one place the `<scope>:<scope_id or empty>:<name>` format is
//! written. A ciphertext moved to another row therefore fails to decrypt, but
//! only as long as every caller builds the AAD the same way, which is why no
//! other module formats that string itself.
//!
//! Plaintext and key material live in `Zeroizing` buffers and leave no copy
//! behind; nothing here logs, formats or `Debug`-prints either
//! (`ARCHITECTURE.md`, "Secrets", Credential handling and transcripts; rule 3).

use aes_gcm::aead::{Aead, KeyInit, Nonce, Payload};
use aes_gcm::{Aes256Gcm, Key};
use rand::Rng;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::keyring::{DATA_KEY_LEN, SecretsError, SecretsKeyring, WRAP_NONCE_LEN};
use crate::models::{EncryptedValue, ScopeRef, SecretName, SecretScope};
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

/// The additional authenticated data for one row:
/// `<scope>:<scope_id or empty>:<name>`.
///
/// `global::DEPLOY_TOKEN`, `project:<uuid>:DEPLOY_TOKEN`,
/// `user:<uuid>:DEPLOY_TOKEN` — the scope spelled as the Postgres enum stores
/// it, the id as a lowercase hyphenated UUID and the name exactly as entered,
/// which is what binds a ciphertext to its row (`docs/data-model.md`,
/// `secrets`; ADR 0006).
///
/// Taking the three parts loose rather than a [`ScopeRef`] is deliberate: a
/// `Secret` coming out of the database has them as three columns, and a
/// rotation sweep that had to rebuild a validated pair from a row just to
/// decrypt it could fail on a row it is only re-wrapping. [`aad_for`] is the
/// shorter form for the write paths, which do hold a [`ScopeRef`].
pub fn aad(scope: SecretScope, scope_id: Option<Uuid>, name: &str) -> String {
    match scope_id {
        Some(id) => format!("{}:{id}:{name}", scope.as_str()),
        None => format!("{}::{name}", scope.as_str()),
    }
}

/// [`aad`] for a validated scope and name, as the create and rename paths hold
/// them.
pub fn aad_for(scope: &ScopeRef, name: &SecretName) -> String {
    aad(scope.scope(), scope.scope_id(), name.as_str())
}

/// Encrypt `plaintext` under a fresh data key bound to `aad`.
///
/// A new 32-byte data key and a new 12-byte nonce per call, and the data key
/// wrapped under the keyring's highest version: the five fields of the result
/// are the five columns that store them, ready to bind into an `INSERT`.
///
/// An empty `plaintext` is legal here — AES-GCM produces the bare tag for it
/// and it round-trips — because rejecting empty values belongs to the API
/// layer, not to the cipher.
pub fn seal(
    keyring: &SecretsKeyring,
    aad: &str,
    plaintext: &[u8],
) -> std::result::Result<EncryptedValue, SecretsError> {
    let mut data_key = Zeroizing::new([0u8; DATA_KEY_LEN]);
    rand::rng().fill_bytes(data_key.as_mut_slice());

    let mut nonce = [0u8; VALUE_NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);

    let ciphertext = encrypt(&data_key, &nonce, aad, plaintext)?;
    let wrapped = keyring.wrap_data_key(&data_key)?;

    Ok(EncryptedValue {
        ciphertext,
        nonce: nonce.to_vec(),
        data_key_wrapped: wrapped.wrapped,
        data_key_nonce: wrapped.nonce.to_vec(),
        key_version: wrapped.version,
    })
}

/// Decrypt a stored value, given the same `aad` it was sealed under.
///
/// Every failure is [`SecretsError::Decrypt`] — a wrong AAD, a tampered
/// ciphertext, a nonce of the wrong width, a key version the environment no
/// longer carries. The caller learns that the row cannot be read and nothing
/// about which step failed, because the difference between "the tag did not
/// verify" and "that AAD is wrong" is exactly what an attacker probing with
/// guessed row identities would want. The startup check
/// ([`SecretsKeyring::verify_against_db`]) is where a missing key version is
/// reported as itself, to the operator rather than to a request.
pub fn open(
    keyring: &SecretsKeyring,
    aad: &str,
    value: &EncryptedValue,
) -> std::result::Result<Zeroizing<Vec<u8>>, SecretsError> {
    let data_key = data_key_of(keyring, value).map_err(|_| SecretsError::Decrypt)?;

    decrypt(&data_key, &value.nonce, aad, &value.ciphertext)
}

/// Re-encrypt a value under a new AAD, keeping its data key.
///
/// What a rename needs: the row's identity changes, so the ciphertext has to
/// be bound to the new one, but the data key and its wrapping are unaffected
/// (`docs/data-model.md`, `secrets`). `data_key_wrapped`, `data_key_nonce` and
/// `key_version` come back unchanged and only `ciphertext` and `nonce` move,
/// so the caller updates the row in one statement.
///
/// The nonce is always fresh: re-encrypting under the same data key with the
/// old nonce would be the one thing AES-GCM does not survive.
pub fn reseal(
    keyring: &SecretsKeyring,
    old_aad: &str,
    new_aad: &str,
    value: &EncryptedValue,
) -> std::result::Result<EncryptedValue, SecretsError> {
    let data_key = data_key_of(keyring, value).map_err(|_| SecretsError::Decrypt)?;
    let plaintext = decrypt(&data_key, &value.nonce, old_aad, &value.ciphertext)?;

    let mut nonce = [0u8; VALUE_NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let ciphertext = encrypt(&data_key, &nonce, new_aad, &plaintext)?;

    Ok(EncryptedValue {
        ciphertext,
        nonce: nonce.to_vec(),
        ..value.clone()
    })
}

/// Re-wrap a row's data key under the newest master key.
///
/// The rotation step: `ciphertext` and `nonce` come back byte-identical and
/// only the three wrapping fields move, so no value is ever decrypted to move
/// a row onto a new master key (`ARCHITECTURE.md`, "Secrets", Rotation). A row
/// already on the newest version is returned unchanged, without unwrapping, so
/// a sweep can call this on anything it selected.
///
/// Unlike [`open`], a failure keeps its shape: rotation is an operator's
/// operation, and [`SecretsError::UnknownKeyVersion`] telling them which key to
/// put back is the whole point of the error.
pub fn rewrap(
    keyring: &SecretsKeyring,
    value: &EncryptedValue,
) -> std::result::Result<EncryptedValue, SecretsError> {
    // Compared in `u32`, the keyring's own numbering: a negative or otherwise
    // impossible stored version is simply not the current one, and falls
    // through to the unwrap that reports it.
    if u32::try_from(value.key_version).is_ok_and(|version| version == keyring.current_version()) {
        return Ok(value.clone());
    }

    let data_key = data_key_of(keyring, value)?;
    let wrapped = keyring.wrap_data_key(&data_key)?;

    Ok(EncryptedValue {
        data_key_wrapped: wrapped.wrapped,
        data_key_nonce: wrapped.nonce.to_vec(),
        key_version: wrapped.version,
        ..value.clone()
    })
}

/// Unwrap the data key of one row.
///
/// Private: a data key is never handed to a caller outside this module, and
/// the three columns are always read together.
fn data_key_of(
    keyring: &SecretsKeyring,
    value: &EncryptedValue,
) -> std::result::Result<Zeroizing<[u8; DATA_KEY_LEN]>, SecretsError> {
    keyring.unwrap_data_key(
        &value.data_key_wrapped,
        &value.data_key_nonce,
        value.key_version,
    )
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

    #[test]
    fn the_aad_has_one_form_per_scope() {
        let id = a_uuid();

        assert_eq!(
            aad(SecretScope::Global, None, "DEPLOY_TOKEN"),
            "global::DEPLOY_TOKEN"
        );
        assert_eq!(
            aad(SecretScope::Project, Some(id), "DEPLOY_TOKEN"),
            "project:11111111-2222-3333-4444-555555555555:DEPLOY_TOKEN"
        );
        assert_eq!(
            aad(SecretScope::User, Some(id), "DEPLOY_TOKEN"),
            "user:11111111-2222-3333-4444-555555555555:DEPLOY_TOKEN"
        );
    }

    #[test]
    fn aad_for_agrees_with_the_loose_form() {
        let id = a_uuid();
        let name = SecretName::parse("DEPLOY_TOKEN").expect("a valid name");

        assert_eq!(
            aad_for(&ScopeRef::global(), &name),
            aad(SecretScope::Global, None, "DEPLOY_TOKEN")
        );
        assert_eq!(
            aad_for(&ScopeRef::project(id), &name),
            aad(SecretScope::Project, Some(id), "DEPLOY_TOKEN")
        );
        assert_eq!(
            aad_for(&ScopeRef::user(id), &name),
            aad(SecretScope::User, Some(id), "DEPLOY_TOKEN")
        );
    }

    #[test]
    fn a_sealed_value_opens_again() {
        let keyring = two_version_keyring();
        let aad = aad(SecretScope::Global, None, "DEPLOY_TOKEN");

        let sealed = seal(&keyring, &aad, b"not-a-real-token").expect("sealing succeeds");

        assert_eq!(sealed.nonce.len(), VALUE_NONCE_LEN);
        assert_eq!(sealed.data_key_nonce.len(), WRAP_NONCE_LEN);
        assert_eq!(sealed.data_key_wrapped.len(), DATA_KEY_LEN + 16);
        assert_eq!(sealed.key_version, 2, "new rows use the highest version");
        assert!(
            !sealed.ciphertext.windows(4).any(|window| window == b"not-"),
            "the plaintext must not be recognisable in the ciphertext"
        );

        let opened = open(&keyring, &aad, &sealed).expect("opening under the same AAD succeeds");
        assert_eq!(opened.as_slice(), b"not-a-real-token");
    }

    #[test]
    fn two_seals_of_one_value_share_no_material() {
        let keyring = two_version_keyring();
        let aad = aad(SecretScope::Global, None, "DEPLOY_TOKEN");

        let first = seal(&keyring, &aad, b"not-a-real-token").expect("sealing succeeds");
        let second = seal(&keyring, &aad, b"not-a-real-token").expect("sealing succeeds");

        assert_ne!(first.nonce, second.nonce, "every seal draws a fresh nonce");
        assert_ne!(first.ciphertext, second.ciphertext);
        assert_ne!(
            first.data_key_wrapped, second.data_key_wrapped,
            "every seal draws a fresh data key"
        );
    }

    #[test]
    fn an_empty_value_round_trips() {
        let keyring = two_version_keyring();
        let aad = aad(SecretScope::Global, None, "EMPTY");

        let sealed = seal(&keyring, &aad, b"").expect("an empty plaintext is legal here");
        assert_eq!(sealed.ciphertext.len(), 16, "the bare tag");

        let opened = open(&keyring, &aad, &sealed).expect("an empty value opens");
        assert!(opened.is_empty());
    }

    #[test]
    fn a_large_value_round_trips() {
        let keyring = two_version_keyring();
        let aad = aad(SecretScope::Global, None, "BIG");
        let plaintext: Vec<u8> = (0..64 * 1024).map(|index| (index % 251) as u8).collect();

        let sealed = seal(&keyring, &aad, &plaintext).expect("64 KiB seals");
        let opened = open(&keyring, &aad, &sealed).expect("64 KiB opens");

        assert_eq!(opened.as_slice(), plaintext.as_slice());
    }

    #[test]
    fn opening_under_another_rows_identity_fails() {
        let keyring = two_version_keyring();
        let id = a_uuid();
        let other =
            Uuid::parse_str("99999999-8888-7777-6666-555555555555").expect("a literal UUID");
        let sealed = seal(
            &keyring,
            &aad(SecretScope::Project, Some(id), "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");

        for wrong in [
            aad(SecretScope::Project, Some(id), "OTHER_TOKEN"),
            aad(SecretScope::User, Some(id), "DEPLOY_TOKEN"),
            aad(SecretScope::Project, Some(other), "DEPLOY_TOKEN"),
            aad(SecretScope::Global, None, "DEPLOY_TOKEN"),
        ] {
            assert!(
                matches!(open(&keyring, &wrong, &sealed), Err(SecretsError::Decrypt)),
                "a ciphertext must not open under {wrong}"
            );
        }
    }

    #[test]
    fn a_tampered_ciphertext_fails() {
        let keyring = two_version_keyring();
        let aad = aad(SecretScope::Global, None, "DEPLOY_TOKEN");
        let sealed = seal(&keyring, &aad, b"not-a-real-token").expect("sealing succeeds");

        let mut flipped = sealed.clone();
        flipped.ciphertext[0] ^= 0x01;
        assert!(matches!(
            open(&keyring, &aad, &flipped),
            Err(SecretsError::Decrypt)
        ));

        let mut flipped = sealed.clone();
        flipped.nonce[0] ^= 0x01;
        assert!(matches!(
            open(&keyring, &aad, &flipped),
            Err(SecretsError::Decrypt)
        ));

        let mut flipped = sealed;
        flipped.data_key_wrapped[0] ^= 0x01;
        assert!(matches!(
            open(&keyring, &aad, &flipped),
            Err(SecretsError::Decrypt)
        ));
    }

    #[test]
    fn a_row_of_the_wrong_shape_fails_without_panicking() {
        let keyring = two_version_keyring();
        let aad = aad(SecretScope::Global, None, "DEPLOY_TOKEN");
        let sealed = seal(&keyring, &aad, b"not-a-real-token").expect("sealing succeeds");

        // Tag-length and shorter ciphertexts, a truncated and an overlong
        // nonce, and a wrapped key that is not one.
        let mut short = sealed.clone();
        short.ciphertext = vec![0u8; 16];
        assert!(matches!(
            open(&keyring, &aad, &short),
            Err(SecretsError::Decrypt)
        ));

        let mut empty = sealed.clone();
        empty.ciphertext = Vec::new();
        assert!(matches!(
            open(&keyring, &aad, &empty),
            Err(SecretsError::Decrypt)
        ));

        let mut truncated = sealed.clone();
        truncated.nonce.truncate(VALUE_NONCE_LEN - 1);
        assert!(matches!(
            open(&keyring, &aad, &truncated),
            Err(SecretsError::Decrypt)
        ));

        let mut long = sealed.clone();
        long.data_key_nonce.push(0x00);
        assert!(matches!(
            open(&keyring, &aad, &long),
            Err(SecretsError::Decrypt)
        ));

        let mut stub = sealed;
        stub.data_key_wrapped = vec![0u8; 3];
        assert!(matches!(
            open(&keyring, &aad, &stub),
            Err(SecretsError::Decrypt)
        ));
    }

    #[test]
    fn an_unconfigured_key_version_is_reported_as_a_decrypt_failure() {
        let sealed = seal(
            &two_version_keyring(),
            &aad(SecretScope::Global, None, "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");
        assert_eq!(sealed.key_version, 2);

        let aad = aad(SecretScope::Global, None, "DEPLOY_TOKEN");
        assert!(
            matches!(
                open(&version_one_keyring(), &aad, &sealed),
                Err(SecretsError::Decrypt)
            ),
            "open tells the caller nothing about which step failed"
        );
    }

    #[test]
    fn resealing_moves_a_value_to_a_new_identity() {
        let keyring = two_version_keyring();
        let id = a_uuid();
        let old = aad(SecretScope::Project, Some(id), "OLD_NAME");
        let new = aad(SecretScope::Project, Some(id), "NEW_NAME");

        let sealed = seal(&keyring, &old, b"not-a-real-token").expect("sealing succeeds");
        let resealed = reseal(&keyring, &old, &new, &sealed).expect("resealing succeeds");

        assert_eq!(
            open(&keyring, &new, &resealed)
                .expect("the value opens under the new AAD")
                .as_slice(),
            b"not-a-real-token"
        );
        assert!(
            matches!(open(&keyring, &old, &resealed), Err(SecretsError::Decrypt)),
            "the old identity must no longer open the row"
        );

        assert_eq!(
            resealed.data_key_wrapped, sealed.data_key_wrapped,
            "a rename does not change the data key"
        );
        assert_eq!(resealed.data_key_nonce, sealed.data_key_nonce);
        assert_eq!(resealed.key_version, sealed.key_version);
        assert_ne!(resealed.nonce, sealed.nonce, "with a fresh value nonce");
    }

    #[test]
    fn resealing_under_the_wrong_old_identity_fails() {
        let keyring = two_version_keyring();
        let old = aad(SecretScope::Global, None, "OLD_NAME");
        let new = aad(SecretScope::Global, None, "NEW_NAME");
        let sealed = seal(&keyring, &old, b"not-a-real-token").expect("sealing succeeds");

        assert!(matches!(
            reseal(&keyring, &new, &new, &sealed),
            Err(SecretsError::Decrypt)
        ));
    }

    #[test]
    fn rewrapping_moves_the_data_key_and_leaves_the_ciphertext() {
        let old_only = version_one_keyring();
        let aad = aad(SecretScope::Global, None, "DEPLOY_TOKEN");
        let sealed = seal(&old_only, &aad, b"not-a-real-token").expect("sealing succeeds");
        assert_eq!(sealed.key_version, 1);

        let keyring = two_version_keyring();
        let rewrapped = rewrap(&keyring, &sealed).expect("rewrapping succeeds");

        assert_eq!(rewrapped.key_version, 2);
        assert_eq!(
            rewrapped.ciphertext, sealed.ciphertext,
            "rotation never touches a ciphertext"
        );
        assert_eq!(rewrapped.nonce, sealed.nonce);
        assert_ne!(rewrapped.data_key_wrapped, sealed.data_key_wrapped);
        assert_ne!(rewrapped.data_key_nonce, sealed.data_key_nonce);

        assert_eq!(
            open(&keyring, &aad, &rewrapped)
                .expect("the rewrapped row still opens")
                .as_slice(),
            b"not-a-real-token"
        );
    }

    #[test]
    fn rewrapping_a_current_row_changes_nothing() {
        let keyring = two_version_keyring();
        let aad = aad(SecretScope::Global, None, "DEPLOY_TOKEN");
        let sealed = seal(&keyring, &aad, b"not-a-real-token").expect("sealing succeeds");

        let rewrapped = rewrap(&keyring, &sealed).expect("a current row is a no-op");

        assert!(rewrapped == sealed, "every column comes back unchanged");
    }

    #[test]
    fn rewrapping_a_row_whose_version_is_gone_names_the_version() {
        let old_only = version_one_keyring();
        let sealed = seal(
            &old_only,
            &aad(SecretScope::Global, None, "DEPLOY_TOKEN"),
            b"not-a-real-token",
        )
        .expect("sealing succeeds");

        // A keyring that has moved on without keeping version 1: rotation is
        // the operator's own operation, so it is told which key is missing.
        let newer = SecretsKeyring::from_entries(vec![(2, [0x22; MASTER_KEY_LEN])])
            .expect("one fake version is a valid keyring");

        assert!(matches!(
            rewrap(&newer, &sealed),
            Err(SecretsError::UnknownKeyVersion(1))
        ));
    }
}
