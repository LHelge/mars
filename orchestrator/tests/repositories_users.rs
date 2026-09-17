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
use chrono::{DateTime, SubsecRound, TimeDelta, Utc};
use mars_orchestrator::models::{Email, NewUser, User, UserUpdate, Username};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::UserRepository;
use uuid::Uuid;

/// The administrator the `users` migration seeds (`docs/data-model.md`).
const SEEDED_ADMIN: Uuid = Uuid::from_u128(1);

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the authentication epic will produce (`CLAUDE.md`, rule 3).
const FAKE_HASH: &str = "$argon2id$fake$hash";

/// The same, for the hash a password change replaces [`FAKE_HASH`] with.
const NEW_FAKE_HASH: &str = "$argon2id$fake$newhash";

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

/// Not a credential: a stand-in for the SHA-256 hex of an opaque token
/// (`CLAUDE.md`, rule 3). The column is only `TEXT` and only unique, so a
/// readable value makes a better fixture than sixty-four hex digits.
fn fake_token_hash(label: &str) -> String {
    format!("fake-token-hash-{label}")
}

/// Insert a `refresh_tokens` row directly, bypassing the repository, so that
/// the state a password change has to clean up can be set up exactly.
async fn insert_refresh_token(
    pool: &PgPool,
    user_id: Uuid,
    label: &str,
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO refresh_tokens (id, user_id, token_hash, expires_at, revoked_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(user_id)
    .bind(fake_token_hash(label))
    .bind(expires_at)
    .bind(revoked_at)
    .execute(pool)
    .await
    .expect("the refresh token inserts");

    id
}

/// The same, for `password_reset_tokens`, which has no repository yet.
async fn insert_reset_token(
    pool: &PgPool,
    user_id: Uuid,
    label: &str,
    used_at: Option<DateTime<Utc>>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO password_reset_tokens (id, user_id, token_hash, expires_at, used_at)
         VALUES ($1, $2, $3, NOW() + INTERVAL '1 hour', $4)",
    )
    .bind(id)
    .bind(user_id)
    .bind(fake_token_hash(label))
    .bind(used_at)
    .execute(pool)
    .await
    .expect("the reset token inserts");

    id
}

/// Every still-unrevoked refresh token of one user, as `(id, token_hash)`.
async fn unrevoked_refresh_tokens(pool: &PgPool, user_id: Uuid) -> Vec<(Uuid, String)> {
    sqlx::query_as(
        "SELECT id, token_hash FROM refresh_tokens
         WHERE user_id = $1 AND revoked_at IS NULL
         ORDER BY created_at, id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .expect("the tokens read back")
}

/// `revoked_at` of one refresh token.
async fn refresh_token_revoked_at(pool: &PgPool, id: Uuid) -> Option<DateTime<Utc>> {
    sqlx::query_scalar("SELECT revoked_at FROM refresh_tokens WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("the token exists")
}

/// `used_at` of one reset token.
async fn reset_token_used_at(pool: &PgPool, id: Uuid) -> Option<DateTime<Utc>> {
    sqlx::query_scalar("SELECT used_at FROM password_reset_tokens WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("the token exists")
}

#[tokio::test]
async fn find_by_username_or_email_matches_either_column() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserRepository::new(&pool);

    let ada = insert(&pool, &new_user("ada", "ada@example.com")).await;

    assert_eq!(
        repository
            .find_by_username_or_email("ada")
            .await
            .unwrap()
            .map(|user| user.id),
        Some(ada.id),
    );
    // The email half normalises what it is given, exactly as `Email::parse`
    // did before the row was stored.
    assert_eq!(
        repository
            .find_by_username_or_email("  ADA@Example.COM ")
            .await
            .unwrap()
            .map(|user| user.id),
        Some(ada.id),
    );
    // The username half does not: it is compared verbatim.
    assert!(
        repository
            .find_by_username_or_email("Ada")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_by_username_or_email("nobody@example.com")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .find_by_username_or_email("")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn find_by_username_or_email_prefers_the_username_owner() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserRepository::new(&pool);

    // A username that is also somebody else's email address: one identifier,
    // two matching rows. The username owner wins, whichever order the rows
    // happen to be in.
    let impostor = insert(&pool, &new_user("ada@example.com", "grace@example.com")).await;
    let ada = insert(&pool, &new_user("ada", "ada@example.com")).await;

    let found = repository
        .find_by_username_or_email("ada@example.com")
        .await
        .unwrap()
        .expect("both users match");
    assert_eq!(found.id, impostor.id, "the email owner won the ambiguity");
    assert_ne!(found.id, ada.id);
}

#[tokio::test]
async fn a_password_change_bumps_the_version_and_clears_every_token() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserRepository::new(&pool);

    let mut new = new_user("ada", "ada@example.com");
    new.must_change_password = true;
    let ada = insert(&pool, &new).await;
    let grace = insert(&pool, &new_user("grace", "grace@example.com")).await;

    let hour = TimeDelta::try_hours(1).unwrap();
    // Truncated to what `TIMESTAMPTZ` stores, so a timestamp this test writes
    // and then reads back compares equal.
    let now = Utc::now().trunc_subsecs(6);
    let live = insert_refresh_token(&pool, ada.id, "live", now + hour, None).await;
    let second = insert_refresh_token(&pool, ada.id, "second", now + hour, None).await;
    // Already revoked: the statement must leave its original timestamp alone.
    let already =
        insert_refresh_token(&pool, ada.id, "already", now + hour, Some(now - hour)).await;
    // Another user's token is out of scope and has to survive untouched.
    let other = insert_refresh_token(&pool, grace.id, "other", now + hour, None).await;

    let outstanding = insert_reset_token(&pool, ada.id, "outstanding", None).await;
    let spent = insert_reset_token(&pool, ada.id, "spent", Some(now - hour)).await;
    let other_reset = insert_reset_token(&pool, grace.id, "other-reset", None).await;

    let mut tx = pool.begin().await.unwrap();
    // The documented order: the user row first, then that user's token rows.
    let locked = repository
        .lock_user(&mut tx, ada.id)
        .await
        .unwrap()
        .expect("the user exists");
    assert_eq!(locked.auth_version, 0);
    let changed = repository
        .apply_password_change(
            &mut tx,
            ada.id,
            NEW_FAKE_HASH,
            Some(&fake_token_hash("replacement")),
        )
        .await
        .expect("the password changes");
    tx.commit().await.unwrap();

    assert_eq!(changed.password_hash, NEW_FAKE_HASH);
    assert_eq!(
        changed.auth_version, 1,
        "the version moved by exactly one (ADR 0025)"
    );
    assert!(!changed.must_change_password);
    // Nothing else about the user moved.
    assert_eq!(changed.username, ada.username);
    assert_eq!(changed.email, ada.email);
    assert_eq!(changed.admin, ada.admin);
    assert_eq!(changed.notify_email, ada.notify_email);
    assert_eq!(changed.created_at, ada.created_at);
    assert!(changed.updated_at > ada.updated_at);

    assert!(refresh_token_revoked_at(&pool, live).await.is_some());
    assert!(refresh_token_revoked_at(&pool, second).await.is_some());
    assert_eq!(
        refresh_token_revoked_at(&pool, already).await,
        Some(now - hour),
        "an already revoked token lost its original timestamp",
    );
    assert!(
        refresh_token_revoked_at(&pool, other).await.is_none(),
        "another user's token was revoked",
    );

    assert!(reset_token_used_at(&pool, outstanding).await.is_some());
    assert_eq!(
        reset_token_used_at(&pool, spent).await,
        Some(now - hour),
        "an already spent reset token lost its original timestamp",
    );
    assert!(
        reset_token_used_at(&pool, other_reset).await.is_none(),
        "another user's reset token was invalidated",
    );

    // Exactly one token survives: the replacement, inserted after the blanket
    // revocation and given the documented 30-day life (`SPEC.md`,
    // "Authentication").
    let usable = unrevoked_refresh_tokens(&pool, ada.id).await;
    assert_eq!(usable.len(), 1, "unexpected surviving tokens: {usable:?}");
    assert_eq!(usable[0].1, fake_token_hash("replacement"));

    let expires_at: DateTime<Utc> =
        sqlx::query_scalar("SELECT expires_at FROM refresh_tokens WHERE id = $1")
            .bind(usable[0].0)
            .fetch_one(&pool)
            .await
            .unwrap();
    let expected = now + TimeDelta::try_days(30).unwrap();
    assert!(
        (expires_at - expected).abs() < TimeDelta::try_minutes(5).unwrap(),
        "the replacement expires at {expires_at}, not about 30 days out",
    );
}

#[tokio::test]
async fn a_password_change_without_a_replacement_leaves_nobody_signed_in() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserRepository::new(&pool);

    let ada = insert(&pool, &new_user("ada", "ada@example.com")).await;
    let hour = TimeDelta::try_hours(1).unwrap();
    insert_refresh_token(&pool, ada.id, "live", Utc::now() + hour, None).await;
    insert_refresh_token(&pool, ada.id, "second", Utc::now() + hour, None).await;

    let mut tx = pool.begin().await.unwrap();
    let changed = repository
        .apply_password_change(&mut tx, ada.id, NEW_FAKE_HASH, None)
        .await
        .expect("the password changes");
    tx.commit().await.unwrap();

    assert_eq!(changed.auth_version, 1);
    let usable = unrevoked_refresh_tokens(&pool, ada.id).await;
    assert!(usable.is_empty(), "unexpected surviving tokens: {usable:?}");
}

#[tokio::test]
async fn a_password_change_on_a_missing_user_is_not_found() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserRepository::new(&pool);

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .apply_password_change(&mut tx, Uuid::new_v4(), NEW_FAKE_HASH, None)
        .await
        .expect_err("there is no such user");
    tx.commit().await.unwrap();

    assert_eq!(error.status(), StatusCode::NOT_FOUND);
    assert!(matches!(error, Error::NotFound), "unexpected: {error:?}");
}

#[tokio::test]
async fn a_password_change_rolls_back_with_its_transaction() {
    let (_postgres, pool) = common::db::test_pool().await;
    let repository = UserRepository::new(&pool);

    let ada = insert(&pool, &new_user("ada", "ada@example.com")).await;
    let live = insert_refresh_token(
        &pool,
        ada.id,
        "live",
        Utc::now() + TimeDelta::try_hours(1).unwrap(),
        None,
    )
    .await;

    // Four statements, one transaction: abandoning it has to leave none of
    // them behind (`docs/data-model.md`, "Users and authentication").
    let mut tx = pool.begin().await.unwrap();
    repository
        .apply_password_change(
            &mut tx,
            ada.id,
            NEW_FAKE_HASH,
            Some(&fake_token_hash("ghost")),
        )
        .await
        .expect("the password changes");
    tx.rollback().await.unwrap();

    let stored = repository.find(ada.id).await.unwrap().expect("ada exists");
    assert_eq!(stored.password_hash, FAKE_HASH);
    assert_eq!(stored.auth_version, 0);
    assert!(refresh_token_revoked_at(&pool, live).await.is_none());
    assert_eq!(unrevoked_refresh_tokens(&pool, ada.id).await.len(), 1);
}
