//! The master-key rotation sweep.
//!
//! One routine, [`rewrap_outdated`], run either by `mars-orchestrator
//! rotate-secrets` or by the hourly `secret rotation` cron job
//! (`ARCHITECTURE.md`, "Secrets", Rotation, and "Background jobs"). It selects
//! rows whose `key_version` is behind the newest configured master key in
//! batches of [`ROTATION_BATCH`], unwraps each row's data key under the key
//! that wrapped it, wraps it again under the newest one and writes the three
//! wrapping columns back. `ciphertext` and `nonce` are never read and never
//! written: rotation changes which master key protects a data key, not the
//! data key and not the encryption of the value. An operator can therefore
//! drop an old master key from the environment as soon as no row references it
//! any more, which is what `remaining` in the report answers.
//!
//! **No long transaction.** Each row is its own autocommit `UPDATE ... WHERE id
//! = $1 AND key_version = $2` on a connection taken from the pool for that one
//! statement, so a sweep over a large table never holds a transaction open, the
//! API never waits for the sweep for longer than one row, and a sweep that dies
//! halfway has committed everything it did so far — the next run continues from
//! there. The version in the `WHERE` clause is what makes that safe: a
//! `PUT /secrets/{id}` between the batch and the write replaces the value under
//! a *new* data key wrapped by the newest master key, and the sweep's write for
//! the old data key then matches nothing and is counted as `skipped` rather
//! than overwriting it with a wrapping of the wrong key. A rename keeps the
//! data key, so it commutes with the sweep either way.
//!
//! **Termination.** A row whose `key_version` has no configured key cannot be
//! re-wrapped, stays selectable and would otherwise be handed back by every
//! subsequent batch forever, so the sweep stops as soon as a batch re-wraps
//! nothing. The consequence is deliberate and worth knowing: when the *lowest*
//! versions present are the ones with no key and they fill a whole batch, the
//! sweep stops before reaching rows it could have moved, because
//! [`SecretRepository::list_for_rotation`] hands back the oldest keys first and
//! has no cursor past them. Adding the missing key — which is the only real fix
//! for such a row, and which the startup check demands before the process boots
//! at all — releases the rest on the next run.
//!
//! Nothing here logs a key, a wrapping, a nonce or a value: the sweep's fields
//! are `key_version` and three counters (`CLAUDE.md`, rule 3).

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::prelude::*;
use crate::repositories::SecretRepository;
use crate::repositories::secrets::ROTATION_BATCH;
use crate::secrets::{SecretsError, SecretsKeyring, WrappedKey};

/// What one sweep did.
///
/// `rewrapped` and `skipped` add up to the distinct rows the sweep touched;
/// `remaining` is measured at the end and is how many rows are still behind the
/// newest key — zero means the older master keys can be removed from the
/// environment, and anything else is either a row a concurrent writer is
/// holding or a row under a version the keyring does not carry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RotationReport {
    /// Rows moved onto the newest master key by this sweep.
    pub rewrapped: u64,
    /// Distinct rows the sweep selected but did not move: a version with no
    /// configured key, or a row another writer changed in between. Counted by
    /// id, so a row that two batches both failed to move is one.
    pub skipped: u64,
    /// Rows still below the newest version when the sweep finished.
    pub remaining: u64,
}

/// Move every row behind the newest master key onto it.
///
/// Idempotent: a second run over a table the first one finished selects an
/// empty batch and reports zeros, and so does a sweep with a single-version
/// keyring, which is what the cron job does every hour when nobody is rotating
/// anything.
///
/// Returns `Err` on a database failure or on a row whose wrapping does not
/// verify under the key configured for its version — the latter means the
/// environment carries a *different* key under that version, which no amount
/// of sweeping fixes. A version with no key at all is not an error: it is
/// logged once and skipped, so one un-rotatable row cannot hold up the rest.
pub async fn rewrap_outdated(pool: &PgPool, keyring: &SecretsKeyring) -> Result<RotationReport> {
    let repository = SecretRepository::new(pool);
    let newest = newest_version(keyring)?;

    let mut report = RotationReport::default();
    // One `error!` per version, however many rows are under it.
    let mut reported: BTreeSet<i32> = BTreeSet::new();
    // By id, because a row the sweep cannot move comes back in the next batch
    // and is one skipped row, not two. It stays small: the batch that re-reads
    // them is the batch that ends the sweep.
    let mut skipped: BTreeSet<Uuid> = BTreeSet::new();

    loop {
        let batch = repository.list_for_rotation(newest, ROTATION_BATCH).await?;
        if batch.is_empty() {
            break;
        }

        let mut rewrapped_in_batch = 0;
        for secret in &batch {
            // Only the three wrapping columns and the id are read from the
            // row; `ciphertext` and `nonce` travel with it and are not touched.
            let Some(wrapped) = rewrap_data_key(
                keyring,
                &secret.data_key_wrapped,
                &secret.data_key_nonce,
                secret.key_version,
            )?
            else {
                if reported.insert(secret.key_version) {
                    error!(
                        key_version = secret.key_version,
                        "no master key is configured for this key version; its rows cannot be \
                         rotated until it is added back to SECRETS_MASTER_KEYS"
                    );
                }
                skipped.insert(secret.id);
                continue;
            };

            // One statement, one connection, no transaction: the row is
            // committed before the next one is read.
            let mut connection = pool.acquire().await?;
            let written = repository
                .rewrap(
                    &mut connection,
                    secret.id,
                    secret.key_version,
                    &wrapped.wrapped,
                    &wrapped.nonce,
                    wrapped.version,
                )
                .await?;

            if written {
                rewrapped_in_batch += 1;
            } else {
                // A concurrent writer moved the row after the batch read it;
                // it is already on a newer key, or gone.
                skipped.insert(secret.id);
            }
        }

        report.rewrapped += rewrapped_in_batch;

        // Nothing moved, so the same rows would come back forever.
        if rewrapped_in_batch == 0 {
            break;
        }
    }

    report.skipped = u64::try_from(skipped.len()).unwrap_or(u64::MAX);
    report.remaining = u64::try_from(repository.count_below_version(newest).await?).unwrap_or(0);

    info!(
        rewrapped = report.rewrapped,
        skipped = report.skipped,
        remaining = report.remaining,
        "secret master-key rotation sweep finished"
    );

    Ok(report)
}

/// Re-wrap one row's data key under the newest master key, or report that its
/// version has no key.
///
/// `Ok(None)` is the skip the sweep counts and the reason it can stop: a row
/// the keyring cannot unwrap will be selected by every batch, so the caller
/// treats a batch of nothing but `None` as the end of the sweep. The data key
/// exists only inside this function, in a [`Zeroizing`](zeroize::Zeroizing)
/// buffer that wipes itself when the row is done.
fn rewrap_data_key(
    keyring: &SecretsKeyring,
    data_key_wrapped: &[u8],
    data_key_nonce: &[u8],
    key_version: i32,
) -> std::result::Result<Option<WrappedKey>, SecretsError> {
    let data_key = match keyring.unwrap_data_key(data_key_wrapped, data_key_nonce, key_version) {
        Ok(data_key) => data_key,
        Err(SecretsError::UnknownKeyVersion(_)) => return Ok(None),
        // A wrapping that does not verify under the configured key is a
        // different fault, and it belongs to the operator undiluted.
        Err(err) => return Err(err),
    };

    let wrapped = keyring.wrap_data_key(&data_key)?;
    drop(data_key);

    Ok(Some(wrapped))
}

/// The newest version as `key_version` stores it.
///
/// [`SecretsKeyring`] refuses to build with a version the column could not
/// hold, so the conversion cannot fail; it is written out rather than cast so
/// that a change to that rule surfaces here instead of wrapping into a
/// negative version that no row would ever match.
fn newest_version(keyring: &SecretsKeyring) -> Result<i32> {
    i32::try_from(keyring.current_version()).map_err(|_| {
        Error::Internal("the keyring's newest version does not fit key_version".to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{DATA_KEY_LEN, MASTER_KEY_LEN};

    /// Obviously not a real master key: one repeated byte, per version
    /// (`CLAUDE.md`, rule 3).
    fn fake_master_key(byte: u8) -> [u8; MASTER_KEY_LEN] {
        [byte; MASTER_KEY_LEN]
    }

    /// Equally fake stand-in for a per-row data key.
    fn fake_data_key() -> [u8; DATA_KEY_LEN] {
        [0xAB; DATA_KEY_LEN]
    }

    fn keyring(versions: &[u32]) -> SecretsKeyring {
        let entries = versions
            .iter()
            .map(|version| (*version, fake_master_key(*version as u8)))
            .collect();

        SecretsKeyring::from_entries(entries).expect("the test keyring is valid")
    }

    #[test]
    fn a_row_moves_to_the_newest_key_and_its_data_key_survives() {
        let old = keyring(&[1]);
        let both = keyring(&[1, 2]);
        let wrapped = old
            .wrap_data_key(&fake_data_key())
            .expect("the data key wraps");

        let rotated = rewrap_data_key(&both, &wrapped.wrapped, &wrapped.nonce, wrapped.version)
            .expect("the version has a key")
            .expect("the row is re-wrapped");

        assert_eq!(rotated.version, 2, "the row moves onto the newest key");
        assert_ne!(
            rotated.wrapped, wrapped.wrapped,
            "a different key and a fresh nonce produce a different wrapping"
        );

        let recovered = both
            .unwrap_data_key(&rotated.wrapped, &rotated.nonce, rotated.version)
            .expect("the new wrapping opens under the newest key");
        assert_eq!(
            *recovered,
            fake_data_key(),
            "the data key itself is unchanged, so the ciphertext never has to move"
        );
    }

    #[test]
    fn a_batch_under_a_version_with_no_key_makes_no_progress_and_stops_the_sweep() {
        // The rows were wrapped under version 1; the operator dropped it from
        // the environment and left versions 2 and 3 behind.
        let gone = keyring(&[1]);
        let current = keyring(&[2, 3]);
        let batch: Vec<_> = (0..4)
            .map(|_| {
                gone.wrap_data_key(&fake_data_key())
                    .expect("the data key wraps")
            })
            .collect();

        let rewrapped = batch
            .iter()
            .filter_map(|wrapped| {
                rewrap_data_key(&current, &wrapped.wrapped, &wrapped.nonce, wrapped.version)
                    .expect("a missing key version is a skip, not an error")
            })
            .count();

        assert_eq!(
            rewrapped, 0,
            "every row is skipped, so the sweep's per-batch counter stays at zero and the loop \
             stops instead of selecting the same rows forever"
        );
    }

    #[test]
    fn a_wrapping_that_does_not_verify_is_an_error_rather_than_a_skip() {
        // Same version, different key: the operator put the wrong key back.
        let wrong =
            SecretsKeyring::from_entries(vec![(1, fake_master_key(0x99)), (2, fake_master_key(2))])
                .expect("the test keyring is valid");
        let wrapped = keyring(&[1])
            .wrap_data_key(&fake_data_key())
            .expect("the data key wraps");

        let err = rewrap_data_key(&wrong, &wrapped.wrapped, &wrapped.nonce, wrapped.version)
            .expect_err("a wrapping that does not verify aborts the sweep");
        assert!(matches!(err, SecretsError::Decrypt));
    }

    #[test]
    fn the_newest_version_is_the_column_version() {
        assert_eq!(
            newest_version(&keyring(&[1, 4, 9])).expect("the version fits"),
            9
        );
    }
}
