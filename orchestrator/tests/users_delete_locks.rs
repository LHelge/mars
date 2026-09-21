//! The lock order of user deletion (`docs/data-model.md`, "Users and
//! authentication"; `ARCHITECTURE.md`, "Task tracker" → "Tracker locks never
//! block a foreign-key check"; ADR 0041).
//!
//! Deleting a user clears every nullable reference to it, so the deletion has
//! to wait for whoever holds one of those rows — and once it holds the user
//! row exclusively, a foreign-key check on the user from that same holder
//! waits for the deletion. Two holders can make that check while holding a row
//! that names the user: a session-only writer, whose second `UPDATE` of its
//! `sessions` row re-checks `created_by`, and a tracker mutation that writes
//! another row for the same user while holding a task the user created.
//!
//! Each scenario here is one of those interleavings, held open by hand, and
//! each ended in `40P01` before the deletion took its locks in order: the user
//! row without blocking foreign-key checks, then the projects, then the
//! sessions, and the `DELETE` last.
//!
//! The rows are seeded with unchecked statements rather than through HTTP:
//! what is asserted is which transaction waits for which, and the transactions
//! have to be held open from here to show it. Every credential-shaped value is
//! obviously fake (`CLAUDE.md`, rule 3).
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use mars_orchestrator::events::TaskActor;
use mars_orchestrator::models::{NewEvent, TaskRef};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{SessionRepository, TaskRepository, UserRepository};
use mars_orchestrator::tracker::{CommentAuthor, TrackerMutation, add_comment};
use serde_json::json;
use tokio::time::{sleep, timeout};
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string.
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// Not a real image.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// How long a blocked transaction is given to prove it is blocked.
const BLOCKED_FOR: Duration = Duration::from_millis(400);

/// How long an unblocked transaction is given once nothing should be in its
/// way. Longer than Postgres' `deadlock_timeout`, so a cycle shows up as its
/// own error rather than as this timeout.
const UNBLOCKED_WITHIN: Duration = Duration::from_secs(10);

struct Fixture {
    /// The user being deleted: not an administrator, so the membership count
    /// has nothing to refuse.
    user_id: Uuid,
    project_id: Uuid,
    /// Created by the user.
    session_id: Uuid,
    /// Created by the user.
    task_id: Uuid,
}

async fn seed_user(pool: &PgPool) -> Uuid {
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, $2, $3, $4)")
        .bind(user_id)
        .bind(format!("user-{}", &user_id.simple().to_string()[..8]))
        .bind(format!("{user_id}@example.test"))
        .bind(FAKE_PASSWORD_HASH)
        .execute(pool)
        .await
        .expect("the user seeds");

    user_id
}

async fn seed(pool: &PgPool) -> Fixture {
    let user_id = seed_user(pool).await;

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        // `.invalid` can never resolve (`CLAUDE.md`, rule 3).
        .bind("https://example.invalid/org/repo.git")
        .execute(pool)
        .await
        .expect("the project seeds");

    let profile_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_profiles (id, project_id, name, image, partial_messages)
         VALUES ($1, $2, 'default', $3, TRUE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind(TEST_IMAGE)
    .execute(pool)
    .await
    .expect("the profile seeds");

    let session_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO sessions (id, project_id, profile_id, kind, base_ref, branch,
                              mcp_token_hash, created_by)
        VALUES ($1, $2, $3, 'conversational'::profile_kind, 'main', $4, $5, $6)
        "#,
    )
    .bind(session_id)
    .bind(project_id)
    .bind(profile_id)
    .bind(format!("session/{session_id}"))
    // Not a credential: the column stores a hash, and this is an obviously
    // fake one.
    .bind(format!("fake-token-hash-{session_id}"))
    .bind(user_id)
    .execute(pool)
    .await
    .expect("the session seeds");

    let repository = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    repository
        .insert_default_states(mutation.conn(), project_id)
        .await
        .expect("the default states insert");
    mutation.commit().await.expect("the mutation commits");
    let state_id = repository
        .find_state_by_name(project_id, "backlog")
        .await
        .unwrap()
        .expect("the project has a backlog")
        .id;

    let task_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tasks (id, project_id, number, title, state_id, created_by_user_id)
         VALUES ($1, $2, 1, 'task 1', $3, $4)",
    )
    .bind(task_id)
    .bind(project_id)
    .bind(state_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("the task seeds");

    Fixture {
        user_id,
        project_id,
        session_id,
        task_id,
    }
}

/// The deletion, by an administrator who is somebody else.
fn delete_in_background(pool: &PgPool, user_id: Uuid) -> tokio::task::JoinHandle<Result<()>> {
    let pool = pool.clone();
    tokio::spawn(async move {
        UserRepository::new(&pool)
            .delete(user_id, Uuid::new_v4())
            .await
    })
}

async fn user_exists(pool: &PgPool, user_id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("the user is looked up")
}

#[tokio::test]
async fn deleting_a_user_waits_for_a_writer_of_their_session_instead_of_deadlocking_with_it() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;
    let sessions = SessionRepository::new(&pool);

    // The owner's half of a `result` line: the session row lock and the batch.
    let mut writer = pool.begin().await.expect("the session writer opens");
    sessions
        .append_events(
            &mut writer,
            fixture.session_id,
            &[NewEvent::now(
                "assistant_message",
                json!({ "text": "fake" }),
            )],
        )
        .await
        .expect("the batch is appended under the session row lock");

    // The deletion has to clear `sessions.created_by`, so it waits.
    let deletion = delete_in_background(&pool, fixture.user_id);
    sleep(BLOCKED_FOR).await;
    assert!(
        !deletion.is_finished(),
        "the deletion did not wait for the session row",
    );

    // The second `UPDATE` of the row, whose foreign-key re-check asks for the
    // user: it must not wait for the deletion that is waiting for it.
    timeout(UNBLOCKED_WITHIN, async {
        sessions
            .add_usage(&mut writer, fixture.session_id, 0.01, 1, 1)
            .await?;
        writer.commit().await?;

        Ok::<(), Error>(())
    })
    .await
    .expect("the session writer is not blocked by the deletion")
    .expect("the session writer commits");

    timeout(UNBLOCKED_WITHIN, deletion)
        .await
        .expect("the deletion proceeds once the session row is free")
        .expect("the deletion does not panic")
        .expect("the deletion commits");

    assert!(!user_exists(&pool, fixture.user_id).await);
    let created_by: Option<Uuid> =
        sqlx::query_scalar("SELECT created_by FROM sessions WHERE id = $1")
            .bind(fixture.session_id)
            .fetch_one(&pool)
            .await
            .expect("the session is still there");
    assert_eq!(created_by, None);
}

#[tokio::test]
async fn deleting_a_user_waits_for_a_mutation_writing_for_them_instead_of_deadlocking_with_it() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;

    // A request of the user's own, in flight: the project row, and the task
    // they created.
    let mut mutation = TrackerMutation::begin(
        &pool,
        fixture.project_id,
        TaskActor::User {
            user_id: fixture.user_id,
        },
    )
    .await
    .expect("the mutation opens");
    let task = TaskRepository::new(&pool)
        .find_task_for_update(
            mutation.conn(),
            fixture.project_id,
            TaskRef::Id(fixture.task_id),
        )
        .await
        .expect("the task is read")
        .expect("the task is there");

    // The deletion has to clear `tasks.created_by_user_id`; it waits at the
    // project row, before it holds anything the mutation can want.
    let deletion = delete_in_background(&pool, fixture.user_id);
    sleep(BLOCKED_FOR).await;
    assert!(
        !deletion.is_finished(),
        "the deletion did not wait for the project's mutation",
    );

    // A comment by the user: a foreign-key check on the row being deleted.
    timeout(UNBLOCKED_WITHIN, async {
        add_comment(
            &mut mutation,
            &task,
            CommentAuthor::User(fixture.user_id),
            "still here",
        )
        .await?;
        mutation.commit().await?;

        Ok::<(), Error>(())
    })
    .await
    .expect("the mutation is not blocked by the deletion")
    .expect("the mutation commits");

    timeout(UNBLOCKED_WITHIN, deletion)
        .await
        .expect("the deletion proceeds once the project row is free")
        .expect("the deletion does not panic")
        .expect("the deletion commits");

    assert!(!user_exists(&pool, fixture.user_id).await);
    let authors: Vec<Option<Uuid>> =
        sqlx::query_scalar("SELECT author_user_id FROM task_comments WHERE task_id = $1")
            .bind(fixture.task_id)
            .fetch_all(&pool)
            .await
            .expect("the comments are read");
    assert_eq!(authors, vec![None], "the comment outlives its author");
}
