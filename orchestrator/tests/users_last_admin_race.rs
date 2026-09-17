//! The last-administrator invariant under concurrency (`SPEC.md`, "Users
//! (`/api/users`)", last paragraph; `docs/data-model.md`, "Users and
//! authentication").
//!
//! > The check and mutation are one transaction, serialized with other user
//! > deletions and administrator-role changes, so concurrent requests cannot
//! > each remove one of the final two administrators. A rejected request
//! > changes no fields.
//!
//! > Validation must cover concurrent demotions and deletion racing with
//! > demotion, as well as the single-request cases.
//!
//! `tests/users.rs` covers the single-request cases; this file covers the two
//! the paragraph names. With exactly two administrators left, every pair of
//! concurrent removals has the same shape: one of them takes the
//! administrator-membership advisory lock first and commits, the other waits,
//! re-counts under the lock and finds one administrator where it expected two.
//! So each test asserts three things rather than one — that exactly one request
//! succeeded, that an administrator remains, and that the rejected request left
//! *every* field alone, including the `username` it carried in the same body.
//! The last one is the part a status code cannot show: the rename happens after
//! the check in the same transaction, so only a rollback keeps it out of the
//! row.
//!
//! The two racing tests launch both requests while a third transaction holds
//! the membership lock and release it once both are blocked on it
//! (`common::races::release_after_in_flight`). A bare `join!` is not enough:
//! the first request routinely runs to completion before the second one has
//! loaded its caller, and the second one then fails on the *caller* — 403 for
//! an administrator who was demoted a moment ago, or 401 for one who was
//! deleted — which are the extractor's rules, not this invariant's. The gate
//! puts both callers past authentication before either mutation commits, which
//! is the situation `SPEC.md` describes.
//!
//! Every password here is obviously fake (`CLAUDE.md`, rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::TestApp;
use common::races::{
    IN_FLIGHT, RACE_ITERATIONS, RACE_TIMEOUT, admin_count, hold_admin_membership_lock,
    release_after_in_flight,
};
use mars_orchestrator::models::User;
use mars_orchestrator::repositories::UserRepository;
use serde_json::{Value, json};
use tokio::time::{sleep, timeout};
use uuid::Uuid;

/// Obviously fake, and inside the documented 10–128 range. Nothing here logs
/// in; `POST /api/test/users` simply requires a valid one.
const PASSWORD: &str = "correct-horse-battery-staple";

/// The username the losing `PUT` carries. Finding it in the row would mean the
/// rejected request wrote something.
const RENAMED: &str = "renamed";

/// `PUT`/`DELETE /users/{id}` for `id`.
fn user_route(id: Uuid) -> String {
    format!("/api/users/{id}")
}

/// The documented 409 body for `message`.
fn conflict(message: &str) -> Value {
    json!({ "status": 409, "error": message })
}

/// The `PUT` body that demotes `user` without renaming them.
fn demotion(user: &User) -> Value {
    json!({ "username": user.username, "admin": false })
}

/// The `PUT` body that demotes *and* renames, so a rejection has a second
/// field to have left alone.
fn demotion_and_rename() -> Value {
    json!({ "username": RENAMED, "admin": false })
}

/// The row as the database has it now, or `None` if it is gone.
async fn stored(app: &TestApp, id: Uuid) -> Option<User> {
    UserRepository::new(&app.pool)
        .find(id)
        .await
        .expect("the lookup succeeds")
}

/// Exactly one of two racing removals succeeded and exactly one was refused.
///
/// Each response is paired with the status *it* answers when it wins, because
/// the two removals do not share one: a demotion returns 200 with the updated
/// user and a deletion 204.
///
/// Two 409s would mean neither request could see the other's rollback, which
/// is as much a bug as two successes: serialised, whichever runs first has two
/// administrators to count. Anything else — a 403 for a caller demoted in the
/// meantime, a 401 for one deleted — means the race never happened and the
/// test proved nothing.
fn assert_one_won(first: (&TestResponse, StatusCode), second: (&TestResponse, StatusCode)) {
    let (first_response, first_success) = first;
    let (second_response, second_success) = second;
    let statuses = [first_response.status_code(), second_response.status_code()];

    assert!(
        statuses == [first_success, StatusCode::CONFLICT]
            || statuses == [StatusCode::CONFLICT, second_success],
        "expected one winner ({first_success} or {second_success}) and one 409, \
         got {statuses:?}"
    );
}

/// Two administrators demoting each other at the same time: one of them stays
/// an administrator.
///
/// Repeated with a fresh pair each round, because which request reaches the
/// lock first is the runtime's and Postgres's choice and one round proves only
/// that the winner of that round was serialised correctly.
///
/// Both requests are launched behind [`release_after_in_flight`], so they are
/// provably in flight together: the invariant is only under test when both
/// callers authenticated while both were still administrators.
#[tokio::test]
async fn concurrent_mutual_demotions_leave_one_administrator() {
    let app = TestApp::spawn().await;

    for round in 0..RACE_ITERATIONS {
        // The previous round's survivor is still an administrator, and "the
        // final two" is the premise of the whole test, so the board is cleared
        // before each pair is created.
        sqlx::query("UPDATE users SET admin = FALSE")
            .execute(&app.pool)
            .await
            .expect("the previous round's administrators step down");

        let a = app
            .create_admin(
                &format!("ada{round:02}"),
                &format!("ada{round:02}@example.test"),
                PASSWORD,
            )
            .await;
        let b = app
            .create_admin(
                &format!("bob{round:02}"),
                &format!("bob{round:02}@example.test"),
                PASSWORD,
            )
            .await;
        assert_eq!(admin_count(&app.pool).await, 2);

        let gate = hold_admin_membership_lock(&app.pool).await;
        let a_demotes_b = app
            .put_as(&a, &user_route(b.user.id))
            .json(&demotion(&b.user));
        let b_demotes_a = app
            .put_as(&b, &user_route(a.user.id))
            .json(&demotion(&a.user));

        let (first, second, ()) = timeout(RACE_TIMEOUT, async {
            tokio::join!(a_demotes_b, b_demotes_a, release_after_in_flight(gate))
        })
        .await
        .expect("the two demotions did not deadlock");

        assert_one_won((&first, StatusCode::OK), (&second, StatusCode::OK));
        assert_eq!(
            admin_count(&app.pool).await,
            1,
            "round {round} removed both of the final two administrators"
        );
    }
}

/// The deterministic form of the same race: the membership lock is held while
/// the demotion is in flight, and the administrator who asked for it is
/// demoted inside the holding transaction.
///
/// The request therefore reaches its re-count with one administrator left —
/// the very row it is trying to demote — and must refuse.
#[tokio::test]
async fn a_demotion_that_waits_for_the_last_administrator_is_409() {
    let app = TestApp::spawn().await;
    let a = app.create_admin("ada", "ada@example.test", PASSWORD).await;
    let b = app.create_admin("bob", "bob@example.test", PASSWORD).await;

    // Held before the request starts, so `replace` blocks on
    // `lock_admin_membership` — after it has authenticated `a` as an
    // administrator, and before it counts anything.
    let mut holder = hold_admin_membership_lock(&app.pool).await;

    let a_demotes_b = app
        .put_as(&a, &user_route(b.user.id))
        .json(&demotion_and_rename());

    let demote_a = async {
        sleep(IN_FLIGHT).await;

        sqlx::query("UPDATE users SET admin = FALSE WHERE id = $1")
            .bind(a.user.id)
            .execute(&mut *holder)
            .await
            .expect("the acting administrator is demoted");
        holder.commit().await.expect("the holder commits");
    };

    let (response, ()) = timeout(RACE_TIMEOUT, async { tokio::join!(a_demotes_b, demote_a) })
        .await
        .expect("the demotion and the holder did not deadlock");

    response.assert_status(StatusCode::CONFLICT);
    assert_eq!(
        response.json::<Value>(),
        conflict("cannot demote the last administrator")
    );

    // Neither field of the rejected `PUT` was applied.
    let row = stored(&app, b.user.id).await.expect("bob is still there");
    assert!(row.admin);
    assert_eq!(row.username, b.user.username);
    assert_eq!(admin_count(&app.pool).await, 1);
}

/// A deletion racing a demotion, the two mutations the invariant covers,
/// aimed at each other: one of them wins and an administrator survives.
///
/// Both orders are legitimate, so the branch is on which one committed first
/// rather than on a fixed expectation — but in each of them the loser's
/// transaction rolled back whole, which for the `PUT` means its `username` did
/// not land either.
///
/// Behind the same starting gate as the mutual demotions: a deletion that
/// commits before the demotion's caller has authenticated would answer the
/// demotion with a 401 for a deleted user, which is a different rule.
#[tokio::test]
async fn a_deletion_racing_a_demotion_leaves_an_administrator() {
    let app = TestApp::spawn().await;
    let a = app.create_admin("ada", "ada@example.test", PASSWORD).await;
    let b = app.create_admin("bob", "bob@example.test", PASSWORD).await;

    let gate = hold_admin_membership_lock(&app.pool).await;
    let a_deletes_b = app.delete_as(&a, &user_route(b.user.id));
    let b_demotes_a = app
        .put_as(&b, &user_route(a.user.id))
        .json(&demotion_and_rename());

    let (deletion, demotion, ()) = timeout(RACE_TIMEOUT, async {
        tokio::join!(a_deletes_b, b_demotes_a, release_after_in_flight(gate))
    })
    .await
    .expect("the deletion and the demotion did not deadlock");

    assert_one_won(
        (&deletion, StatusCode::NO_CONTENT),
        (&demotion, StatusCode::OK),
    );

    if deletion.status_code() == StatusCode::NO_CONTENT {
        // The deletion committed first; the demotion then found `ada` to be
        // the only administrator left and refused.
        assert_eq!(
            demotion.json::<Value>(),
            conflict("cannot demote the last administrator")
        );
        assert!(stored(&app, b.user.id).await.is_none());

        let row = stored(&app, a.user.id).await.expect("ada is still there");
        assert!(row.admin);
        assert_eq!(row.username, a.user.username);
    } else {
        // The demotion committed first; the deletion then found `bob` to be
        // the only administrator left and refused.
        assert_eq!(
            deletion.json::<Value>(),
            conflict("cannot delete the last administrator")
        );

        let row = stored(&app, b.user.id).await.expect("bob is still there");
        assert!(row.admin);

        // The winning `PUT` applied both of its fields.
        let row = stored(&app, a.user.id).await.expect("ada is still there");
        assert!(!row.admin);
        assert_eq!(row.username, RENAMED);
    }

    assert_eq!(admin_count(&app.pool).await, 1);
}
