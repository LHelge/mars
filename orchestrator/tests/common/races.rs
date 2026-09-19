//! Locks, counts and timings for the concurrency suites
//! (`tests/auth_revocation_race.rs`, `tests/users_last_admin_race.rs`).
//!
//! Both suites assert the same kind of thing in two forms. The *deterministic*
//! form opens the lock the route under test is about to take, holds it while
//! the request is in flight, does the racing work inside the holding
//! transaction and commits: the interleaving is then chosen rather than hoped
//! for, and the route has no way to reach its check before the racing change is
//! committed. The *repeated* form runs the two requests through
//! [`tokio::join!`] without a lock and asserts the invariant afterwards, so the
//! ordering rules in the routes and the repository are exercised as written
//! rather than only as described.
//!
//! The two `hold_*` helpers return the open transaction. Dropping it rolls the
//! transaction back and releases the lock, so a caller has to hold it for as
//! long as it wants the route to wait; a caller that means to commit calls
//! `commit()` on it.
//!
//! A test that gets the lock order wrong deadlocks, and a deadlocked
//! `#[tokio::test]` hangs until the harness is killed rather than failing. So
//! every join in both suites is wrapped in a [`RACE_TIMEOUT`].

use std::time::Duration;

use mars_orchestrator::repositories::users::ADMIN_MEMBERSHIP_LOCK_KEY;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

/// The ceiling on every raced step.
///
/// Generous enough that a slow container never trips it and short enough that
/// a genuine deadlock is a failing test rather than a hung run.
pub const RACE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a deterministic race leaves a request in flight before the holding
/// transaction commits.
///
/// Long enough for the request to authenticate, hash whatever it hashes and
/// block on the lock; the same half second the single-request last-administrator
/// test in `tests/users.rs` uses.
pub const IN_FLIGHT: Duration = Duration::from_millis(500);

/// How many times each repeated race runs.
///
/// Modest on purpose: the deterministic variants are the guarantee and these
/// are the smoke check, so the number is chosen to keep a CI run short.
pub const RACE_ITERATIONS: usize = 20;

/// Begin a transaction and hold `id`'s `users` row with `SELECT ... FOR
/// UPDATE`.
///
/// This is the lock `UserRepository::lock_user` takes, so every credential
/// mutation — login, refresh, a password change, a reset link — waits behind
/// this transaction (`docs/data-model.md`, "Users and authentication";
/// ADR 0025).
pub async fn hold_user_lock(pool: &PgPool, id: Uuid) -> Transaction<'static, Postgres> {
    let mut tx = pool.begin().await.expect("a transaction begins");

    let locked: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .expect("the user row exists and is locked");
    assert_eq!(locked, id);

    tx
}

/// Begin a transaction and take the administrator-membership advisory lock.
///
/// The same key `UserRepository::lock_admin_membership` takes, so a deletion or
/// a role change waits behind this transaction and re-counts administrators
/// only once it commits (`docs/data-model.md`, "Users and authentication").
pub async fn hold_admin_membership_lock(pool: &PgPool) -> Transaction<'static, Postgres> {
    let mut tx = pool.begin().await.expect("a transaction begins");

    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMIN_MEMBERSHIP_LOCK_KEY)
        .execute(&mut *tx)
        .await
        .expect("the advisory lock is granted");

    tx
}

/// Begin a transaction and hold `project_id`'s `projects` row with `SELECT
/// ... FOR UPDATE`.
///
/// The lock `TrackerMutation::begin` takes, so every tracker mutation of that
/// project — a creation, a state change, a dependency edge, a comment — waits
/// behind this transaction (`ARCHITECTURE.md`, "Task tracker" → "One mutation
/// at a time per project"; ADR 0021). The starting gate for the tracker races,
/// used with [`release_after_in_flight`].
pub async fn hold_project_lock(pool: &PgPool, project_id: Uuid) -> Transaction<'static, Postgres> {
    let mut tx = pool.begin().await.expect("a transaction begins");

    let locked: Uuid = sqlx::query_scalar("SELECT id FROM projects WHERE id = $1 FOR UPDATE")
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await
        .expect("the project row exists and is locked");
    assert_eq!(locked, project_id);

    tx
}

/// Let go of a lock once the requests racing behind it have certainly reached
/// it.
///
/// The starting gate of the repeated races: two requests are launched while
/// [`hold_admin_membership_lock`] is held, so both of them authenticate and
/// block on the lock, and only then does this release it. Without the gate the
/// race is degenerate — the first request runs to completion before the second
/// one has even loaded its caller, which leaves the second one facing a
/// *demoted* caller rather than a changed administrator count, and tests the
/// extractor instead of the invariant.
///
/// A rollback rather than a commit: the holder is a gate and writes nothing.
pub async fn release_after_in_flight(holder: Transaction<'static, Postgres>) {
    tokio::time::sleep(IN_FLIGHT).await;
    holder.rollback().await.expect("the holder rolls back");
}

/// How many of `user_id`'s refresh tokens are still unrevoked.
///
/// The count a revocation race turns on: a password change revokes every one
/// of them and inserts a replacement only for a self-service change, so this
/// is 0 after an administrator's change or a reset and 1 after the user's own
/// (`SPEC.md`, "Authentication").
pub async fn unrevoked_refresh_tokens(pool: &PgPool, user_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM refresh_tokens WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("the count succeeds")
}

/// How many administrators the database has right now.
///
/// The invariant the last-administrator races are about: never zero
/// (`SPEC.md`, "Users (`/api/users`)").
pub async fn admin_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE admin")
        .fetch_one(pool)
        .await
        .expect("the count succeeds")
}
