//! `SessionRepository` against a real Postgres (`CLAUDE.md`, "Testing
//! expectations").
//!
//! The CRUD half is a round trip; the interesting half is the contract
//! `docs/data-model.md`, `events` and ADR 0021 put on the append path:
//!
//! - sequences come from the table under a session row lock, so concurrent
//!   writers produce one unbroken run with no gaps and no duplicates,
//! - a writer for a *different* session never waits for that lock,
//! - and the `pg_notify` that announces a batch is issued inside the writing
//!   transaction, so it is delivered exactly once on commit and not at all on
//!   rollback (ADR 0028's acceptance list).
//!
//! A lock quietly dropped from the append statement would pass every
//! functional assertion and fail here.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use axum::http::StatusCode;
use mars_orchestrator::events::{AgentEvent, AgentEventBody, StopSignal};
use mars_orchestrator::models::{
    EventRow, NewEvent, NewSession, ProfileKind, SessionError, SessionState, SessionTitle,
    StateChange, session_branch,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{AppendedRange, CostDelta, SessionRepository, Transition};
use serde_json::json;
use sqlx::postgres::PgListener;
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the seeded user would carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// How long a blocked transaction is given to prove it is blocked.
const BLOCKED_FOR: Duration = Duration::from_millis(400);

/// How long an unblocked transaction — or an expected notification — is given
/// once nothing should be in its way.
const UNBLOCKED_WITHIN: Duration = Duration::from_secs(10);

/// How many writers the concurrency test runs, and how many events each one
/// appends in its own transaction.
const WRITERS: usize = 8;
const EVENTS_PER_WRITER: usize = 25;

/// The rows a session needs to exist at all.
///
/// Seeded with unchecked statements rather than through their repositories:
/// `ProjectRepository` and the profile models are being written in parallel,
/// and this file only needs the foreign keys to resolve.
struct Fixture {
    user_id: Uuid,
    project_id: Uuid,
    profile_id: Uuid,
}

async fn seed(pool: &PgPool) -> Fixture {
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, $2, $3, $4)")
        .bind(user_id)
        .bind(format!("user-{}", &user_id.simple().to_string()[..8]))
        .bind(format!("{user_id}@example.test"))
        .bind(FAKE_PASSWORD_HASH)
        .execute(pool)
        .await
        .expect("the user seeds");

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        .bind("https://git.example.test/mars.git")
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
    .bind("localhost/mars-session:test")
    .execute(pool)
    .await
    .expect("the profile seeds");

    Fixture {
        user_id,
        project_id,
        profile_id,
    }
}

/// A conversational session on the fixture's project, with a distinct fake
/// token hash (rule 3).
fn new_session(fixture: &Fixture) -> NewSession {
    let mut session = NewSession::new(
        fixture.project_id,
        fixture.profile_id,
        ProfileKind::Conversational,
        "main",
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );
    session.created_by = Some(fixture.user_id);

    session
}

/// Insert `session` in its own committed transaction.
async fn insert(pool: &PgPool, session: &NewSession) -> Uuid {
    let repository = SessionRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let inserted = repository
        .insert(&mut tx, session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    inserted.id
}

/// Append `events` in its own committed transaction.
async fn append(pool: &PgPool, session_id: Uuid, events: &[NewEvent]) -> Vec<i64> {
    let repository = SessionRepository::new(pool);
    let mut tx = pool.begin().await.expect("a transaction begins");
    let sequences = repository
        .append_events(&mut tx, session_id, events)
        .await
        .expect("the events append");
    tx.commit().await.expect("the transaction commits");

    sequences
}

fn text_event(text: &str) -> NewEvent {
    NewEvent::now("text", json!({ "text": text }))
}

/// One translated `text` event, the shape the owner appends per native line.
fn agent_event(text: &str) -> AgentEvent {
    AgentEvent::new(AgentEventBody::Text {
        text: text.to_string(),
    })
}

/// The next `count` notifications as `(channel, payload)` pairs, in the order
/// PostgreSQL delivers them.
async fn notifications(listener: &mut PgListener, count: usize) -> Vec<(String, String)> {
    let mut received = Vec::with_capacity(count);
    for _ in 0..count {
        let notification = tokio::time::timeout(UNBLOCKED_WITHIN, listener.recv())
            .await
            .expect("a committed write notifies")
            .unwrap();
        received.push((
            notification.channel().to_string(),
            notification.payload().to_string(),
        ));
    }

    received
}

fn text_events(count: usize) -> Vec<NewEvent> {
    (0..count)
        .map(|index| text_event(&format!("line {index}")))
        .collect()
}

#[tokio::test]
async fn a_session_survives_an_insert_find_list_update_delete_round_trip() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let new = new_session(&fixture)
        .with_title(Some("  fix the parser  "))
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let inserted = repository.insert(&mut tx, &new).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(inserted.id, new.id);
    assert_eq!(inserted.project_id, fixture.project_id);
    assert_eq!(inserted.profile_id, fixture.profile_id);
    assert_eq!(inserted.created_by, Some(fixture.user_id));
    assert_eq!(inserted.kind, ProfileKind::Conversational);
    // Trimmed by the model, stored as trimmed.
    assert_eq!(inserted.title.as_deref(), Some("fix the parser"));
    assert_eq!(inserted.base_ref, "main");
    assert_eq!(inserted.branch, session_branch(new.id));
    assert_eq!(inserted.mcp_token_hash, new.mcp_token_hash);
    // Column defaults the repository does not set.
    assert_eq!(inserted.state, SessionState::Creating);
    assert_eq!(inserted.last_seq, 0);
    assert_eq!(inserted.cost_usd, 0.0);
    assert_eq!(inserted.input_tokens, 0);
    assert_eq!(inserted.output_tokens, 0);
    assert!(inserted.container_id.is_none());
    assert!(inserted.cli_session_id.is_none());
    assert!(inserted.task_id.is_none());
    assert!(inserted.handoff_id.is_none());
    assert!(inserted.error.is_none());
    assert!(inserted.parked_at.is_none());
    assert!(inserted.ended_at.is_none());

    assert_eq!(
        repository.find(new.id).await.unwrap().as_ref(),
        Some(&inserted)
    );
    assert_eq!(
        repository
            .find_in_project(fixture.project_id, new.id)
            .await
            .unwrap()
            .as_ref(),
        Some(&inserted),
    );
    // The scope is in the WHERE clause: another project's id finds nothing.
    assert!(
        repository
            .find_in_project(Uuid::new_v4(), new.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(repository.find(Uuid::new_v4()).await.unwrap().is_none());

    // A second, newer session sorts first.
    let other = new_session(&fixture);
    let other_id = insert(&pool, &other).await;

    let listed = repository
        .list_by_project(fixture.project_id, None)
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|session| session.id).collect::<Vec<_>>(),
        [other_id, new.id],
    );
    assert_eq!(
        repository
            .list_all(None)
            .await
            .unwrap()
            .iter()
            .map(|session| session.id)
            .collect::<Vec<_>>(),
        [other_id, new.id],
    );
    assert_eq!(
        repository
            .list_by_project(fixture.project_id, Some(SessionState::Creating))
            .await
            .unwrap()
            .len(),
        2,
    );
    assert!(
        repository
            .list_by_project(fixture.project_id, Some(SessionState::Running))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repository
            .list_all(Some(SessionState::Done))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        repository
            .list_by_project(Uuid::new_v4(), None)
            .await
            .unwrap()
            .is_empty()
    );

    let mut tx = pool.begin().await.unwrap();
    let title = SessionTitle::parse("renamed").unwrap();
    let retitled = repository
        .update_title(&mut tx, new.id, Some(&title))
        .await
        .unwrap()
        .expect("the session is there");
    assert!(
        repository
            .set_container_id(&mut tx, new.id, Some("fake-container-id"))
            .await
            .unwrap()
    );
    assert!(
        repository
            .set_cli_session_id(&mut tx, new.id, "fake-cli-session-id")
            .await
            .unwrap()
    );
    assert!(
        repository
            .set_mcp_token_hash(&mut tx, new.id, "fake-mcp-token-hash-rotated")
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    assert_eq!(retitled.title.as_deref(), Some("renamed"));
    let stored = repository.find(new.id).await.unwrap().unwrap();
    assert_eq!(stored.container_id.as_deref(), Some("fake-container-id"));
    assert_eq!(
        stored.cli_session_id.as_deref(),
        Some("fake-cli-session-id")
    );
    assert_eq!(stored.mcp_token_hash, "fake-mcp-token-hash-rotated");

    // Clearing the title and the container id are both the `None` case.
    let mut tx = pool.begin().await.unwrap();
    let cleared = repository
        .update_title(&mut tx, new.id, None)
        .await
        .unwrap()
        .unwrap();
    assert!(
        repository
            .set_container_id(&mut tx, new.id, None)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert!(cleared.title.is_none());
    assert!(
        repository
            .find(new.id)
            .await
            .unwrap()
            .unwrap()
            .container_id
            .is_none()
    );

    // A missing session is `false`/`None` everywhere, never an error.
    let mut tx = pool.begin().await.unwrap();
    assert!(
        repository
            .update_title(&mut tx, Uuid::new_v4(), None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !repository
            .set_container_id(&mut tx, Uuid::new_v4(), None)
            .await
            .unwrap()
    );
    assert!(
        !repository
            .set_cli_session_id(&mut tx, Uuid::new_v4(), "fake-cli-session-id")
            .await
            .unwrap()
    );
    assert!(!repository.delete(&mut tx, Uuid::new_v4()).await.unwrap());
    tx.commit().await.unwrap();

    // Events cascade with the session row.
    append(&pool, new.id, &text_events(2)).await;
    let mut tx = pool.begin().await.unwrap();
    assert!(repository.delete(&mut tx, new.id).await.unwrap());
    tx.commit().await.unwrap();

    assert!(repository.find(new.id).await.unwrap().is_none());
    assert_eq!(repository.max_seq(new.id).await.unwrap(), 0);
    assert_eq!(
        repository
            .list_by_project(fixture.project_id, None)
            .await
            .unwrap()
            .len(),
        1,
    );
}

#[tokio::test]
async fn a_branch_that_is_not_the_sessions_own_never_reaches_the_database() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let mut new = new_session(&fixture);
    new.branch = "main".to_string();

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert(&mut tx, &new)
        .await
        .expect_err("the branch must be rejected");
    tx.rollback().await.unwrap();

    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(error.to_string(), SessionError::InvalidBranch.to_string());
    assert!(repository.find(new.id).await.unwrap().is_none());
}

#[tokio::test]
async fn a_duplicate_mcp_token_hash_is_an_internal_error() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let first = new_session(&fixture);
    insert(&pool, &first).await;

    let mut second = new_session(&fixture);
    second.mcp_token_hash = first.mcp_token_hash.clone();

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .insert(&mut tx, &second)
        .await
        .expect_err("a reused token hash is a defect");
    tx.rollback().await.unwrap();

    // Never a 409: the hash is of a random token the caller never chose.
    assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(matches!(error, Error::Internal(_)));

    // And on rotation, through the same mapping.
    let other = new_session(&fixture);
    let other_id = insert(&pool, &other).await;
    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .set_mcp_token_hash(&mut tx, other_id, &first.mcp_token_hash)
        .await
        .expect_err("a reused token hash is a defect");
    tx.rollback().await.unwrap();
    assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn appending_numbers_events_from_one_and_caches_the_highest() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let new = new_session(&fixture);
    let id = insert(&pool, &new).await;

    // An empty batch is a no-op: no sequences, no notification, nothing moved.
    let before = repository.find(id).await.unwrap().unwrap();
    assert_eq!(append(&pool, id, &[]).await, Vec::<i64>::new());
    let after = repository.find(id).await.unwrap().unwrap();
    assert_eq!(after.last_seq, 0);
    assert_eq!(after.last_activity_at, before.last_activity_at);

    assert_eq!(append(&pool, id, &text_events(3)).await, [1, 2, 3]);
    assert_eq!(append(&pool, id, &[text_event("later")]).await, [4]);

    let stored = repository.find(id).await.unwrap().unwrap();
    assert_eq!(stored.last_seq, 4);
    assert_eq!(stored.last_seq, repository.max_seq(id).await.unwrap());
    assert!(stored.last_activity_at > before.last_activity_at);

    let (events, has_more) = repository.list_events(id, None, 100).await.unwrap();
    assert!(!has_more);
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    assert!(events.iter().all(|event| event.session_id == id));
    assert!(events.iter().all(|event| event.kind == "text"));
    assert_eq!(events[3].payload, json!({ "text": "later" }));
}

#[tokio::test]
async fn appending_to_a_session_that_is_gone_is_not_found() {
    let (_postgres, pool) = common::db::test_pool().await;
    let _fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .append_events(&mut tx, Uuid::new_v4(), &[text_event("hello")])
        .await
        .expect_err("there is no such session");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);

    let error = repository
        .lock_session(&mut tx, Uuid::new_v4())
        .await
        .expect_err("there is no such session");
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn concurrent_writers_produce_one_unbroken_sequence() {
    // Eight writers plus the test's own connection (`CLAUDE.md`, "Testing
    // expectations"; the coordinator's pool sizing).
    let (_postgres, pool) = common::db::test_pool_with(12).await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let new = new_session(&fixture);
    let id = insert(&pool, &new).await;

    let mut writers = Vec::with_capacity(WRITERS);
    for writer in 0..WRITERS {
        let pool = pool.clone();
        writers.push(tokio::spawn(async move {
            let repository = SessionRepository::new(&pool);
            let events: Vec<NewEvent> = (0..EVENTS_PER_WRITER)
                .map(|index| text_event(&format!("writer {writer} line {index}")))
                .collect();

            let mut tx = pool.begin().await.expect("a transaction begins");
            let sequences = repository
                .append_events(&mut tx, id, &events)
                .await
                .expect("the events append");
            tx.commit().await.expect("the transaction commits");

            sequences
        }));
    }

    let mut all = Vec::new();
    for writer in writers {
        let sequences = writer.await.expect("the writer finishes");
        assert_eq!(sequences.len(), EVENTS_PER_WRITER);
        // Each writer held the row lock for its whole batch, so its own
        // sequences are consecutive.
        let consecutive: Vec<i64> =
            (sequences[0]..sequences[0] + EVENTS_PER_WRITER as i64).collect();
        assert_eq!(sequences, consecutive, "a batch was interleaved");
        all.extend(sequences);
    }

    let total = (WRITERS * EVENTS_PER_WRITER) as i64;
    let unique: BTreeSet<i64> = all.iter().copied().collect();
    assert_eq!(unique.len(), all.len(), "a sequence was handed out twice");
    assert_eq!(
        unique.into_iter().collect::<Vec<_>>(),
        (1..=total).collect::<Vec<_>>(),
        "the sequence has a gap",
    );

    assert_eq!(repository.max_seq(id).await.unwrap(), total);
    assert_eq!(
        repository.find(id).await.unwrap().unwrap().last_seq,
        total,
        "last_seq is not the highest committed sequence",
    );
    let (events, _) = repository.list_events(id, None, 500).await.unwrap();
    assert_eq!(events.len(), total as usize);
}

#[tokio::test]
async fn a_writer_waits_for_the_same_session_but_not_for_another() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let first = new_session(&fixture);
    let first_id = insert(&pool, &first).await;
    let second = new_session(&fixture);
    let second_id = insert(&pool, &second).await;

    // Hold the lock on the first session.
    let mut holder = pool.begin().await.unwrap();
    assert_eq!(
        repository
            .append_events(&mut holder, first_id, &[text_event("held")])
            .await
            .unwrap(),
        [1],
    );

    // A writer for the *other* session must not wait for it.
    let other = {
        let pool = pool.clone();
        tokio::spawn(async move {
            let repository = SessionRepository::new(&pool);
            let mut tx = pool.begin().await.unwrap();
            let sequences = repository
                .append_events(&mut tx, second_id, &[text_event("unrelated")])
                .await
                .unwrap();
            tx.commit().await.unwrap();
            sequences
        })
    };
    assert_eq!(
        tokio::time::timeout(UNBLOCKED_WITHIN, other)
            .await
            .expect("a writer for another session must not block")
            .unwrap(),
        [1],
    );

    // A writer for the same session waits until the holder commits.
    let blocked = {
        let pool = pool.clone();
        tokio::spawn(async move {
            let repository = SessionRepository::new(&pool);
            let mut tx = pool.begin().await.unwrap();
            let sequences = repository
                .append_events(&mut tx, first_id, &[text_event("after the lock")])
                .await
                .unwrap();
            tx.commit().await.unwrap();
            sequences
        })
    };
    let mut blocked = std::pin::pin!(blocked);
    assert!(
        tokio::time::timeout(BLOCKED_FOR, &mut blocked)
            .await
            .is_err(),
        "the second writer did not wait for the session row lock",
    );

    holder.commit().await.unwrap();

    assert_eq!(
        tokio::time::timeout(UNBLOCKED_WITHIN, blocked)
            .await
            .expect("the writer runs once the lock is free")
            .unwrap(),
        [2],
        "the waiting writer did not see the committed event",
    );
    assert_eq!(repository.max_seq(first_id).await.unwrap(), 2);
}

#[tokio::test]
async fn a_batch_notifies_once_on_commit_and_never_on_rollback() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let new = new_session(&fixture);
    let id = insert(&pool, &new).await;

    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener.listen("session_events").await.unwrap();

    append(&pool, id, &text_events(3)).await;
    let notification = tokio::time::timeout(UNBLOCKED_WITHIN, listener.recv())
        .await
        .expect("a committed batch notifies")
        .unwrap();
    assert_eq!(notification.channel(), "session_events");
    // One notification per batch, carrying its highest sequence.
    assert_eq!(notification.payload(), format!("{id}:3"));

    // A rolled-back batch publishes neither its rows nor its notification
    // (ADR 0028).
    let mut tx = pool.begin().await.unwrap();
    repository
        .append_events(&mut tx, id, &text_events(2))
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    assert!(
        tokio::time::timeout(BLOCKED_FOR, listener.recv())
            .await
            .is_err(),
        "a rolled-back batch notified anyway",
    );
    assert_eq!(repository.max_seq(id).await.unwrap(), 3);

    // The next committed batch reuses the sequence the rollback gave back, and
    // its notification is the next one delivered — so the rolled-back one was
    // never queued behind it either.
    append(&pool, id, &[text_event("after the rollback")]).await;
    let notification = tokio::time::timeout(UNBLOCKED_WITHIN, listener.recv())
        .await
        .expect("a committed batch notifies")
        .unwrap();
    assert_eq!(notification.payload(), format!("{id}:4"));
    assert!(
        tokio::time::timeout(BLOCKED_FOR, listener.recv())
            .await
            .is_err(),
        "a batch notified more than once",
    );
}

#[tokio::test]
async fn a_state_change_sets_its_timestamps_and_notifies() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let new = new_session(&fixture);
    let id = insert(&pool, &new).await;

    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener.listen("session_state").await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let running = repository
        .set_state(&mut tx, id, SessionState::Running, &StateChange::plain())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(running.state, SessionState::Running);
    assert!(running.parked_at.is_none());
    assert!(running.ended_at.is_none());
    let notification = tokio::time::timeout(UNBLOCKED_WITHIN, listener.recv())
        .await
        .expect("a state change notifies")
        .unwrap();
    assert_eq!(notification.channel(), "session_state");
    assert_eq!(notification.payload(), format!("{id}:running"));

    let mut tx = pool.begin().await.unwrap();
    let parked = repository
        .set_state(&mut tx, id, SessionState::Parked, &StateChange::plain())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(parked.parked_at.is_some(), "parked_at was not set");
    assert!(parked.ended_at.is_none());
    assert_eq!(
        tokio::time::timeout(UNBLOCKED_WITHIN, listener.recv())
            .await
            .unwrap()
            .unwrap()
            .payload(),
        format!("{id}:parked"),
    );

    let mut tx = pool.begin().await.unwrap();
    let failed = repository
        .set_state(
            &mut tx,
            id,
            SessionState::Failed,
            &StateChange::failed("container exited 137"),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(failed.state, SessionState::Failed);
    assert_eq!(failed.error.as_deref(), Some("container exited 137"));
    assert!(failed.ended_at.is_some(), "ended_at was not set");
    assert_eq!(failed.parked_at, parked.parked_at);
    assert_eq!(
        tokio::time::timeout(UNBLOCKED_WITHIN, listener.recv())
            .await
            .unwrap()
            .unwrap()
            .payload(),
        format!("{id}:failed"),
    );

    // A rolled-back state change publishes neither the row nor the
    // notification.
    let mut tx = pool.begin().await.unwrap();
    repository
        .set_state(&mut tx, id, SessionState::Parked, &StateChange::plain())
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    assert!(
        tokio::time::timeout(BLOCKED_FOR, listener.recv())
            .await
            .is_err(),
        "a rolled-back state change notified anyway",
    );
    assert_eq!(
        repository.find(id).await.unwrap().unwrap().state,
        SessionState::Failed,
    );
}

#[tokio::test]
async fn a_transition_the_diagram_does_not_have_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let new = new_session(&fixture);
    let id = insert(&pool, &new).await;

    // creating -> running -> done, then nothing.
    let mut tx = pool.begin().await.unwrap();
    repository
        .set_state(&mut tx, id, SessionState::Running, &StateChange::plain())
        .await
        .unwrap();
    let done = repository
        .set_state(&mut tx, id, SessionState::Done, &StateChange::plain())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(done.ended_at.is_some());

    for to in [
        SessionState::Running,
        SessionState::Parked,
        SessionState::Failed,
        SessionState::Creating,
        // No self-loops: `done -> done` is refused too.
        SessionState::Done,
    ] {
        let mut tx = pool.begin().await.unwrap();
        let error = repository
            .set_state(&mut tx, id, to, &StateChange::plain())
            .await
            .expect_err("done is final");
        tx.rollback().await.unwrap();

        assert_eq!(error.status(), StatusCode::CONFLICT, "done -> {to}");
        assert_eq!(
            error.to_string(),
            SessionError::InvalidTransition {
                from: SessionState::Done,
                to,
            }
            .to_string(),
        );
    }

    assert_eq!(
        repository.find(id).await.unwrap().unwrap().state,
        SessionState::Done,
    );

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .set_state(
            &mut tx,
            Uuid::new_v4(),
            SessionState::Running,
            &StateChange::plain(),
        )
        .await
        .expect_err("there is no such session");
    tx.rollback().await.unwrap();
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_events_pages_backwards_from_before() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let new = new_session(&fixture);
    let id = insert(&pool, &new).await;
    append(&pool, id, &text_events(5)).await;

    let seqs = |events: Vec<EventRow>| {
        events
            .into_iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>()
    };

    // The newest page first, oldest-first within the page.
    let (page, has_more) = repository.list_events(id, None, 2).await.unwrap();
    assert_eq!(seqs(page), [4, 5]);
    assert!(has_more);

    let (page, has_more) = repository.list_events(id, Some(4), 2).await.unwrap();
    assert_eq!(seqs(page), [2, 3]);
    assert!(has_more);

    let (page, has_more) = repository.list_events(id, Some(2), 2).await.unwrap();
    assert_eq!(seqs(page), [1]);
    assert!(!has_more);

    // `before` is exclusive, so the oldest event has nothing before it.
    let (page, has_more) = repository.list_events(id, Some(1), 10).await.unwrap();
    assert!(page.is_empty());
    assert!(!has_more);

    // The limit is clamped to 1..=500 at both ends.
    let (page, has_more) = repository.list_events(id, None, 0).await.unwrap();
    assert_eq!(seqs(page), [5]);
    assert!(has_more);
    let (page, has_more) = repository.list_events(id, None, 10_000).await.unwrap();
    assert_eq!(seqs(page), [1, 2, 3, 4, 5]);
    assert!(!has_more);

    // Replay from a cursor.
    assert_eq!(
        seqs(repository.list_events_after(id, 3).await.unwrap()),
        [4, 5]
    );
    assert_eq!(
        seqs(repository.list_events_after(id, 0).await.unwrap()),
        [1, 2, 3, 4, 5],
    );
    assert!(
        repository
            .list_events_after(id, 5)
            .await
            .unwrap()
            .is_empty()
    );

    // Another session's stream is untouched by both.
    let other_id = insert(&pool, &new_session(&fixture)).await;
    assert!(
        repository
            .list_events(other_id, None, 10)
            .await
            .unwrap()
            .0
            .is_empty()
    );
    assert!(
        repository
            .list_events_after(other_id, 0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn max_seq_and_max_offset_read_the_stream() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let new = new_session(&fixture);
    let id = insert(&pool, &new).await;

    // An empty stream: no sequence, no offset.
    assert_eq!(repository.max_seq(id).await.unwrap(), 0);
    assert_eq!(repository.max_offset(id).await.unwrap(), None);

    append(
        &pool,
        id,
        &[
            text_event("no offset yet"),
            text_event("first line").with_offset(120),
            text_event("second line").with_offset(4096),
        ],
    )
    .await;

    assert_eq!(repository.max_seq(id).await.unwrap(), 3);
    assert_eq!(repository.max_offset(id).await.unwrap(), Some(4096));

    // The offset is internal and is stripped on the way out.
    let (events, _) = repository.list_events(id, None, 10).await.unwrap();
    assert_eq!(events[2].payload["_offset"], json!(4096));
    assert_eq!(events[2].public_payload(), json!({ "text": "second line" }),);

    // Offsets are per session.
    let other_id = insert(&pool, &new_session(&fixture)).await;
    assert_eq!(repository.max_offset(other_id).await.unwrap(), None);
    assert_eq!(repository.max_seq(other_id).await.unwrap(), 0);
}

#[tokio::test]
async fn usage_accumulates_and_refuses_to_go_backwards() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let new = new_session(&fixture);
    let id = insert(&pool, &new).await;

    let mut tx = pool.begin().await.unwrap();
    repository
        .add_usage(&mut tx, id, 0.25, 1_000, 200)
        .await
        .unwrap();
    repository
        .add_usage(&mut tx, id, 0.75, 500, 100)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let stored = repository.find(id).await.unwrap().unwrap();
    assert!(
        (stored.cost_usd - 1.0).abs() < f64::EPSILON,
        "{}",
        stored.cost_usd
    );
    assert_eq!(stored.input_tokens, 1_500);
    assert_eq!(stored.output_tokens, 300);

    let mut tx = pool.begin().await.unwrap();
    for (cost, input, output) in [
        (-0.01, 0, 0),
        (0.0, -1, 0),
        (0.0, 0, -1),
        (f64::NAN, 0, 0),
        (f64::INFINITY, 0, 0),
    ] {
        let error = repository
            .add_usage(&mut tx, id, cost, input, output)
            .await
            .expect_err("a counter may not move backwards");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error.to_string(), SessionError::InvalidUsage.to_string());
    }
    tx.commit().await.unwrap();

    let unchanged = repository.find(id).await.unwrap().unwrap();
    assert_eq!(unchanged.input_tokens, 1_500);
    assert_eq!(unchanged.output_tokens, 300);
}

#[tokio::test]
async fn the_scoped_reads_answer_not_found_and_filter_by_state() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let id = insert(&pool, &new_session(&fixture)).await;

    assert_eq!(repository.get(id).await.unwrap().id, id);
    assert_eq!(
        repository
            .get_in_project(fixture.project_id, id)
            .await
            .unwrap()
            .id,
        id,
    );

    // A missing session, and another project's session, are both 404.
    for error in [
        repository
            .get(Uuid::new_v4())
            .await
            .expect_err("no session"),
        repository
            .get_in_project(Uuid::new_v4(), id)
            .await
            .expect_err("not this project's session"),
    ] {
        assert_eq!(error.status(), StatusCode::NOT_FOUND);
    }

    assert_eq!(
        repository
            .list_by_state(SessionState::Creating)
            .await
            .unwrap()
            .iter()
            .map(|session| session.id)
            .collect::<Vec<_>>(),
        [id],
    );
    assert!(
        repository
            .list_by_state(SessionState::Running)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_transition_writes_its_event_and_notifies_both_channels() {
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let id = insert(&pool, &new_session(&fixture)).await;

    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener
        .listen_all(["session_state", "session_events"])
        .await
        .unwrap();

    // creating -> running: the event, the row and both notifications in one
    // transaction.
    let mut tx = pool.begin().await.unwrap();
    let running = repository
        .transition(
            &mut tx,
            id,
            &Transition::new(
                SessionState::Creating,
                SessionState::Running,
                "container started",
            ),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(running.state, SessionState::Running);
    assert_eq!(running.last_seq, 1);
    assert!(running.parked_at.is_none());
    assert!(running.ended_at.is_none());

    let (events, has_more) = repository.list_events(id, None, 10).await.unwrap();
    assert!(!has_more);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "state_change");
    assert_eq!(
        events[0].payload,
        json!({ "from": "creating", "to": "running", "reason": "container started" }),
    );

    let notified = notifications(&mut listener, 2).await;
    assert_eq!(
        notified,
        [
            ("session_state".to_string(), format!("{id}:running")),
            ("session_events".to_string(), format!("{id}:1")),
        ],
    );

    // running -> parked with the signal that stopped it.
    let mut tx = pool.begin().await.unwrap();
    let parked = repository
        .transition(
            &mut tx,
            id,
            &Transition::new(SessionState::Running, SessionState::Parked, "stopped")
                .with_signal(StopSignal::Sigint),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(parked.parked_at.is_some(), "parked_at was not set");
    assert!(parked.ended_at.is_none());
    let (events, _) = repository.list_events(id, Some(3), 10).await.unwrap();
    assert_eq!(
        events[1].payload,
        json!({
            "from": "running", "to": "parked", "reason": "stopped", "signal": "SIGINT",
        }),
    );
    let _ = notifications(&mut listener, 2).await;

    // parked -> failed with no explicit error: the reason is the error.
    let mut tx = pool.begin().await.unwrap();
    let failed = repository
        .transition(
            &mut tx,
            id,
            &Transition::new(
                SessionState::Parked,
                SessionState::Failed,
                "relaunch failed",
            ),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(failed.error.as_deref(), Some("relaunch failed"));
    assert!(failed.ended_at.is_some(), "ended_at was not set");
    assert_eq!(failed.parked_at, parked.parked_at);
    let _ = notifications(&mut listener, 2).await;

    // failed -> parked is the retry: it clears both the error and the end.
    let mut tx = pool.begin().await.unwrap();
    let retried = repository
        .transition(
            &mut tx,
            id,
            &Transition::new(SessionState::Failed, SessionState::Parked, "user retried"),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(retried.state, SessionState::Parked);
    assert!(retried.error.is_none(), "the retry kept the error");
    assert!(retried.ended_at.is_none(), "the retry kept ended_at");
    assert!(retried.parked_at.is_some());
    let _ = notifications(&mut listener, 2).await;

    // A rolled-back transition publishes neither the row, the event nor either
    // notification (ADR 0028).
    let mut tx = pool.begin().await.unwrap();
    repository
        .transition(
            &mut tx,
            id,
            &Transition::new(SessionState::Parked, SessionState::Done, "user ended it"),
        )
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    assert!(
        tokio::time::timeout(BLOCKED_FOR, listener.recv())
            .await
            .is_err(),
        "a rolled-back transition notified anyway",
    );
    let stored = repository.get(id).await.unwrap();
    assert_eq!(stored.state, SessionState::Parked);
    assert_eq!(repository.max_seq(id).await.unwrap(), 4);
    assert_eq!(stored.last_seq, 4);
}

#[tokio::test]
async fn a_transition_from_a_state_the_session_left_is_a_conflict() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let id = insert(&pool, &new_session(&fixture)).await;

    // The caller read `running` from a stale snapshot; the row says `creating`.
    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .transition(
            &mut tx,
            id,
            &Transition::new(SessionState::Running, SessionState::Parked, "stopped"),
        )
        .await
        .expect_err("the session is not running");
    tx.rollback().await.unwrap();

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "session is creating");

    // `from` right, edge missing: the model's own refusal. A session that has
    // never run cannot be parked; ending it, which is an edge, is
    // `a_creating_session_is_ended_over_its_own_edge` below.
    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .transition(
            &mut tx,
            id,
            &Transition::new(SessionState::Creating, SessionState::Parked, "parked"),
        )
        .await
        .expect_err("creating -> parked is not an edge");
    tx.rollback().await.unwrap();

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(
        error.to_string(),
        SessionError::InvalidTransition {
            from: SessionState::Creating,
            to: SessionState::Parked,
        }
        .to_string(),
    );

    // Neither attempt changed the row or wrote an event.
    let stored = repository.get(id).await.unwrap();
    assert_eq!(stored.state, SessionState::Creating);
    assert_eq!(stored.last_seq, 0);
    assert_eq!(repository.max_seq(id).await.unwrap(), 0);

    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .transition(
            &mut tx,
            Uuid::new_v4(),
            &Transition::new(SessionState::Creating, SessionState::Running, "started"),
        )
        .await
        .expect_err("there is no such session");
    tx.rollback().await.unwrap();
    assert_eq!(error.status(), StatusCode::NOT_FOUND);
}

/// The edge a user's end of a session that is still `creating` takes: the row
/// closes `done`, `ended_at` is stamped and the transcript records the change
/// like any other (`ARCHITECTURE.md`, "Session lifecycle"; task `qhyhw`).
#[tokio::test]
async fn a_creating_session_is_ended_over_its_own_edge() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let id = insert(&pool, &new_session(&fixture)).await;

    let mut tx = pool.begin().await.unwrap();
    let ended = repository
        .transition(
            &mut tx,
            id,
            &Transition::new(SessionState::Creating, SessionState::Done, "ended by user"),
        )
        .await
        .expect("creating -> done is an edge");
    tx.commit().await.unwrap();

    assert_eq!(ended.state, SessionState::Done);
    assert!(ended.ended_at.is_some());
    assert!(ended.error.is_none());
    assert_eq!(repository.max_seq(id).await.unwrap(), 1);
}

#[tokio::test]
async fn a_native_line_commits_its_events_offset_and_counters_together() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let id = insert(&pool, &new_session(&fixture)).await;
    let before = repository.get(id).await.unwrap().last_activity_at;

    let line = [
        agent_event("first"),
        agent_event("second"),
        agent_event("third"),
    ];

    let mut tx = pool.begin().await.unwrap();
    let range = repository
        .append_native_line(
            &mut tx,
            id,
            &line,
            512,
            0,
            Some(CostDelta {
                cost_usd: 0.25,
                input_tokens: 1_000,
                output_tokens: 200,
            }),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        range,
        AppendedRange {
            first_seq: 1,
            last_seq: 3,
        },
    );

    let (events, _) = repository.list_events(id, None, 10).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2, 3],
    );
    // Only the last event of the line carries the offset, so the offset advances
    // exactly once per line (`ARCHITECTURE.md`, "Durability and recovery").
    assert_eq!(events[0].payload, json!({ "text": "first" }));
    assert_eq!(events[1].payload, json!({ "text": "second" }));
    assert_eq!(
        events[2].payload,
        json!({ "text": "third", "_offset": 512 })
    );

    let stored = repository.get(id).await.unwrap();
    assert_eq!(stored.last_seq, 3);
    assert!(stored.last_activity_at > before);
    assert!(
        (stored.cost_usd - 0.25).abs() < f64::EPSILON,
        "{}",
        stored.cost_usd
    );
    assert_eq!(stored.input_tokens, 1_000);
    assert_eq!(stored.output_tokens, 200);
    assert_eq!(repository.max_offset(id).await.unwrap(), Some(512));

    // A second line picks up from the offset the first committed, and a line
    // without a `result` leaves the counters alone.
    let mut tx = pool.begin().await.unwrap();
    let range = repository
        .append_native_line(&mut tx, id, &[agent_event("fourth")], 900, 512, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        range,
        AppendedRange {
            first_seq: 4,
            last_seq: 4,
        },
    );
    assert_eq!(repository.max_offset(id).await.unwrap(), Some(900));
    assert_eq!(repository.get(id).await.unwrap().input_tokens, 1_000);

    // An offset that moved under the writer: no rows, no counters, a conflict.
    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .append_native_line(&mut tx, id, &[agent_event("fifth")], 1_200, 512, None)
        .await
        .expect_err("the offset moved");
    tx.rollback().await.unwrap();

    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.to_string(), "transcript offset moved");
    assert_eq!(repository.max_seq(id).await.unwrap(), 4);
    assert_eq!(repository.max_offset(id).await.unwrap(), Some(900));

    // A line that translated to nothing is a programming error, not a committed
    // offset with no row behind it.
    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .append_native_line(&mut tx, id, &[], 1_200, 900, None)
        .await
        .expect_err("an empty line is a bug");
    tx.rollback().await.unwrap();
    assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(repository.max_seq(id).await.unwrap(), 4);

    // A negative delta is refused before the lock is taken.
    let mut tx = pool.begin().await.unwrap();
    let error = repository
        .append_native_line(
            &mut tx,
            id,
            &[agent_event("sixth")],
            1_200,
            900,
            Some(CostDelta {
                cost_usd: -1.0,
                input_tokens: 0,
                output_tokens: 0,
            }),
        )
        .await
        .expect_err("a counter may not move backwards");
    tx.rollback().await.unwrap();
    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert_eq!(repository.max_seq(id).await.unwrap(), 4);

    // And the offset never leaves the orchestrator (`SPEC.md`, "AgentEvent").
    let (page, has_more) = repository.events_page(id, None, 2).await.unwrap();
    assert!(has_more);
    assert_eq!(
        page.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [3, 4]
    );
    for event in &page {
        let encoded = serde_json::to_value(event).unwrap();
        assert!(encoded.get("_offset").is_none(), "leaked: {encoded}");
    }

    let replayed = repository.events_after(id, 3).await.unwrap();
    assert_eq!(
        replayed.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [4]
    );

    let (page, has_more) = repository.events_page(id, Some(1), 10).await.unwrap();
    assert!(page.is_empty());
    assert!(!has_more);
}

#[tokio::test]
async fn an_appended_event_is_the_single_event_form_of_the_batch() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let id = insert(&pool, &new_session(&fixture)).await;

    let mut tx = pool.begin().await.unwrap();
    let first = repository
        .append_event(&mut tx, id, &agent_event("one"))
        .await
        .unwrap();
    let second = repository
        .append_event(&mut tx, id, &agent_event("two"))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!((first, second), (1, 2));
    let (events, _) = repository.list_events(id, None, 10).await.unwrap();
    // No offset: these writers do not translate a transcript line.
    assert_eq!(events[0].payload, json!({ "text": "one" }));
    assert_eq!(repository.max_offset(id).await.unwrap(), None);
    assert_eq!(repository.get(id).await.unwrap().last_seq, 2);
}

#[tokio::test]
async fn failing_every_creating_session_leaves_the_others_alone() {
    let (_postgres, pool) = common::db::test_pool().await;
    let fixture = seed(&pool).await;
    let repository = SessionRepository::new(&pool);

    let creating = insert(&pool, &new_session(&fixture)).await;
    let running = insert(&pool, &new_session(&fixture)).await;
    let done = insert(&pool, &new_session(&fixture)).await;

    let mut tx = pool.begin().await.unwrap();
    repository
        .transition(
            &mut tx,
            running,
            &Transition::new(SessionState::Creating, SessionState::Running, "started"),
        )
        .await
        .unwrap();
    repository
        .transition(
            &mut tx,
            done,
            &Transition::new(SessionState::Creating, SessionState::Running, "started"),
        )
        .await
        .unwrap();
    repository
        .transition(
            &mut tx,
            done,
            &Transition::new(SessionState::Running, SessionState::Done, "ended"),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let failed = repository
        .fail_all_creating("orchestrator restarted")
        .await
        .unwrap();

    assert_eq!(
        failed.iter().map(|session| session.id).collect::<Vec<_>>(),
        [creating],
    );
    assert_eq!(failed[0].state, SessionState::Failed);
    assert_eq!(failed[0].error.as_deref(), Some("orchestrator restarted"));
    assert!(failed[0].ended_at.is_some());

    // Each row got its `state_change` event, in its own committed transaction.
    let (events, _) = repository.list_events(creating, None, 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "state_change");
    assert_eq!(
        events[0].payload,
        json!({
            "from": "creating", "to": "failed", "reason": "orchestrator restarted",
        }),
    );

    assert_eq!(
        repository.get(running).await.unwrap().state,
        SessionState::Running,
    );
    assert_eq!(
        repository.get(done).await.unwrap().state,
        SessionState::Done
    );

    // Nothing is left to fail the second time.
    assert!(
        repository
            .fail_all_creating("orchestrator restarted")
            .await
            .unwrap()
            .is_empty()
    );
}
