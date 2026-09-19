//! `tracker::handoffs::publish_in_transaction`: the database half of
//! publishing a code hand-off (`ARCHITECTURE.md`, "Task tracker" → "Code
//! hand-offs" and "Review approval"; `SPEC.md`, "Code hand-offs and review";
//! `docs/data-model.md`, `tasks`, `task_handoffs`, `task_comments`,
//! `task_sessions`; ADR 0018, 0021, 0028, 0030).
//!
//! The git half is `tests/tracker_handoffs.rs`; nothing here touches git. A
//! [`PreparedHandoff`] is exactly the hand-over between the two, so every case
//! builds one directly and asserts what the one transaction underneath it
//! leaves behind:
//!
//! - a revision by the holding session writes one comment and one `unreviewed`
//!   record, points `current_handoff_id` at it, clears the lease, resets
//!   `attempts`, moves the task, links the session, and emits `state_changed`
//!   carrying the new hand-off followed by `commented`;
//! - a revision by a user is created by that user and still links the source
//!   session the code came from;
//! - a forward with a decision records the deciding caller and the time;
//! - a forward without one carries the previous verdict, reviewer and time
//!   forward — including after the reviewer's user row is deleted;
//! - a new revision after an approval is `unreviewed` even at the same commit;
//! - a task that moved since preparation is a 409 that writes nothing;
//! - a terminal target closes the task and unblocks its dependants.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use axum::http::StatusCode;
use chrono::Utc;
use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::models::{
    HandoffCaller, NewSession, NewTask, NewTaskComment, NewTaskHandoff, ProfileKind,
    ReviewDecision, ReviewStatus, Task, TaskComment, TaskDependencyKind, TaskHandoff, TaskRef,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::tasks::test_support::{StateFields, TaskRepositoryTestExt};
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::tracker::graph::recompute_blocked;
use mars_orchestrator::tracker::handoffs::{
    HANDOFF_RECHECK_FAILED, PreparedHandoff, ReviewCarry, publish_in_transaction,
};
use mars_orchestrator::tracker::{TaskDto, TrackerMutation, UpdateTaskInput};
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the seeded user would carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// Not a real image: the stub the session tests replay a fixture transcript
/// with.
const TEST_IMAGE: &str = "localhost/mars-session-stub:test";

/// An obviously fake but well-formed SHA-1 object id (rule 3).
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// A second one, for the revision that supersedes the first.
const OTHER_COMMIT: &str = "89abcdef0123456789abcdef0123456789abcdef";

/// A project with the documented default states, a profile to hang sessions
/// off, and a user to attribute REST-side changes to.
struct Fixture {
    user_id: Uuid,
    project_id: Uuid,
    profile_id: Uuid,
}

async fn seed(pool: &PgPool) -> Fixture {
    let user_id = seed_user(pool).await;

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        // `.invalid` can never resolve (rule 3).
        .bind("https://example.invalid/org/repo.git")
        .execute(pool)
        .await
        .expect("the project seeds");

    let profile_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_profiles (id, project_id, name, image, partial_messages)
         VALUES ($1, $2, $3, $4, TRUE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind("default")
    .bind(TEST_IMAGE)
    .execute(pool)
    .await
    .expect("the profile seeds");

    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(pool)
        .insert_default_states(mutation.conn(), project_id)
        .await
        .expect("the default states insert");
    mutation.commit().await.expect("the mutation commits");

    Fixture {
        user_id,
        project_id,
        profile_id,
    }
}

/// One more user, for the cases that need a reviewer distinct from the
/// forwarder.
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

/// A session, so a lease and a hand-off have something to point at.
async fn seed_session(pool: &PgPool, project_id: Uuid, profile_id: Uuid) -> Uuid {
    let session = NewSession::new(
        project_id,
        profile_id,
        ProfileKind::Conversational,
        "main",
        // Not a credential: a fake stand-in for the hashed MCP token (rule 3).
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );

    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = SessionRepository::new(pool)
        .insert(&mut tx, &session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    inserted.id
}

/// A task of this project in the given state.
async fn task(pool: &PgPool, project_id: Uuid, title: &str, state: &str) -> Task {
    let mut new = NewTask::new(project_id, title).expect("the title parses");
    new.state_id = Some(state_id(pool, project_id, state).await);

    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let inserted = TaskRepository::new(pool)
        .insert_task(mutation.conn(), project_id, &new)
        .await
        .expect("the task inserts");
    mutation.commit().await.expect("the mutation commits");

    inserted
}

/// The id of a project's state by name.
async fn state_id(pool: &PgPool, project_id: Uuid, name: &str) -> Uuid {
    TaskRepository::new(pool)
        .find_state_by_name(project_id, name)
        .await
        .expect("the state reads")
        .expect("the project has this state")
        .id
}

/// Put a lease and an attempt count on a task, the way a claim would.
async fn claim(pool: &PgPool, project_id: Uuid, task_id: Uuid, session_id: Uuid) -> Task {
    set_fields(
        pool,
        project_id,
        task_id,
        StateFields {
            lease: Some(Some((session_id, Utc::now()))),
            attempts: Some(2),
            ..StateFields::default()
        },
    )
    .await
}

async fn set_fields(pool: &PgPool, project_id: Uuid, task_id: Uuid, fields: StateFields) -> Task {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let task = TaskRepository::new(pool)
        .set_task_state_fields(mutation.conn(), project_id, task_id, &fields)
        .await
        .expect("the fields write");
    mutation.commit().await.expect("the mutation commits");

    task
}

/// An existing hand-off on `task`, made current: what a forward forwards.
///
/// `reviewed_by` records an approval by that user, the way a reviewing forward
/// before this one would have.
async fn current_handoff(
    pool: &PgPool,
    project_id: Uuid,
    task: &Task,
    source_session_id: Option<Uuid>,
    created_by_user_id: Uuid,
    reviewed_by: Option<Uuid>,
) -> (Task, TaskHandoff) {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let repository = TaskRepository::new(pool);

    let comment = NewTaskComment::from_user(task.id, created_by_user_id, "the first revision");
    let comment = repository
        .insert_comment(mutation.conn(), project_id, &comment)
        .await
        .expect("the comment inserts");

    let mut handoff = NewTaskHandoff::new(
        task.id,
        format!("session/{}", source_session_id.unwrap_or_else(Uuid::new_v4)),
        COMMIT,
        comment.id,
    );
    handoff.source_session_id = source_session_id;
    handoff.created_by_user_id = Some(created_by_user_id);
    if let Some(reviewer) = reviewed_by {
        handoff.review_status = ReviewStatus::Approved;
        handoff.reviewed_by_user_id = Some(reviewer);
        handoff.reviewed_at = Some(Utc::now());
    }
    let handoff = repository
        .insert_handoff(mutation.conn(), project_id, &handoff)
        .await
        .expect("the hand-off inserts");
    mutation.commit().await.expect("the mutation commits");

    let task = set_fields(
        pool,
        project_id,
        task.id,
        StateFields {
            current_handoff_id: Some(Some(handoff.id)),
            ..StateFields::default()
        },
    )
    .await;

    (task, handoff)
}

/// What [`prepare`](mars_orchestrator::tracker::handoffs::prepare) would have
/// produced for this task, without doing any git work.
fn prepared(
    task: &Task,
    source_session_id: Option<Uuid>,
    commit: &str,
    review: ReviewCarry,
) -> PreparedHandoff {
    let id = Uuid::new_v4();

    PreparedHandoff {
        id,
        task_id: task.id,
        state_id: task.state_id,
        lease_holder_session_id: task.lease_holder_session_id,
        source_session_id,
        source_branch: format!("session/{}", source_session_id.unwrap_or_else(Uuid::new_v4)),
        commit: commit.to_string(),
        comment: "ready for review".to_string(),
        previous_handoff_id: task.current_handoff_id,
        review,
        ref_name: format!("refs/handoffs/{id}"),
    }
}

/// Publish the way the composition above this function will: open the
/// mutation, read the row under its lock, publish, and commit only on success.
async fn publish(
    pool: &PgPool,
    project_id: Uuid,
    caller: HandoffCaller,
    prepared: &PreparedHandoff,
    update: UpdateTaskInput,
) -> Result<TaskDto> {
    let actor = match caller {
        HandoffCaller::User { user_id } => TaskActor::User { user_id },
        HandoffCaller::Session { session_id } => TaskActor::Session { session_id },
    };

    let mut mutation = TrackerMutation::begin(pool, project_id, actor)
        .await
        .expect("the mutation opens");

    let published = async {
        let task = TaskRepository::new(pool)
            .find_task_for_update(mutation.conn(), project_id, TaskRef::Id(prepared.task_id))
            .await?
            .ok_or(Error::NotFound)?;

        publish_in_transaction(&mut mutation, &task, prepared, &caller, update).await
    }
    .await;

    match published {
        Ok(task) => {
            mutation.commit().await.expect("the mutation commits");
            Ok(task)
        }
        Err(error) => {
            mutation.no_change().await.expect("the mutation rolls back");
            Err(error)
        }
    }
}

/// The state-only update every hand-off carries at minimum.
fn move_to(state: &str) -> UpdateTaskInput {
    UpdateTaskInput {
        state: Some(state.to_string()),
        ..UpdateTaskInput::default()
    }
}

/// The task as it is committed.
async fn read(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> TaskDto {
    TaskRepository::new(pool)
        .load_task_dto(project_id, task_id)
        .await
        .expect("the task reads")
        .expect("the task is in this project")
}

/// Every committed event of the project, oldest first.
async fn events(pool: &PgPool, project_id: Uuid) -> Vec<TaskEvent> {
    TaskRepository::new(pool)
        .list_task_events_after(project_id, 0, 100)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
        .collect()
}

/// The events written after `seq`, which is how each test reads only its own.
async fn events_after(pool: &PgPool, project_id: Uuid, seq: i64) -> Vec<TaskEvent> {
    events(pool, project_id)
        .await
        .into_iter()
        .filter(|event| event.seq > seq)
        .collect()
}

/// The highest sequence written so far.
async fn last_seq(pool: &PgPool, project_id: Uuid) -> i64 {
    events(pool, project_id)
        .await
        .last()
        .map(|event| event.seq)
        .unwrap_or(0)
}

/// This task's comments, oldest first.
async fn comments(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> Vec<TaskComment> {
    TaskRepository::new(pool)
        .list_comments(project_id, task_id)
        .await
        .expect("the comments read")
}

/// This task's hand-off records, oldest first.
async fn handoffs(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> Vec<TaskHandoff> {
    TaskRepository::new(pool)
        .list_handoffs(project_id, task_id)
        .await
        .expect("the hand-offs read")
}

/// The sessions linked to this task.
async fn linked_sessions(pool: &PgPool, task_id: Uuid) -> Vec<Uuid> {
    TaskRepository::new(pool)
        .list_task_sessions(task_id)
        .await
        .expect("the links read")
        .into_iter()
        .map(|link| link.session_id)
        .collect()
}

#[tokio::test]
async fn a_revision_by_the_holding_session_writes_the_whole_publication() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "implement it", "ready").await;
    let subject = claim(&pool, project_id, subject.id, session_id).await;
    let seq = last_seq(&pool, project_id).await;

    let prepared = prepared(&subject, Some(session_id), COMMIT, ReviewCarry::Fresh);
    let published = publish(
        &pool,
        project_id,
        HandoffCaller::Session { session_id },
        &prepared,
        move_to("review"),
    )
    .await
    .expect("the publication succeeds");

    // The task moved, the lease went with the move, and the pointer is the
    // record that was just written.
    assert_eq!(published.state, "review");
    assert_eq!(published.lease_holder_session_id, None);
    assert_eq!(published.lease_since, None);
    assert_eq!(published.attempts, 0);
    assert_eq!(published.closed_at, None);
    let handoff = published.handoff.clone().expect("the task has a hand-off");
    assert_eq!(handoff.id, prepared.id);
    assert_eq!(handoff.commit, COMMIT);
    assert_eq!(handoff.source_session_id, Some(session_id));
    assert_eq!(handoff.source_branch, prepared.source_branch);
    assert_eq!(handoff.created_by_session_id, Some(session_id));
    assert_eq!(handoff.created_by_user_id, None);
    assert_eq!(handoff.review_status, ReviewStatus::Unreviewed);
    assert_eq!(handoff.reviewed_by_user_id, None);
    assert_eq!(handoff.reviewed_by_session_id, None);
    assert_eq!(handoff.reviewed_at, None);
    assert_eq!(read(&pool, project_id, subject.id).await, published);

    // Exactly one comment, by the calling session, and the record names it.
    let comments = comments(&pool, project_id, subject.id).await;
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].body, "ready for review");
    assert_eq!(comments[0].author_session_id, Some(session_id));
    assert_eq!(comments[0].author_user_id, None);
    assert!(!comments[0].system);
    assert_eq!(handoff.comment_id, Some(comments[0].id));

    assert_eq!(handoffs(&pool, project_id, subject.id).await.len(), 1);
    assert_eq!(linked_sessions(&pool, subject.id).await, vec![session_id]);

    // The state event first, carrying the new hand-off, then the comment.
    let written = events_after(&pool, project_id, seq).await;
    let kinds: Vec<TaskEventKind> = written.iter().map(|event| event.kind).collect();
    assert_eq!(
        kinds,
        vec![TaskEventKind::StateChanged, TaskEventKind::Commented]
    );

    let state_changed = &written[0];
    assert_eq!(state_changed.actor, TaskActor::Session { session_id });
    assert_eq!(state_changed.from.as_deref(), Some("ready"));
    assert_eq!(state_changed.to.as_deref(), Some("review"));
    let in_event = state_changed.task.as_ref().expect("the task is carried");
    assert_eq!(
        in_event.handoff.as_ref().map(|handoff| handoff.id),
        Some(prepared.id)
    );
    assert_eq!(in_event.lease_holder_session_id, None);

    let commented = &written[1];
    let comment = commented.comment.as_ref().expect("the comment is carried");
    assert_eq!(comment.id, comments[0].id);
    assert_eq!(comment.body, "ready for review");
}

#[tokio::test]
async fn a_revision_by_a_user_is_created_by_that_user_and_links_the_source() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    // A user is not bound by anybody's lease, and the lease it clears is the
    // agent's (`SPEC.md`, "Tasks").
    let subject = task(&pool, project_id, "implement it", "ready").await;
    let subject = claim(&pool, project_id, subject.id, session_id).await;

    let prepared = prepared(&subject, Some(session_id), COMMIT, ReviewCarry::Fresh);
    let published = publish(
        &pool,
        project_id,
        HandoffCaller::User {
            user_id: fixture.user_id,
        },
        &prepared,
        move_to("review"),
    )
    .await
    .expect("the publication succeeds");

    let handoff = published.handoff.expect("the task has a hand-off");
    assert_eq!(handoff.created_by_user_id, Some(fixture.user_id));
    assert_eq!(handoff.created_by_session_id, None);
    assert_eq!(handoff.review_status, ReviewStatus::Unreviewed);

    let comments = comments(&pool, project_id, subject.id).await;
    assert_eq!(comments[0].author_user_id, Some(fixture.user_id));
    assert_eq!(comments[0].author_session_id, None);

    // The user is not a session, but the code still came from one.
    assert_eq!(linked_sessions(&pool, subject.id).await, vec![session_id]);
}

#[tokio::test]
async fn a_hand_off_carries_the_ordinary_field_updates_sent_with_it() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "implement it", "ready").await;
    let subject = claim(&pool, project_id, subject.id, session_id).await;
    let seq = last_seq(&pool, project_id).await;

    let prepared = prepared(&subject, Some(session_id), COMMIT, ReviewCarry::Fresh);
    let published = publish(
        &pool,
        project_id,
        HandoffCaller::Session { session_id },
        &prepared,
        UpdateTaskInput {
            title: Some("implement it properly".to_string()),
            labels: Some(vec!["backend".to_string()]),
            ..move_to("review")
        },
    )
    .await
    .expect("the publication succeeds");

    assert_eq!(published.title, "implement it properly");
    assert_eq!(published.labels, vec!["backend".to_string()]);
    assert_eq!(published.state, "review");

    // The fields moved before the task did, and the comment still comes last.
    let kinds: Vec<TaskEventKind> = events_after(&pool, project_id, seq)
        .await
        .iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            TaskEventKind::Updated,
            TaskEventKind::StateChanged,
            TaskEventKind::Commented,
        ]
    );
}

#[tokio::test]
async fn a_forward_with_a_decision_records_the_deciding_session() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let source_session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let reviewer_session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "review it", "review").await;
    let (subject, _) = current_handoff(
        &pool,
        project_id,
        &subject,
        Some(source_session_id),
        fixture.user_id,
        None,
    )
    .await;
    let subject = claim(&pool, project_id, subject.id, reviewer_session_id).await;

    let prepared = prepared(
        &subject,
        Some(source_session_id),
        COMMIT,
        ReviewCarry::Decision(ReviewDecision::Approved),
    );
    let published = publish(
        &pool,
        project_id,
        HandoffCaller::Session {
            session_id: reviewer_session_id,
        },
        &prepared,
        move_to("merge"),
    )
    .await
    .expect("the publication succeeds");

    let handoff = published.handoff.expect("the task has a hand-off");
    assert_eq!(handoff.review_status, ReviewStatus::Approved);
    assert_eq!(handoff.reviewed_by_session_id, Some(reviewer_session_id));
    assert_eq!(handoff.reviewed_by_user_id, None);
    assert!(handoff.reviewed_at.is_some());
    // The forwarder is the creator as well as the reviewer, and the commit is
    // the one it was passed.
    assert_eq!(handoff.created_by_session_id, Some(reviewer_session_id));
    assert_eq!(handoff.commit, COMMIT);

    // The record it forwarded is still there, still unreviewed: rows are
    // immutable (`docs/data-model.md`, `task_handoffs`).
    let all = handoffs(&pool, project_id, subject.id).await;
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].review_status, ReviewStatus::Unreviewed);

    // Both the source session and the forwarding one worked on this task.
    let mut linked = linked_sessions(&pool, subject.id).await;
    linked.sort();
    let mut expected = vec![source_session_id, reviewer_session_id];
    expected.sort();
    assert_eq!(linked, expected);
}

#[tokio::test]
async fn a_forward_without_a_decision_carries_the_attribution_forward() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let source_session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let reviewer_id = seed_user(&pool).await;

    let subject = task(&pool, project_id, "merge it", "merge").await;
    let (subject, previous) = current_handoff(
        &pool,
        project_id,
        &subject,
        Some(source_session_id),
        fixture.user_id,
        Some(reviewer_id),
    )
    .await;

    let prepared = prepared(
        &subject,
        previous.source_session_id,
        &previous.commit,
        ReviewCarry::CarriedFrom(Box::new(previous.clone())),
    );
    let published = publish(
        &pool,
        project_id,
        HandoffCaller::User {
            user_id: fixture.user_id,
        },
        &prepared,
        move_to("done"),
    )
    .await
    .expect("the publication succeeds");

    let handoff = published.handoff.expect("the task has a hand-off");
    assert_eq!(handoff.review_status, ReviewStatus::Approved);
    assert_eq!(handoff.reviewed_by_user_id, Some(reviewer_id));
    assert_eq!(handoff.reviewed_at, previous.reviewed_at);
    // The verdict is the reviewer's; the record is the forwarder's.
    assert_eq!(handoff.created_by_user_id, Some(fixture.user_id));
}

#[tokio::test]
async fn a_carried_decision_survives_the_reviewer_being_deleted() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let source_session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let reviewer_id = seed_user(&pool).await;

    let subject = task(&pool, project_id, "merge it", "merge").await;
    let (subject, previous) = current_handoff(
        &pool,
        project_id,
        &subject,
        Some(source_session_id),
        fixture.user_id,
        Some(reviewer_id),
    )
    .await;

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(reviewer_id)
        .execute(&pool)
        .await
        .expect("the reviewer is deleted");

    // `ON DELETE SET NULL` left the verdict and its time without a reviewer.
    let previous = TaskRepository::new(&pool)
        .find_handoff(project_id, previous.id)
        .await
        .expect("the record reads")
        .expect("the record is this project's");
    assert_eq!(previous.review_status, ReviewStatus::Approved);
    assert_eq!(previous.reviewed_by_user_id, None);
    assert!(previous.reviewed_at.is_some());

    let prepared = prepared(
        &subject,
        previous.source_session_id,
        &previous.commit,
        ReviewCarry::CarriedFrom(Box::new(previous.clone())),
    );
    let published = publish(
        &pool,
        project_id,
        HandoffCaller::User {
            user_id: fixture.user_id,
        },
        &prepared,
        move_to("done"),
    )
    .await
    .expect("the publication succeeds");

    let handoff = published.handoff.expect("the task has a hand-off");
    assert_eq!(handoff.review_status, ReviewStatus::Approved);
    assert_eq!(handoff.reviewed_by_user_id, None);
    assert_eq!(handoff.reviewed_by_session_id, None);
    assert_eq!(handoff.reviewed_at, previous.reviewed_at);
}

#[tokio::test]
async fn a_new_revision_never_carries_an_old_approval() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "revise it", "review").await;
    let (subject, previous) = current_handoff(
        &pool,
        project_id,
        &subject,
        Some(session_id),
        fixture.user_id,
        Some(fixture.user_id),
    )
    .await;
    let subject = claim(&pool, project_id, subject.id, session_id).await;

    // The same commit, published again as a revision.
    let prepared = prepared(
        &subject,
        Some(session_id),
        &previous.commit,
        ReviewCarry::Fresh,
    );
    let published = publish(
        &pool,
        project_id,
        HandoffCaller::Session { session_id },
        &prepared,
        move_to("ready"),
    )
    .await
    .expect("the publication succeeds");

    let handoff = published.handoff.expect("the task has a hand-off");
    assert_eq!(handoff.commit, previous.commit);
    assert_eq!(handoff.review_status, ReviewStatus::Unreviewed);
    assert_eq!(handoff.reviewed_by_user_id, None);
    assert_eq!(handoff.reviewed_at, None);
}

#[tokio::test]
async fn a_task_that_moved_since_preparation_is_a_conflict_that_writes_nothing() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "implement it", "ready").await;
    let subject = claim(&pool, project_id, subject.id, session_id).await;
    let prepared = prepared(&subject, Some(session_id), COMMIT, ReviewCarry::Fresh);

    // Somebody else moved it between the git half and this one.
    set_fields(
        &pool,
        project_id,
        subject.id,
        StateFields {
            state_id: Some(state_id(&pool, project_id, "needs_human").await),
            ..StateFields::default()
        },
    )
    .await;
    let seq = last_seq(&pool, project_id).await;

    let error = publish(
        &pool,
        project_id,
        HandoffCaller::Session { session_id },
        &prepared,
        move_to("review"),
    )
    .await
    .expect_err("the recheck fails");

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), HANDOFF_RECHECK_FAILED);

    assert!(comments(&pool, project_id, subject.id).await.is_empty());
    assert!(handoffs(&pool, project_id, subject.id).await.is_empty());
    assert!(events_after(&pool, project_id, seq).await.is_empty());
    let unchanged = read(&pool, project_id, subject.id).await;
    assert_eq!(unchanged.state, "needs_human");
    assert!(unchanged.handoff.is_none());
}

#[tokio::test]
async fn a_session_that_lost_the_lease_may_not_publish() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;
    let other_session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "implement it", "ready").await;
    let subject = claim(&pool, project_id, subject.id, session_id).await;
    let prepared = prepared(&subject, Some(session_id), COMMIT, ReviewCarry::Fresh);

    // The reaper released it and another session claimed it.
    claim(&pool, project_id, subject.id, other_session_id).await;

    let error = publish(
        &pool,
        project_id,
        HandoffCaller::Session { session_id },
        &prepared,
        move_to("review"),
    )
    .await
    .expect_err("the recheck fails");

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), HANDOFF_RECHECK_FAILED);
    assert!(handoffs(&pool, project_id, subject.id).await.is_empty());
}

#[tokio::test]
async fn a_forward_whose_current_hand_off_moved_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "review it", "review").await;
    let (subject, _) = current_handoff(
        &pool,
        project_id,
        &subject,
        Some(session_id),
        fixture.user_id,
        None,
    )
    .await;

    let prepared = prepared(&subject, Some(session_id), COMMIT, ReviewCarry::Fresh);

    // A revision landed first: the pointer is no longer the one prepared for.
    let (subject, _) = current_handoff(
        &pool,
        project_id,
        &subject,
        Some(session_id),
        fixture.user_id,
        None,
    )
    .await;

    let error = publish(
        &pool,
        project_id,
        HandoffCaller::User {
            user_id: fixture.user_id,
        },
        &prepared,
        move_to("merge"),
    )
    .await
    .expect_err("the recheck fails");

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), HANDOFF_RECHECK_FAILED);
    assert_eq!(handoffs(&pool, project_id, subject.id).await.len(), 2);
}

#[tokio::test]
async fn a_hand_off_into_a_terminal_state_closes_the_task_and_unblocks_its_dependants() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let project_id = fixture.project_id;
    let session_id = seed_session(&pool, project_id, fixture.profile_id).await;

    let subject = task(&pool, project_id, "implement it", "merge").await;
    let dependant = task(&pool, project_id, "build on it", "ready").await;

    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(&pool)
        .insert_dependency(
            mutation.conn(),
            project_id,
            dependant.id,
            subject.id,
            TaskDependencyKind::Blocks,
        )
        .await
        .expect("the edge inserts");
    recompute_blocked(&mut mutation, &[dependant.id])
        .await
        .expect("the flag is recomputed");
    mutation.commit().await.expect("the mutation commits");
    assert!(read(&pool, project_id, dependant.id).await.blocked);

    let subject = claim(&pool, project_id, subject.id, session_id).await;
    let seq = last_seq(&pool, project_id).await;

    let prepared = prepared(&subject, Some(session_id), OTHER_COMMIT, ReviewCarry::Fresh);
    let published = publish(
        &pool,
        project_id,
        HandoffCaller::Session { session_id },
        &prepared,
        move_to("done"),
    )
    .await
    .expect("the publication succeeds");

    assert_eq!(published.state, "done");
    assert!(published.closed_at.is_some());
    // Closing with a final revision is allowed: the record is still written.
    assert_eq!(
        published.handoff.map(|handoff| handoff.id),
        Some(prepared.id)
    );
    assert!(!read(&pool, project_id, dependant.id).await.blocked);

    let kinds: Vec<TaskEventKind> = events_after(&pool, project_id, seq)
        .await
        .iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            TaskEventKind::StateChanged,
            TaskEventKind::Unblocked,
            TaskEventKind::Commented,
        ]
    );
}
