//! `GET /api/projects/{pid}/tasks/stream` end to end (`SPEC.md`, "SSE: task
//! stream"; `ARCHITECTURE.md`, "Event delivery").
//!
//! The task board's stream, over a real HTTP transport, because an SSE body
//! is the one thing `axum-test`'s buffered response cannot hold: it never
//! ends. What is asserted here is the part of the contract no unit test can
//! see:
//!
//! - everything that must be answered with a *status* is answered before the
//!   response opens — 401, 403, 404 and 400 — because that is the only answer
//!   a browser can act on: SSE has no error frame;
//! - the handler subscribes before it replays, so an event committed while the
//!   first page is being written is delivered exactly once, and the
//!   commit-during-replay scenario below is the proof;
//! - a lost notification is covered by the safety read, asserted with the
//!   shared listener switched off so nothing *but* the safety read can deliver
//!   the event;
//! - the body opens with a `: ready` comment before the replay, so a stream
//!   with nothing to replay still writes a body byte at once and a buffering
//!   proxy cannot hold the client's `open` event back;
//! - the keepalive comment is also the re-authorization tick: a revoked login
//!   ends the body, silently, because there is nothing to send instead.
//!
//! The periods are `common::TEST_TIMINGS` (100 ms keepalive, 300 ms safety
//! read), not the documented fifteen and thirty seconds.
//!
//! Every event is seeded through a real `TrackerMutation`, which is the only
//! writer allowed to allocate a sequence (`docs/data-model.md`,
//! `task_events`). `deleted` is the kind used throughout, because it is the
//! one kind whose task row is expected to be missing: it needs no task
//! fixtures and it carries the `task_id` whose survival `SPEC.md` promises.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use common::sse::SseReader;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::tracker::TrackerMutation;
use serde_json::Value;
use uuid::Uuid;

/// How long something that should already be on its way is waited for.
///
/// Generous against the test timings on purpose: every assertion below is
/// "this arrives", never "this arrives quickly", and a scheduling hiccup on a
/// loaded machine must not be a failure.
const WITHIN: Duration = Duration::from_secs(2);

/// An obviously fake password, long enough for `POST /api/test/users`
/// (rule 3).
const PASSWORD: &str = "not-a-real-password";

/// The rows one stream needs: a user to open it as and a project to watch.
struct Fixture {
    user: AuthenticatedUser,
    project_id: Uuid,
}

/// A signed-in user and a project, seeded the way the other repository-level
/// suites seed one: what this file is about starts at the events.
async fn arrange(app: &TestApp) -> Fixture {
    let user = app
        .create_user("boarder", "boarder@example.test", PASSWORD)
        .await;

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        .bind("https://example.invalid/org/repo.git")
        .execute(&app.pool)
        .await
        .expect("the project seeds");

    Fixture { user, project_id }
}

/// Append `count` `deleted` events in one mutation and return the task ids
/// they carry, in order.
async fn append(app: &TestApp, project_id: Uuid, count: usize) -> Vec<Uuid> {
    let mut mutation = TrackerMutation::begin(&app.pool, project_id, TaskActor::System)
        .await
        .expect("the mutation begins");

    let ids: Vec<Uuid> = (0..count).map(|_| Uuid::new_v4()).collect();
    for id in &ids {
        mutation.emit_deleted(*id).expect("the event is emitted");
    }

    mutation.commit().await.expect("the mutation commits");

    ids
}

/// The `TaskEvent` a frame carries, with its framing asserted.
fn task_event(frame: &common::sse::SseFrame) -> TaskEvent {
    assert_eq!(
        frame.event.as_deref(),
        Some("task"),
        "every task frame names its event",
    );

    let event: TaskEvent = frame.json();
    assert_eq!(
        frame.id.as_deref(),
        Some(event.seq.to_string().as_str()),
        "the SSE id is the sequence",
    );

    event
}

// ---- before the response opens ----

#[tokio::test]
async fn a_stream_without_a_token_is_401() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let response = app.sse_response(fixture.project_id, None, None, None).await;

    assert_eq!(response.status(), 401);
    let body: Value = response.json().await.expect("the body is the error JSON");
    assert_eq!(body["status"], 401);
    assert_eq!(body["error"], "authentication required");
}

#[tokio::test]
async fn a_gated_user_cannot_open_the_stream() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    let gated = app
        .create_gated_user("gated", "gated@example.invalid")
        .await;

    let response = app
        .sse_response(fixture.project_id, Some(&gated.access_token), None, None)
        .await;

    assert_eq!(response.status(), 403);
    let body: Value = response.json().await.expect("the body is the error JSON");
    assert_eq!(body["error"], "password change required");
}

#[tokio::test]
async fn an_unknown_project_is_404_before_anything_is_streamed() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let response = app
        .sse_response(Uuid::new_v4(), Some(&fixture.user.access_token), None, None)
        .await;

    assert_eq!(response.status(), 404);
    let body: Value = response.json().await.expect("the body is the error JSON");
    assert_eq!(body["error"], "project not found");
}

#[tokio::test]
async fn an_after_that_is_not_a_sequence_is_400() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let response = app
        .sse_response(
            fixture.project_id,
            Some(&fixture.user.access_token),
            Some("abc"),
            None,
        )
        .await;

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.expect("the body is the error JSON");
    assert_eq!(body["error"], "invalid cursor");
}

#[tokio::test]
async fn a_last_event_id_that_is_not_a_sequence_is_400() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let response = app
        .sse_response(
            fixture.project_id,
            Some(&fixture.user.access_token),
            None,
            Some("x"),
        )
        .await;

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.expect("the body is the error JSON");
    assert_eq!(body["error"], "invalid cursor");
}

#[tokio::test]
async fn the_open_response_carries_the_documented_headers_and_no_retry() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    append(&app, fixture.project_id, 1).await;

    let response = app
        .sse_response(
            fixture.project_id,
            Some(&fixture.user.access_token),
            None,
            None,
        )
        .await;

    assert_eq!(response.status(), 200);
    let headers = response.headers().clone();
    assert_eq!(headers["content-type"], "text/event-stream");
    assert_eq!(headers["cache-control"], "no-cache");
    assert_eq!(headers["x-accel-buffering"], "no");

    // The client disables automatic reconnect and reopens explicitly, so a
    // `retry:` field would contradict it (`SPEC.md`, "Authentication").
    let mut body = String::new();
    let mut bytes = Box::pin(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );
    let _ = tokio::time::timeout(Duration::from_millis(500), async {
        while let Some(chunk) = futures_util::StreamExt::next(&mut bytes).await {
            body.push_str(std::str::from_utf8(&chunk).expect("the body is UTF-8"));
        }
    })
    .await;

    assert!(body.contains("event: task"), "the replayed event arrives");
    assert!(body.contains(": keepalive"), "the keepalive is a comment");
    assert!(
        !body.contains("retry:"),
        "no reconnection delay is ever sent: {body}",
    );
}

#[tokio::test]
async fn a_stream_with_nothing_to_replay_writes_its_first_frame_at_once() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    // No events at all: without the opening comment the first body byte would
    // be the first keepalive, and a proxy that holds the headers until then
    // holds the client's `open` event with it (`SPEC.md`, "SSE: task
    // stream").
    let started = std::time::Instant::now();
    let mut reader = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );

    let frame = tokio::time::timeout(Duration::from_secs(1), reader.next_frame())
        .await
        .expect("the first frame arrives at once")
        .expect("the stream is open");

    assert!(
        frame.is_ready(),
        "the body opens with `: ready`, before any replay: {frame:?}",
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the opening comment does not wait for a timer",
    );
}

#[tokio::test]
async fn the_opening_comment_precedes_the_replay() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    append(&app, fixture.project_id, 1).await;

    let mut reader = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );

    let first = reader.next_frame().await.expect("the stream is open");
    assert!(first.is_ready(), "the comment comes first: {first:?}");
    assert_eq!(task_event(&reader.event_within(WITHIN).await).seq, 1);
}

// ---- replay ----

#[tokio::test]
async fn last_event_id_replays_only_what_comes_after_it() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    let ids = append(&app, fixture.project_id, 5).await;

    let mut reader = SseReader::new(
        app.sse(
            fixture.project_id,
            &fixture.user.access_token,
            None,
            Some(2),
        )
        .await,
    );

    for seq in 3..=5i64 {
        let event = task_event(&reader.event_within(WITHIN).await);
        assert_eq!(event.seq, seq);
        assert_eq!(event.kind, TaskEventKind::Deleted);
        assert_eq!(
            event.task_id,
            Some(ids[seq as usize - 1]),
            "a deleted event keeps the original task id",
        );
        assert!(event.task.is_none(), "a deleted event carries no task");
    }
}

#[tokio::test]
async fn after_replays_exactly_as_last_event_id_does() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    append(&app, fixture.project_id, 5).await;

    let mut reader = SseReader::new(
        app.sse(
            fixture.project_id,
            &fixture.user.access_token,
            Some(2),
            None,
        )
        .await,
    );

    for seq in 3..=5i64 {
        assert_eq!(task_event(&reader.event_within(WITHIN).await).seq, seq);
    }
}

#[tokio::test]
async fn the_header_wins_over_the_query_parameter() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    append(&app, fixture.project_id, 5).await;

    // `?after=0` would replay everything; the header says four are already
    // processed, and the header is the browser's own record.
    let mut reader = SseReader::new(
        app.sse(
            fixture.project_id,
            &fixture.user.access_token,
            Some(0),
            Some(4),
        )
        .await,
    );

    assert_eq!(task_event(&reader.event_within(WITHIN).await).seq, 5);
}

#[tokio::test]
async fn a_cursor_beyond_the_stream_replays_nothing_and_follows_from_it() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    append(&app, fixture.project_id, 2).await;

    let mut reader = SseReader::new(
        app.sse(
            fixture.project_id,
            &fixture.user.access_token,
            None,
            Some(99),
        )
        .await,
    );

    // A replay of zero rows still opens the response, and the keepalive is
    // what the client sees first — the frontend waits for `open` before its
    // first REST load (`SPEC.md`, "SSE: task stream").
    reader.keepalive_within(WITHIN).await;

    // The cursor is the contract, not a guess at what the client has: it
    // claims everything through 99, so `seq 3` is behind it and is not sent.
    // Live follows from the cursor it was given.
    append(&app, fixture.project_id, 1).await;

    let quiet = tokio::time::timeout(Duration::from_millis(500), reader.event_within(WITHIN)).await;
    assert!(quiet.is_err(), "nothing at or below the cursor is sent");
}

#[tokio::test]
async fn after_latest_replays_nothing_and_follows_from_the_current_end() {
    use futures_util::StreamExt;

    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    append(&app, fixture.project_id, 3).await;

    // What a board opening this project for the first time sends: it has no
    // cursor, and the history is a replay it would throw away, because it
    // loads an authoritative REST snapshot as soon as this stream is open
    // (`SPEC.md`, "Frontend", "Board refresh ordering").
    let response = app
        .sse_response(
            fixture.project_id,
            Some(&fixture.user.access_token),
            Some("latest"),
            None,
        )
        .await;
    assert_eq!(response.status(), 200, "`latest` is a cursor, not a 400");

    let mut reader = SseReader::new(
        response
            .bytes_stream()
            .map(|chunk| chunk.expect("the body streams")),
    );

    let first = reader.next_frame().await.expect("the stream is open");
    assert!(first.is_ready(), "the body still opens with `: ready`");

    let quiet = tokio::time::timeout(Duration::from_millis(500), reader.event_within(WITHIN)).await;
    assert!(quiet.is_err(), "none of the three events is replayed");

    // The cursor really is the end of the stream, not zero: the next event
    // arrives live, once.
    append(&app, fixture.project_id, 1).await;
    assert_eq!(task_event(&reader.event_within(WITHIN).await).seq, 4);
}

// ---- live ----

#[tokio::test]
async fn an_event_appended_after_the_replay_arrives_live() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    append(&app, fixture.project_id, 1).await;

    let mut reader = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );
    assert_eq!(task_event(&reader.event_within(WITHIN).await).seq, 1);

    append(&app, fixture.project_id, 1).await;

    assert_eq!(task_event(&reader.event_within(WITHIN).await).seq, 2);
}

#[tokio::test]
async fn a_rolled_back_mutation_sends_nothing() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut reader = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );
    reader.keepalive_within(WITHIN).await;

    // Emitted and then dropped without a commit: `pg_notify` runs inside the
    // transaction, so PostgreSQL discards the notification with the rows
    // (ADR 0028).
    {
        let mut mutation = TrackerMutation::begin(&app.pool, fixture.project_id, TaskActor::System)
            .await
            .expect("the mutation begins");
        mutation
            .emit_deleted(Uuid::new_v4())
            .expect("the event is emitted");
    }

    let quiet = tokio::time::timeout(Duration::from_millis(500), reader.event_within(WITHIN)).await;
    assert!(quiet.is_err(), "a rolled-back mutation produces no frame");
}

#[tokio::test]
async fn two_streams_on_one_project_both_receive_every_event() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut first = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );
    let mut second = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );
    first.keepalive_within(WITHIN).await;
    second.keepalive_within(WITHIN).await;

    append(&app, fixture.project_id, 1).await;

    assert_eq!(task_event(&first.event_within(WITHIN).await).seq, 1);
    assert_eq!(task_event(&second.event_within(WITHIN).await).seq, 1);
}

// ---- the keepalive, which is the re-authorization tick ----

#[tokio::test]
async fn a_keepalive_comment_arrives_and_the_stream_stays_open() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut reader = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );

    let frame = reader.keepalive_within(WITHIN).await;
    assert!(frame.is_keepalive(), "the comment is `: keepalive`");

    // Still open: a second one follows, and an event appended afterwards is
    // still delivered.
    reader.keepalive_within(WITHIN).await;
    append(&app, fixture.project_id, 1).await;
    assert_eq!(task_event(&reader.event_within(WITHIN).await).seq, 1);
}

#[tokio::test]
async fn a_raised_auth_version_ends_the_body() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut reader = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );
    reader.keepalive_within(WITHIN).await;

    sqlx::query("UPDATE users SET auth_version = auth_version + 1 WHERE id = $1")
        .bind(fixture.user.user.id)
        .execute(&app.pool)
        .await
        .expect("the login generation is superseded");

    assert!(
        reader.ended_within(WITHIN).await,
        "a revoked login closes the stream on the next keepalive tick",
    );
}

// ---- the safety read ----

#[tokio::test]
async fn the_safety_read_delivers_what_a_lost_notification_would_have() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut reader = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );
    reader.keepalive_within(WITHIN).await;

    // With the shared listener stopped nothing publishes to the fan-out, so
    // the only thing that can deliver the event is the periodic read.
    app.listener.shutdown.cancel();

    append(&app, fixture.project_id, 1).await;

    assert_eq!(task_event(&reader.event_within(WITHIN).await).seq, 1);
}

#[tokio::test]
async fn a_project_deleted_mid_stream_ends_the_body() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    let mut reader = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );
    reader.keepalive_within(WITHIN).await;

    sqlx::query("DELETE FROM projects WHERE id = $1")
        .bind(fixture.project_id)
        .execute(&app.pool)
        .await
        .expect("the project is deleted");

    assert!(
        reader.ended_within(WITHIN).await,
        "the safety read notices the project is gone",
    );
}

// ---- the race the subscription order exists for ----

#[tokio::test]
async fn an_event_committed_during_the_replay_arrives_exactly_once() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;

    // More than one page (500), so the commit below lands while the first page
    // is still being written.
    append(&app, fixture.project_id, 600).await;

    let mut reader = SseReader::new(
        app.sse(fixture.project_id, &fixture.user.access_token, None, None)
            .await,
    );

    let pool = app.pool.clone();
    let project_id = fixture.project_id;
    let appender = tokio::spawn(async move {
        let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
            .await
            .expect("the mutation begins");
        mutation
            .emit_deleted(Uuid::new_v4())
            .expect("the event is emitted");
        mutation
            .commit()
            .await
            .expect("the mutation commits")
            .last_seq()
            .expect("the batch allocated a sequence")
    });

    let mut seen: Vec<i64> = Vec::new();
    while seen.last() != Some(&601) {
        seen.push(task_event(&reader.event_within(WITHIN).await).seq);
    }

    assert_eq!(appender.await.expect("the appender finishes"), 601);
    assert_eq!(
        seen,
        (1..=601).collect::<Vec<i64>>(),
        "every event arrives once, in order, across the page boundary",
    );
    assert_eq!(
        seen.iter().copied().collect::<BTreeSet<i64>>().len(),
        seen.len(),
        "no event is delivered twice",
    );
}

#[tokio::test]
async fn a_client_that_disconnects_lets_go_of_its_subscription() {
    let app = TestApp::spawn().await;
    let fixture = arrange(&app).await;
    append(&app, fixture.project_id, 600).await;

    {
        let mut reader = SseReader::new(
            app.sse(fixture.project_id, &fixture.user.access_token, None, None)
                .await,
        );
        // Mid-replay: one frame read, hundreds still to come, and then the
        // response is dropped.
        task_event(&reader.event_within(WITHIN).await);
    }

    // Hyper drops the body future when the connection goes, which drops the
    // receiver the handler owns.
    let gone = tokio::time::timeout(WITHIN, async {
        while app.state.fanout.project_subscribers(fixture.project_id) != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    assert!(gone.is_ok(), "the fan-out forgets a project nobody watches");
}
