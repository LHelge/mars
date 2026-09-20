//! `RefreshTokenRepository` against a real Postgres (`CLAUDE.md`, "Testing
//! expectations").
//!
//! Most of it is a round trip and the scoping of each `WHERE` clause. The one
//! test that is not is the last: it runs a refresh against a password change
//! the way `docs/data-model.md`, "Users and authentication" says the two have
//! to interleave — user row first, token row second — and shows that the
//! refresh waiting on the lock reads the revocation rather than escaping it
//! with the row it saw before waiting (ADR 0025).
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use mars_orchestrator::models::{Email, NewUser, RefreshToken, User, Username};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{RefreshTokenRepository, UserRepository};
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the authentication epic will produce (`CLAUDE.md`, rule 3).
const FAKE_HASH: &str = "$argon2id$fake$hash";

/// How long a blocked transaction is given to prove it is blocked.
const BLOCKED_FOR: Duration = Duration::from_millis(400);

/// How long an unblocked transaction is given to finish once the lock is free.
const UNBLOCKED_WITHIN: Duration = Duration::from_secs(10);

/// Not a credential either: a stand-in for the SHA-256 hex of an opaque token.
fn fake_token_hash(label: &str) -> String {
    format!("fake-token-hash-{label}")
}

/// An hour from now, the usual "clearly still valid" expiry.
fn in_an_hour() -> DateTime<Utc> {
    Utc::now() + TimeDelta::try_hours(1).unwrap()
}

/// Insert a user in its own committed transaction and return the stored row.
async fn user(pool: &PgPool, username: &str, email: &str) -> User {
    let new = NewUser {
        id: Uuid::new_v4(),
        username: Username::parse(username).expect("the test username is valid"),
        email: Email::parse(email).expect("the test email is valid"),
        password_hash: FAKE_HASH.to_string(),
        admin: false,
        must_change_password: false,
    };

    let repository = UserRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(&mut tx, &new)
        .await
        .expect("the user inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
}

/// Insert a token in its own committed transaction.
async fn token(
    pool: &PgPool,
    user_id: Uuid,
    label: &str,
    expires_at: DateTime<Utc>,
) -> RefreshToken {
    let repository = RefreshTokenRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(&mut tx, user_id, &fake_token_hash(label), expires_at)
        .await
        .expect("the token inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
}

#[tokio::test]
async fn a_token_survives_an_insert_find_revoke_round_trip() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = RefreshTokenRepository::new(&pool);

    let ada = user(&pool, "ada", "ada@example.com").await;
    let expires_at = in_an_hour();
    let inserted = token(&pool, ada.id, "live", expires_at).await;

    assert_eq!(inserted.user_id, ada.id);
    assert_eq!(inserted.token_hash, fake_token_hash("live"));
    // `TIMESTAMPTZ` keeps microseconds, `DateTime<Utc>` nanoseconds, so the
    // round trip rounds; the caller's expiry is what was stored to the
    // precision the column has.
    assert!(
        (inserted.expires_at - expires_at).abs() < TimeDelta::try_milliseconds(1).unwrap(),
        "stored {}, asked for {expires_at}",
        inserted.expires_at,
    );
    assert_eq!(inserted.revoked_at, None);

    // The unlocked lookup a refresh starts from.
    assert_eq!(
        repository
            .find_by_hash(&fake_token_hash("live"))
            .await
            .unwrap()
            .as_ref(),
        Some(&inserted)
    );
    assert!(
        repository
            .find_by_hash(&fake_token_hash("never-issued"))
            .await
            .unwrap()
            .is_none()
    );

    // The authoritative re-read, under the user-row lock. Validity is the
    // statement's, so a live token comes back and nothing is checked in Rust.
    let mut tx = pool.begin().await.unwrap();
    UserRepository::new(&pool)
        .lock_user(&mut tx, ada.id)
        .await
        .unwrap()
        .expect("the user exists");
    assert_eq!(
        repository
            .find_usable_by_hash_for_user(&mut tx, &fake_token_hash("live"), ada.id)
            .await
            .unwrap()
            .as_ref(),
        Some(&inserted)
    );

    assert!(repository.revoke(&mut tx, inserted.id).await.unwrap());
    // A second revocation matches no row: a no-op, never an error.
    assert!(!repository.revoke(&mut tx, inserted.id).await.unwrap());
    tx.commit().await.unwrap();

    let revoked = repository
        .find_by_hash(&fake_token_hash("live"))
        .await
        .unwrap()
        .expect("the row is revoked, not deleted");
    assert!(revoked.revoked_at.is_some());
    // And the locked lookup no longer offers it: the revocation is in its
    // `WHERE` clause.
    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .find_usable_by_hash_for_user(&mut tx, &fake_token_hash("live"), ada.id)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();

    // Revoking a token that never existed is `false` as well.
    let mut tx = pool.begin().await.unwrap();
    assert!(!repository.revoke(&mut tx, Uuid::new_v4()).await.unwrap());
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn find_usable_by_hash_for_user_ignores_another_user_and_every_unusable_row() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = RefreshTokenRepository::new(&pool);

    let ada = user(&pool, "ada", "ada@example.com").await;
    let grace = user(&pool, "grace", "grace@example.com").await;
    token(&pool, ada.id, "ada", in_an_hour()).await;
    let an_hour_ago = Utc::now() - TimeDelta::try_hours(1).unwrap();
    token(&pool, ada.id, "expired", an_hour_ago).await;
    let revoked = token(&pool, ada.id, "revoked", in_an_hour()).await;

    let mut tx = pool.begin().await.unwrap();
    assert!(repository.revoke(&mut tx, revoked.id).await.unwrap());

    // The hash exists; it is simply not Grace's, and the scope is in the
    // `WHERE` clause rather than in a check after the read.
    assert!(
        repository
            .find_usable_by_hash_for_user(&mut tx, &fake_token_hash("ada"), grace.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_usable_by_hash_for_user(&mut tx, &fake_token_hash("ada"), ada.id)
            .await
            .unwrap()
            .is_some()
    );
    // Validity is in the same `WHERE` clause: an expired or revoked row of the
    // right user is not found either, so there is nothing left for a caller to
    // check afterwards.
    for label in ["expired", "revoked", "never-issued"] {
        assert!(
            repository
                .find_usable_by_hash_for_user(&mut tx, &fake_token_hash(label), ada.id)
                .await
                .unwrap()
                .is_none(),
            "{label} came back from the usable lookup",
        );
    }
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn revoke_by_hash_revokes_once_and_never_errors() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = RefreshTokenRepository::new(&pool);

    let ada = user(&pool, "ada", "ada@example.com").await;
    token(&pool, ada.id, "live", in_an_hour()).await;

    // Logout's statement: straight to the pool, no transaction, no lock.
    assert!(
        repository
            .revoke_by_hash(&fake_token_hash("live"))
            .await
            .unwrap()
    );
    assert!(
        !repository
            .revoke_by_hash(&fake_token_hash("live"))
            .await
            .unwrap(),
        "a second logout with the same cookie reported a change",
    );
    assert!(
        !repository
            .revoke_by_hash(&fake_token_hash("never-issued"))
            .await
            .unwrap()
    );

    let revoked = repository
        .find_by_hash(&fake_token_hash("live"))
        .await
        .unwrap()
        .expect("the row exists");
    assert!(revoked.revoked_at.is_some());
}

#[tokio::test]
async fn revoke_all_for_user_stops_at_that_user() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = RefreshTokenRepository::new(&pool);

    let ada = user(&pool, "ada", "ada@example.com").await;
    let grace = user(&pool, "grace", "grace@example.com").await;

    token(&pool, ada.id, "first", in_an_hour()).await;
    token(&pool, ada.id, "second", in_an_hour()).await;
    let already = token(&pool, ada.id, "already", in_an_hour()).await;
    token(&pool, grace.id, "grace", in_an_hour()).await;

    // The single revocation commits on its own, so the timestamp it wrote can
    // be read back from the pool and compared with what survives the sweep.
    let mut tx = pool.begin().await.unwrap();
    assert!(repository.revoke(&mut tx, already.id).await.unwrap());
    tx.commit().await.unwrap();
    let revoked_first_at = repository
        .find_by_hash(&fake_token_hash("already"))
        .await
        .unwrap()
        .expect("the token exists")
        .revoked_at;
    assert!(revoked_first_at.is_some());

    let mut tx = pool.begin().await.unwrap();
    // Two of the three: the one revoked a moment ago is already out.
    assert_eq!(
        repository
            .revoke_all_for_user(&mut tx, ada.id)
            .await
            .unwrap(),
        2
    );
    // And again, with nothing left to revoke.
    assert_eq!(
        repository
            .revoke_all_for_user(&mut tx, ada.id)
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();

    for label in ["first", "second", "already"] {
        let stored = repository
            .find_by_hash(&fake_token_hash(label))
            .await
            .unwrap()
            .expect("the row exists");
        assert!(stored.revoked_at.is_some(), "{label} is still usable");
    }
    // The earlier revocation kept its own timestamp.
    assert_eq!(
        repository
            .find_by_hash(&fake_token_hash("already"))
            .await
            .unwrap()
            .unwrap()
            .revoked_at,
        revoked_first_at,
    );
    // Grace was signing in on another machine and stays signed in.
    assert!(
        repository
            .find_by_hash(&fake_token_hash("grace"))
            .await
            .unwrap()
            .expect("the row exists")
            .revoked_at
            .is_none()
    );
}

#[tokio::test]
async fn delete_expired_removes_only_expired_rows() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = RefreshTokenRepository::new(&pool);

    let ada = user(&pool, "ada", "ada@example.com").await;
    let hour = TimeDelta::try_hours(1).unwrap();

    token(&pool, ada.id, "live", Utc::now() + hour).await;
    token(&pool, ada.id, "expired", Utc::now() - hour).await;
    // Revoked but unexpired: the cron job deletes by expiry only, so this one
    // stays until its own `expires_at` passes.
    let revoked = token(&pool, ada.id, "revoked", Utc::now() + hour).await;
    let mut tx = pool.begin().await.unwrap();
    assert!(repository.revoke(&mut tx, revoked.id).await.unwrap());
    tx.commit().await.unwrap();

    assert_eq!(repository.delete_expired(Utc::now()).await.unwrap(), 1);
    // Nothing left to reap on the next tick.
    assert_eq!(repository.delete_expired(Utc::now()).await.unwrap(), 0);

    assert!(
        repository
            .find_by_hash(&fake_token_hash("expired"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_by_hash(&fake_token_hash("live"))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        repository
            .find_by_hash(&fake_token_hash("revoked"))
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_refresh_waiting_on_the_user_lock_reads_the_revocation() {
    // Two connections: the password change holding the user row, the refresh
    // waiting for it.
    let (_postgres, pool) = common::db::test_pool_with(4).await;
    let users = UserRepository::new(&pool);
    let tokens = RefreshTokenRepository::new(&pool);

    let ada = user(&pool, "ada", "ada@example.com").await;
    let cookie = fake_token_hash("in-flight");
    token(&pool, ada.id, "in-flight", in_an_hour()).await;

    // The refresh has already made its unlocked lookup and would, on that
    // evidence alone, happily rotate the token.
    let seen = tokens
        .find_by_hash(&cookie)
        .await
        .unwrap()
        .expect("the token exists");
    assert!(seen.revoked_at.is_none());

    // Meanwhile a password change takes the user row and revokes everything.
    let mut holder = pool.begin().await.unwrap();
    users
        .lock_user(&mut holder, ada.id)
        .await
        .unwrap()
        .expect("the user exists");
    users
        .apply_password_change(&mut holder, ada.id, "$argon2id$fake$newhash", None)
        .await
        .unwrap();

    let waiting_pool = pool.clone();
    let waiting_cookie = cookie.clone();
    let waiter = tokio::spawn(async move {
        let users = UserRepository::new(&waiting_pool);
        let tokens = RefreshTokenRepository::new(&waiting_pool);
        let mut tx = waiting_pool.begin().await.unwrap();
        // The documented order: the user row, then that user's token row.
        let user = users.lock_user(&mut tx, ada.id).await.unwrap();
        let token = tokens
            .find_usable_by_hash_for_user(&mut tx, &waiting_cookie, ada.id)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        (user, token)
    });

    tokio::time::sleep(BLOCKED_FOR).await;
    assert!(
        !waiter.is_finished(),
        "the refresh did not wait for the password change's user-row lock"
    );

    holder.commit().await.unwrap();

    let (user, token) = tokio::time::timeout(UNBLOCKED_WITHIN, waiter)
        .await
        .expect("the lock is released on commit")
        .expect("the waiting task did not panic");

    assert_eq!(
        user.expect("the user exists").auth_version,
        1,
        "the refresh revalidated against a stale user row",
    );
    assert!(
        token.is_none(),
        "the refresh would have escaped revocation with a revoked token",
    );
    // The row read before waiting still says otherwise, which is exactly why
    // the re-read under the lock is the authoritative one.
    assert!(seen.revoked_at.is_none());
}
