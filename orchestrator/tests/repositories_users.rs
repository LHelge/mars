//! `UserRepository` against a real Postgres (`CLAUDE.md`, "Testing
//! expectations").
//!
//! The CRUD half is a round trip and the two duplicate paths; the interesting
//! half is the two locks `docs/data-model.md`, "Users and authentication"
//! requires. Those are asserted the only way a lock can be: a second
//! transaction is started while the first still holds the lock, and the test
//! shows that it does not get through until the first commits. A lock that was
//! quietly dropped from a statement would pass every functional assertion and
//! fail here.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use mars_orchestrator::models::{Email, NewUser, User, UserUpdate, Username};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::UserRepository;
use uuid::Uuid;

/// The administrator the `users` migration seeds (`docs/data-model.md`).
const SEEDED_ADMIN: Uuid = Uuid::from_u128(1);

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the authentication epic will produce (`CLAUDE.md`, rule 3).
const FAKE_HASH: &str = "$argon2id$fake$hash";

/// How long a blocked transaction is given to prove it is blocked. Long enough
/// that a slow container has certainly started the second transaction, short
/// enough not to drag the suite out.
const BLOCKED_FOR: Duration = Duration::from_millis(400);

/// How long an unblocked transaction is given to finish once the lock is free.
const UNBLOCKED_WITHIN: Duration = Duration::from_secs(10);

fn new_user(username: &str, email: &str) -> NewUser {
    NewUser {
        id: Uuid::new_v4(),
        username: Username::parse(username).expect("the test username is valid"),
        email: Email::parse(email).expect("the test email is valid"),
        password_hash: FAKE_HASH.to_string(),
        admin: false,
        must_change_password: false,
    }
}

/// Insert `user` in its own committed transaction.
async fn insert(pool: &PgPool, user: &NewUser) -> User {
    let repository = UserRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(&mut tx, user)
        .await
        .expect("the user inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
}

#[tokio::test]
async fn a_user_survives_an_insert_find_list_update_delete_round_trip() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserRepository::new(&pool);

    let new = new_user("ada", "Ada@Example.COM");
    let inserted = insert(&pool, &new).await;

    assert_eq!(inserted.id, new.id);
    assert_eq!(inserted.username, "ada");
    // The normalised form is what was stored, not what was typed.
    assert_eq!(inserted.email, "ada@example.com");
    assert_eq!(inserted.password_hash, FAKE_HASH);
    assert_eq!(inserted.auth_version, 0);
    assert!(!inserted.admin);
    assert!(!inserted.must_change_password);
    // Column defaults the repository does not set.
    assert!(inserted.notify_email);

    assert_eq!(
        repository.find(new.id).await.unwrap().as_ref(),
        Some(&inserted)
    );
    assert_eq!(
        repository.find_by_username("ada").await.unwrap().as_ref(),
        Some(&inserted)
    );
    assert_eq!(
        repository
            .find_by_email(&Email::parse(" ADA@example.com ").unwrap())
            .await
            .unwrap()
            .as_ref(),
        Some(&inserted)
    );
    assert!(repository.find(Uuid::new_v4()).await.unwrap().is_none());
    assert!(
        repository
            .find_by_username("nobody")
            .await
            .unwrap()
            .is_none()
    );

    // The seeded admin was created first, so it sorts first.
    let listed = repository.list().await.unwrap();
    assert_eq!(
        listed.iter().map(|user| user.id).collect::<Vec<_>>(),
        [SEEDED_ADMIN, new.id]
    );

    let update = UserUpdate {
        username: Some(Username::parse("ada.l").unwrap()),
        admin: Some(true),
        notify_email: Some(false),
    };
    let mut tx = pool.begin().await.unwrap();
    let updated = repository
        .update(&mut tx, new.id, &update)
        .await
        .unwrap()
        .expect("the user exists");
    tx.commit().await.unwrap();

    assert_eq!(updated.username, "ada.l");
    assert!(updated.admin);
    assert!(!updated.notify_email);
    // Untouched fields keep their values, and `updated_at` moved.
    assert_eq!(updated.email, "ada@example.com");
    assert_eq!(updated.auth_version, 0);
    assert_eq!(updated.created_at, inserted.created_at);
    assert!(updated.updated_at > inserted.updated_at);

    // An empty update still refreshes `updated_at` and changes nothing else.
    let mut tx = pool.begin().await.unwrap();
    let untouched = repository
        .update(&mut tx, new.id, &UserUpdate::default())
        .await
        .unwrap()
        .expect("the user exists");
    tx.commit().await.unwrap();
    assert_eq!(untouched.username, "ada.l");
    assert!(untouched.admin);
    assert!(!untouched.notify_email);

    // Updating a user that does not exist is `None`, not an error.
    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .update(&mut tx, Uuid::new_v4(), &update)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    assert!(repository.delete(&mut tx, new.id).await.unwrap());
    // A second delete matches no row: `false`, not an error, so the route can
    // answer 404.
    assert!(!repository.delete(&mut tx, new.id).await.unwrap());
    tx.commit().await.unwrap();

    assert!(repository.find(new.id).await.unwrap().is_none());
    assert_eq!(repository.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_duplicate_username_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserRepository::new(&pool);

    insert(&pool, &new_user("ada", "ada@example.com")).await;

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert(&mut tx, &new_user("ada", "other@example.com"))
        .await
        .expect_err("the username is taken");

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert!(
        matches!(&error, Error::Conflict(message) if message == "username already taken"),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn a_duplicate_email_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserRepository::new(&pool);

    insert(&pool, &new_user("ada", "ada@example.com")).await;

    // Different case and spacing: `Email::parse` normalises it onto the same
    // stored value, so the unique index catches it.
    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert(&mut tx, &new_user("grace", " Ada@Example.COM "))
        .await
        .expect_err("the email is registered");

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert!(
        matches!(&error, Error::Conflict(message) if message == "email already registered"),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn renaming_a_user_onto_a_taken_username_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserRepository::new(&pool);

    let ada = insert(&pool, &new_user("ada", "ada@example.com")).await;
    insert(&pool, &new_user("grace", "grace@example.com")).await;

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .update(
            &mut tx,
            ada.id,
            &UserUpdate {
                username: Some(Username::parse("grace").unwrap()),
                ..UserUpdate::default()
            },
        )
        .await
        .expect_err("the username is taken");

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert!(
        matches!(&error, Error::Conflict(message) if message == "username already taken"),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn count_admins_sees_the_seeded_administrator_and_then_none() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserRepository::new(&pool);

    let mut tx = pool.begin().await.unwrap();
    assert_eq!(repository.count_admins(&mut tx).await.unwrap(), 1);

    // An ordinary user does not move the count.
    repository
        .insert(&mut tx, &new_user("ada", "ada@example.com"))
        .await
        .unwrap();
    assert_eq!(repository.count_admins(&mut tx).await.unwrap(), 1);

    // Deleting the last administrator does; the repository allows it, because
    // the invariant is the caller's composition, not this statement's.
    assert!(repository.delete(&mut tx, SEEDED_ADMIN).await.unwrap());
    assert_eq!(repository.count_admins(&mut tx).await.unwrap(), 0);
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    assert_eq!(repository.count_admins(&mut tx).await.unwrap(), 0);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn lock_user_returns_the_row_and_holds_it_until_commit() {
    // Two connections: one holding the lock, one waiting for it.
    let (_postgres, pool) = common::db::test_pool_with(4).await;
    let repository = UserRepository::new(&pool);

    let mut holder = pool.begin().await.unwrap();
    let locked = repository
        .lock_user(&mut holder, SEEDED_ADMIN)
        .await
        .unwrap()
        .expect("the seeded administrator exists");
    assert_eq!(locked.id, SEEDED_ADMIN);
    assert_eq!(locked.username, "admin");
    assert!(locked.admin);
    assert!(locked.must_change_password);

    // Change the row under the lock, so the waiter can prove it read the
    // committed value rather than the one it could have seen before waiting.
    repository
        .update(
            &mut holder,
            SEEDED_ADMIN,
            &UserUpdate {
                username: Some(Username::parse("root").unwrap()),
                ..UserUpdate::default()
            },
        )
        .await
        .unwrap()
        .expect("the seeded administrator exists");

    let waiting_pool = pool.clone();
    let waiter = tokio::spawn(async move {
        let repository = UserRepository::new(&waiting_pool);
        let mut tx = waiting_pool.begin().await.unwrap();
        let user = repository.lock_user(&mut tx, SEEDED_ADMIN).await.unwrap();
        tx.commit().await.unwrap();
        user
    });

    // The waiter has certainly reached its `SELECT ... FOR UPDATE` by now and
    // is still stuck on it.
    tokio::time::sleep(BLOCKED_FOR).await;
    assert!(
        !waiter.is_finished(),
        "a second FOR UPDATE on the same row did not wait for the first transaction"
    );

    holder.commit().await.unwrap();

    let user = tokio::time::timeout(UNBLOCKED_WITHIN, waiter)
        .await
        .expect("the lock is released on commit")
        .expect("the waiting task did not panic")
        .expect("the seeded administrator exists");
    assert_eq!(user.username, "root");
}

#[tokio::test]
async fn lock_user_on_a_missing_user_locks_nothing() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserRepository::new(&pool);

    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .lock_user(&mut tx, Uuid::new_v4())
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn lock_admin_membership_serialises_two_transactions() {
    let (_postgres, pool) = common::db::test_pool_with(4).await;
    let repository = UserRepository::new(&pool);

    let mut holder = pool.begin().await.unwrap();
    repository.lock_admin_membership(&mut holder).await.unwrap();

    // The whole point of the lock: this second administrator only becomes
    // visible to the waiter's re-count after the first transaction commits.
    let mut second_admin = new_user("grace", "grace@example.com");
    second_admin.admin = true;
    repository.insert(&mut holder, &second_admin).await.unwrap();

    let waiting_pool = pool.clone();
    let waiter = tokio::spawn(async move {
        let repository = UserRepository::new(&waiting_pool);
        let mut tx = waiting_pool.begin().await.unwrap();
        repository.lock_admin_membership(&mut tx).await.unwrap();
        let count = repository.count_admins(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        count
    });

    tokio::time::sleep(BLOCKED_FOR).await;
    assert!(
        !waiter.is_finished(),
        "a second pg_advisory_xact_lock on the same key did not wait"
    );

    holder.commit().await.unwrap();

    let count = tokio::time::timeout(UNBLOCKED_WITHIN, waiter)
        .await
        .expect("the advisory lock is released on commit")
        .expect("the waiting task did not panic");
    assert_eq!(count, 2, "the re-count did not see the committed work");
}

#[tokio::test]
async fn the_advisory_lock_is_released_on_rollback() {
    let (_postgres, pool) = common::db::test_pool_with(4).await;
    let repository = UserRepository::new(&pool);

    let mut rolled_back = pool.begin().await.unwrap();
    repository
        .lock_admin_membership(&mut rolled_back)
        .await
        .unwrap();
    rolled_back.rollback().await.unwrap();

    // Nothing to unlock by hand: the next transaction takes it immediately.
    let mut tx = pool.begin().await.unwrap();
    tokio::time::timeout(UNBLOCKED_WITHIN, repository.lock_admin_membership(&mut tx))
        .await
        .expect("a rolled-back transaction does not keep the advisory lock")
        .unwrap();
    tx.commit().await.unwrap();
}
