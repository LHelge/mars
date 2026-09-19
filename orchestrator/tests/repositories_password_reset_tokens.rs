//! `PasswordResetTokenRepository` against a real Postgres (`CLAUDE.md`,
//! "Testing expectations").
//!
//! `docs/data-model.md`, `password_reset_tokens` gives two rules and both are
//! asserted here: a token "must be unexpired and have null `used_at` when
//! revalidated under the user lock", and consuming one invalidates *every*
//! outstanding token of that user, not just itself.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use chrono::{DateTime, SubsecRound, TimeDelta, Utc};
use mars_orchestrator::models::{Email, NewUser, User, Username};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{PasswordResetTokenRepository, UserRepository};
use uuid::Uuid;

/// Not a credential: obviously fake stand-ins for the SHA-256 hex of a reset
/// token (`CLAUDE.md`, rule 3).
const FAKE_TOKEN_HASH: &str = "fake-reset-token-hash";
const OTHER_FAKE_TOKEN_HASH: &str = "fake-second-reset-token-hash";

/// Not a credential: the fake Argon2id PHC string the user rows carry.
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// The current time, truncated to the microsecond `TIMESTAMPTZ` stores, so a
/// stored expiry compares equal to the value that was sent.
fn now() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

/// An hour from now, the shape of a real reset link's lifetime.
fn soon() -> DateTime<Utc> {
    now() + TimeDelta::hours(1)
}

/// An expiry safely in the past.
fn expired() -> DateTime<Utc> {
    now() - TimeDelta::hours(1)
}

/// Insert a user in its own committed transaction.
async fn insert_user(pool: &PgPool, username: &str, address: &str) -> User {
    let repository = UserRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(
            &mut tx,
            &NewUser {
                id: Uuid::new_v4(),
                username: Username::parse(username).expect("the test username is valid"),
                email: Email::parse(address).expect("the test email is valid"),
                password_hash: FAKE_PASSWORD_HASH.to_string(),
                admin: false,
                must_change_password: false,
            },
        )
        .await
        .expect("the user inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
}

/// Insert a reset token in its own committed transaction.
async fn insert_token(pool: &PgPool, user_id: Uuid, token_hash: &str, expires_at: DateTime<Utc>) {
    let repository = PasswordResetTokenRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    repository
        .insert(&mut tx, user_id, token_hash, expires_at)
        .await
        .expect("the token inserts");
    tx.commit().await.expect("the transaction commits");
}

#[tokio::test]
async fn a_reset_token_survives_an_insert_locate_revalidate_consume_round_trip() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = PasswordResetTokenRepository::new(&pool);
    let users = UserRepository::new(&pool);

    let user = insert_user(&pool, "ada", "ada@example.com").await;
    let expires_at = soon();

    let mut tx = pool.begin().await.unwrap();
    let inserted = repository
        .insert(&mut tx, user.id, FAKE_TOKEN_HASH, expires_at)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(inserted.user_id, user.id);
    assert_eq!(inserted.token_hash, FAKE_TOKEN_HASH);
    assert_eq!(inserted.expires_at, expires_at);
    assert!(inserted.used_at.is_none());

    // Step one of consumption: locate the user from the token alone.
    let located = repository
        .find_by_hash(FAKE_TOKEN_HASH)
        .await
        .unwrap()
        .expect("the token exists");
    assert_eq!(located, inserted);
    assert!(
        repository
            .find_by_hash("fake-unknown-hash")
            .await
            .unwrap()
            .is_none()
    );

    // Steps two and three: lock the user, then revalidate under the lock.
    let mut tx = pool.begin().await.unwrap();
    users
        .lock_user(&mut tx, located.user_id)
        .await
        .unwrap()
        .expect("the user exists");
    let revalidated = repository
        .find_valid_by_hash_for_user(&mut tx, FAKE_TOKEN_HASH, user.id)
        .await
        .unwrap()
        .expect("the token is valid");
    assert_eq!(revalidated, inserted);

    assert_eq!(
        repository
            .mark_all_used_for_user(&mut tx, user.id)
            .await
            .unwrap(),
        1
    );
    tx.commit().await.unwrap();

    // Single use: the row is still found by hash, but never revalidates again.
    let spent = repository
        .find_by_hash(FAKE_TOKEN_HASH)
        .await
        .unwrap()
        .expect("the token row is kept");
    assert!(spent.used_at.is_some());

    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .find_valid_by_hash_for_user(&mut tx, FAKE_TOKEN_HASH, user.id)
            .await
            .unwrap()
            .is_none()
    );
    // And a second consumption has nothing left to spend.
    assert_eq!(
        repository
            .mark_all_used_for_user(&mut tx, user.id)
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn an_expired_token_never_revalidates() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = PasswordResetTokenRepository::new(&pool);

    let user = insert_user(&pool, "ada", "ada@example.com").await;
    insert_token(&pool, user.id, FAKE_TOKEN_HASH, expired()).await;

    // Still locatable — the unlocked read is unfiltered on purpose — but not
    // valid.
    assert!(
        repository
            .find_by_hash(FAKE_TOKEN_HASH)
            .await
            .unwrap()
            .is_some()
    );

    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .find_valid_by_hash_for_user(&mut tx, FAKE_TOKEN_HASH, user.id)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_token_does_not_revalidate_against_another_user() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = PasswordResetTokenRepository::new(&pool);

    let ada = insert_user(&pool, "ada", "ada@example.com").await;
    let grace = insert_user(&pool, "grace", "grace@example.com").await;
    insert_token(&pool, ada.id, FAKE_TOKEN_HASH, soon()).await;

    // The user id in the `WHERE` is what pins the token to the row the caller
    // locked: presenting Ada's token while holding Grace's lock spends
    // nothing.
    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .find_valid_by_hash_for_user(&mut tx, FAKE_TOKEN_HASH, grace.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_valid_by_hash_for_user(&mut tx, FAKE_TOKEN_HASH, ada.id)
            .await
            .unwrap()
            .is_some()
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn consuming_one_token_invalidates_every_outstanding_token_of_that_user() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = PasswordResetTokenRepository::new(&pool);

    // Two requests within the hourly limit leave two live links.
    let ada = insert_user(&pool, "ada", "ada@example.com").await;
    let grace = insert_user(&pool, "grace", "grace@example.com").await;
    insert_token(&pool, ada.id, FAKE_TOKEN_HASH, soon()).await;
    insert_token(&pool, ada.id, OTHER_FAKE_TOKEN_HASH, soon()).await;
    insert_token(&pool, grace.id, "fake-third-reset-token-hash", soon()).await;

    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        repository
            .mark_all_used_for_user(&mut tx, ada.id)
            .await
            .unwrap(),
        2
    );
    tx.commit().await.unwrap();

    for hash in [FAKE_TOKEN_HASH, OTHER_FAKE_TOKEN_HASH] {
        let token = repository
            .find_by_hash(hash)
            .await
            .unwrap()
            .expect("the token row is kept");
        assert!(token.used_at.is_some(), "{hash} is still live");
    }

    // Another user's link is untouched.
    let other = repository
        .find_by_hash("fake-third-reset-token-hash")
        .await
        .unwrap()
        .expect("the token exists");
    assert!(other.used_at.is_none());
}

#[tokio::test]
async fn the_reaper_deletes_expired_tokens_only() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = PasswordResetTokenRepository::new(&pool);

    let user = insert_user(&pool, "ada", "ada@example.com").await;
    insert_token(&pool, user.id, FAKE_TOKEN_HASH, soon()).await;
    insert_token(&pool, user.id, OTHER_FAKE_TOKEN_HASH, expired()).await;

    // A spent token that has not expired stays, so replaying its link inside
    // the window is still recognised as spent rather than as unknown. Spending
    // reaches both of this user's outstanding rows, expired or not.
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        repository
            .mark_all_used_for_user(&mut tx, user.id)
            .await
            .unwrap(),
        2
    );
    tx.commit().await.unwrap();

    assert_eq!(repository.delete_expired().await.unwrap(), 1);
    assert!(
        repository
            .find_by_hash(OTHER_FAKE_TOKEN_HASH)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_by_hash(FAKE_TOKEN_HASH)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(repository.delete_expired().await.unwrap(), 0);
}

#[tokio::test]
async fn deleting_a_user_takes_their_tokens_with_them() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = PasswordResetTokenRepository::new(&pool);
    let users = UserRepository::new(&pool);

    let user = insert_user(&pool, "ada", "ada@example.com").await;
    insert_token(&pool, user.id, FAKE_TOKEN_HASH, soon()).await;

    // Any acting id but the target's: `delete` reads it only to refuse a
    // self-deletion, and `ada` is no administrator, so no count stands in the
    // way either.
    users.delete(user.id, Uuid::new_v4()).await.unwrap();

    // `ON DELETE CASCADE` (`docs/data-model.md`): no orphaned live link.
    assert!(
        repository
            .find_by_hash(FAKE_TOKEN_HASH)
            .await
            .unwrap()
            .is_none()
    );
}
