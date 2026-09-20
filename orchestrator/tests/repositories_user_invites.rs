//! `UserInviteRepository` against a real Postgres (`CLAUDE.md`, "Testing
//! expectations").
//!
//! The interesting assertions are all about the word *open*. An invite is open
//! while it is unaccepted **and** unexpired, but `user_invites_open_email_idx`
//! only knows the first half, so the tests below pin both readings: what the
//! reads hide, what the index still refuses, and how
//! `delete_expired_open_for_email` reconciles the two. The acceptance lock is
//! asserted the only way a lock can be — a second transaction that does not
//! get through until the first commits.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use chrono::{DateTime, SubsecRound, TimeDelta, Utc};
use mars_orchestrator::models::{Email, NewUser, User, UserInvite, Username};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::user_invites;
use mars_orchestrator::repositories::{UserInviteRepository, UserRepository};
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the SHA-256 hex of an
/// invite token (`CLAUDE.md`, rule 3).
const FAKE_TOKEN_HASH: &str = "fake-invite-token-hash";

/// A second one, for the resend path.
const OTHER_FAKE_TOKEN_HASH: &str = "fake-rotated-invite-token-hash";

/// Not a credential: the fake Argon2id PHC string the user rows in these tests
/// carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// How long a blocked transaction is given to prove it is blocked.
const BLOCKED_FOR: Duration = Duration::from_millis(400);

/// How long an unblocked transaction is given to finish once the lock is free.
const UNBLOCKED_WITHIN: Duration = Duration::from_secs(10);

/// The current time, truncated to the microsecond `TIMESTAMPTZ` stores, so a
/// stored expiry compares equal to the value that was sent.
fn now() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

fn email(raw: &str) -> Email {
    Email::parse(raw).expect("the test email is valid")
}

/// An expiry safely in the past.
fn expired() -> DateTime<Utc> {
    now() - TimeDelta::days(1)
}

/// Insert an invite in its own committed transaction.
async fn insert_invite(
    pool: &PgPool,
    address: &str,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> UserInvite {
    let repository = UserInviteRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(
            &mut tx,
            &email(address),
            token_hash,
            false,
            None,
            expires_at,
        )
        .await
        .expect("the invite inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
}

/// Insert a user in its own committed transaction, for the foreign keys.
async fn insert_user(pool: &PgPool, username: &str, address: &str) -> User {
    let repository = UserRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(
            &mut tx,
            &NewUser {
                id: Uuid::new_v4(),
                username: Username::parse(username).expect("the test username is valid"),
                email: email(address),
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

#[tokio::test]
async fn an_invite_survives_an_insert_list_find_accept_round_trip() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserInviteRepository::new(&pool);

    let inviter = insert_user(&pool, "ada", "ada@example.com").await;
    let expires_at = user_invites::expires_at(now());

    let mut tx = pool.begin().await.unwrap();
    let inserted = repository
        .insert(
            &mut tx,
            // The normalised form is what gets stored, not what was typed.
            &email(" Grace@Example.COM "),
            FAKE_TOKEN_HASH,
            true,
            Some(inviter.id),
            expires_at,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(inserted.email, "grace@example.com");
    assert_eq!(inserted.token_hash, FAKE_TOKEN_HASH);
    assert!(inserted.admin);
    assert_eq!(inserted.invited_by, Some(inviter.id));
    assert_eq!(inserted.expires_at, expires_at);
    assert!(inserted.accepted_at.is_none());
    assert!(inserted.accepted_user_id.is_none());

    assert_eq!(
        repository.list_open().await.unwrap(),
        vec![inserted.clone()]
    );
    assert_eq!(
        repository.find_open_by_hash(FAKE_TOKEN_HASH).await.unwrap(),
        Some(inserted.clone())
    );
    assert_eq!(
        repository.find_open_by_id(inserted.id).await.unwrap(),
        Some(inserted.clone())
    );
    assert!(
        repository
            .find_open_by_hash("fake-unknown-hash")
            .await
            .unwrap()
            .is_none()
    );

    // Accepting is the invitee's transaction: lock, then mark.
    let invitee = insert_user(&pool, "grace", "grace@example.com").await;
    let mut tx = pool.begin().await.unwrap();
    let locked = repository
        .lock_open_by_hash(&mut tx, FAKE_TOKEN_HASH)
        .await
        .unwrap()
        .expect("the invite is open");
    assert_eq!(locked, inserted);
    assert!(
        repository
            .mark_accepted(&mut tx, inserted.id, invitee.id)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    // Single use, from every angle.
    assert!(repository.list_open().await.unwrap().is_empty());
    assert!(
        repository
            .find_open_by_hash(FAKE_TOKEN_HASH)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_open_by_id(inserted.id)
            .await
            .unwrap()
            .is_none()
    );

    // A second acceptance matches no row, which is how the loser of a race
    // learns to answer `invalid or expired invite`.
    let mut tx = pool.begin().await.unwrap();
    assert!(
        !repository
            .mark_accepted(&mut tx, inserted.id, invitee.id)
            .await
            .unwrap()
    );
    // And an accepted invite cannot be revoked or resent.
    assert!(!repository.delete_open(inserted.id).await.unwrap());
    assert!(
        repository
            .rotate_token(inserted.id, OTHER_FAKE_TOKEN_HASH, now())
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn an_expired_invite_is_not_open() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserInviteRepository::new(&pool);

    let invite = insert_invite(&pool, "grace@example.com", FAKE_TOKEN_HASH, expired()).await;

    assert!(repository.list_open().await.unwrap().is_empty());
    assert!(
        repository
            .find_open_by_hash(FAKE_TOKEN_HASH)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_open_by_id(invite.id)
            .await
            .unwrap()
            .is_none()
    );

    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .lock_open_by_hash(&mut tx, FAKE_TOKEN_HASH)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();

    // It is still revocable: the row exists and the admin can clear it.
    assert!(repository.delete_open(invite.id).await.unwrap());
}

#[tokio::test]
async fn a_second_open_invite_for_the_same_email_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserInviteRepository::new(&pool);

    let live = user_invites::expires_at(now());
    insert_invite(&pool, "grace@example.com", FAKE_TOKEN_HASH, live).await;

    // Different case and spacing: `Email::parse` normalises it onto the same
    // stored value, so the partial unique index catches it.
    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert(
            &mut tx,
            &email(" Grace@Example.COM "),
            OTHER_FAKE_TOKEN_HASH,
            false,
            None,
            live,
        )
        .await
        .expect_err("the address already has an open invite");

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert!(
        matches!(&error, Error::Conflict(message)
            if message == "an open invite already exists for this email"),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn inviting_an_email_that_belongs_to_a_user_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserInviteRepository::new(&pool);

    insert_user(&pool, "grace", "grace@example.com").await;

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert(
            &mut tx,
            &email("GRACE@example.com"),
            FAKE_TOKEN_HASH,
            false,
            None,
            user_invites::expires_at(now()),
        )
        .await
        .expect_err("the address is already a user");

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert!(
        matches!(&error, Error::Conflict(message) if message == "email already belongs to a user"),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn clearing_an_expired_invite_makes_room_for_a_new_one() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserInviteRepository::new(&pool);

    let stale = insert_invite(&pool, "grace@example.com", FAKE_TOKEN_HASH, expired()).await;
    let live = user_invites::expires_at(now());
    let address = email("grace@example.com");

    // The index does not know about `expires_at`, so the stale row is still in
    // the way even though nothing can be done with it any more.
    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .insert(&mut tx, &address, OTHER_FAKE_TOKEN_HASH, false, None, live)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    // Clear it and re-insert in one transaction, which is what the invite
    // route does.
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        repository
            .delete_expired_open_for_email(&mut tx, &address)
            .await
            .unwrap(),
        1
    );
    let replacement = repository
        .insert(&mut tx, &address, OTHER_FAKE_TOKEN_HASH, false, None, live)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_ne!(replacement.id, stale.id);
    assert_eq!(repository.list_open().await.unwrap(), [replacement]);

    // A live invite is a real conflict and is not cleared away.
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        repository
            .delete_expired_open_for_email(&mut tx, &address)
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn resending_rotates_the_token_and_the_expiry() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserInviteRepository::new(&pool);

    let invite = insert_invite(&pool, "grace@example.com", FAKE_TOKEN_HASH, expired()).await;

    let renewed = user_invites::expires_at(now());
    let rotated = repository
        .rotate_token(invite.id, OTHER_FAKE_TOKEN_HASH, renewed)
        .await
        .unwrap()
        .expect("the invite is unaccepted");

    assert_eq!(rotated.id, invite.id);
    assert_eq!(rotated.token_hash, OTHER_FAKE_TOKEN_HASH);
    assert_eq!(rotated.expires_at, renewed);
    assert!(rotated.expires_at > invite.expires_at);
    assert_eq!(rotated.email, invite.email);
    assert_eq!(rotated.created_at, invite.created_at);

    // One invite, one live token: the link in the earlier email is dead and
    // the expired invite is open again.
    assert!(
        repository
            .find_open_by_hash(FAKE_TOKEN_HASH)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        repository
            .find_open_by_hash(OTHER_FAKE_TOKEN_HASH)
            .await
            .unwrap(),
        Some(rotated)
    );

    assert!(
        repository
            .rotate_token(Uuid::new_v4(), OTHER_FAKE_TOKEN_HASH, renewed)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn the_reaper_deletes_expired_unaccepted_invites_only() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserInviteRepository::new(&pool);

    let live = insert_invite(
        &pool,
        "ada@example.com",
        FAKE_TOKEN_HASH,
        user_invites::expires_at(now()),
    )
    .await;
    insert_invite(&pool, "grace@example.com", OTHER_FAKE_TOKEN_HASH, expired()).await;

    // An accepted invite that has since expired is history, not garbage.
    let accepted = insert_invite(&pool, "kay@example.com", "fake-third-hash", expired()).await;
    let user = insert_user(&pool, "kay", "kay@example.com").await;
    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .mark_accepted(&mut tx, accepted.id, user.id)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    assert_eq!(repository.delete_expired(Utc::now()).await.unwrap(), 1);
    assert_eq!(repository.list_open().await.unwrap(), [live]);
    assert_eq!(repository.delete_expired(Utc::now()).await.unwrap(), 0);
    // The accepted row survived, so it can still be revoked-checked by id.
    assert!(!repository.delete_open(accepted.id).await.unwrap());
}

#[tokio::test]
async fn lock_open_by_hash_holds_the_invite_until_commit() {
    // Two connections: one accepting the invite, one waiting to.
    let (_postgres, pool) = common::db::test_pool_with(4).await;
    let repository = UserInviteRepository::new(&pool);

    let invite = insert_invite(
        &pool,
        "grace@example.com",
        FAKE_TOKEN_HASH,
        user_invites::expires_at(now()),
    )
    .await;
    let invitee = insert_user(&pool, "grace", "grace@example.com").await;

    let mut holder = pool.begin().await.unwrap();
    let locked = repository
        .lock_open_by_hash(&mut holder, FAKE_TOKEN_HASH)
        .await
        .unwrap()
        .expect("the invite is open");
    assert_eq!(locked.id, invite.id);
    assert!(
        repository
            .mark_accepted(&mut holder, invite.id, invitee.id)
            .await
            .unwrap()
    );

    let waiting_pool = pool.clone();
    let waiter = tokio::spawn(async move {
        let repository = UserInviteRepository::new(&waiting_pool);
        let mut tx = waiting_pool.begin().await.unwrap();
        let invite = repository
            .lock_open_by_hash(&mut tx, FAKE_TOKEN_HASH)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        invite
    });

    // The waiter has certainly reached its `SELECT ... FOR UPDATE` by now and
    // is still stuck on it.
    tokio::time::sleep(BLOCKED_FOR).await;
    assert!(
        !waiter.is_finished(),
        "a second FOR UPDATE on the same invite did not wait for the first transaction"
    );

    holder.commit().await.unwrap();

    // The loser of the race re-reads under its own lock and finds nothing
    // open: one token, one user.
    let invite = tokio::time::timeout(UNBLOCKED_WITHIN, waiter)
        .await
        .expect("the lock is released on commit")
        .expect("the waiting task did not panic");
    assert!(invite.is_none(), "the invite was accepted twice");
}
