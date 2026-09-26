//! `POST /api/projects/{pid}/git/revert` (`SPEC.md`, "Git"; `ARCHITECTURE.md`,
//! "Git model", Revert; ADR 0053).
//!
//! The commit itself on bare repositories alone is `git::revert`'s unit tests.
//! What is asserted here is the whole path on a real project repository with
//! real hand-offs:
//!
//! - the head moves forward to one new commit whose tree is `to`'s and whose
//!   only parent is the old head; `reverted` is the attributed first-parent
//!   range `to..head`, each naming it in `reverted_by` as the history then
//!   does, and nothing about any task moves without `reopen`;
//! - with `reopen`, every terminal task of the range moves to the requested
//!   state with its current hand-off dropped, the user's comment once and a
//!   system comment naming the commit, and a task that is not terminal is
//!   left alone; a terminal task without a current hand-off is reopened too;
//! - the refusals: a terminal or unknown reopen state and an empty comment
//!   (400), a stale `expected_head` (409 `branch has moved`), a head that
//!   already has `to`'s tree (409 `nothing to revert`), a `to` that is
//!   not a strictly older first-parent commit and a `branch` that is not an
//!   integration head (400), no token (401) and an unknown project (404) —
//!   each leaving the head, the tasks and the event stream untouched.
//!
//! There is no project membership in Mars: a project is out of reach only by
//! not existing, which is the 404 below.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::AuthenticatedUser;
use common::handoffs::Fixture;
use mars_orchestrator::git::testutil::{run_git, test_identity};
use mars_orchestrator::git::{GitActor, GitRef, GitService, create_work_clone, refs, resolve_base};
use mars_orchestrator::models::{HistoryEntry, Task};
use mars_orchestrator::repositories::TaskRepository;
use mars_orchestrator::tracker::{REOPEN_STATE_NOT_OPEN, TaskDetailDto, TaskDto};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

/// The documented success body.
#[derive(Debug, Deserialize)]
struct RevertResponse {
    commit: String,
    reverted: Vec<HistoryEntry>,
    reopened: Vec<TaskDto>,
}

/// The documented error body with its exact message (`SPEC.md`, "REST API").
fn assert_error(response: &TestResponse, status: StatusCode, message: &str) {
    response.assert_status(status);
    response.assert_json(&json!({ "status": status.as_u16(), "error": message }));
}

/// The documented error body, whatever its message.
fn assert_status(response: &TestResponse, status: StatusCode) {
    response.assert_status(status);
    assert_eq!(
        response.json::<Value>()["status"],
        json!(status.as_u16()),
        "{}",
        response.text()
    );
}

async fn revert(fixture: &Fixture, user: &AuthenticatedUser, body: Value) -> TestResponse {
    fixture
        .app
        .post_as(
            user,
            &format!("/api/projects/{}/git/revert", fixture.project.id),
        )
        .json(&body)
        .await
}

/// `PUT` a task as the user and assert it went through.
async fn put(fixture: &Fixture, user: &AuthenticatedUser, task: &Task, body: Value) -> TaskDto {
    let response = fixture
        .app
        .put_as(
            user,
            &format!("/api/projects/{}/tasks/{}", fixture.project.id, task.id),
        )
        .json(&body)
        .await;
    response.assert_status(StatusCode::OK);

    response.json::<TaskDto>()
}

/// A session with a work clone from `main` holding one commit to `file`.
async fn session_with_file(fixture: &Fixture, file: &str) -> (Uuid, String) {
    let session_id = fixture.seed_session().await;
    {
        let guard = fixture.guard().await;
        let paths = fixture.paths();
        let base = resolve_base(&guard, &paths, None, "main")
            .await
            .expect("the base resolves");
        create_work_clone(&guard, &paths, session_id, &base, &test_identity())
            .await
            .expect("the work clone is created");
    }

    let commit = fixture
        .commit_in_work_clone(session_id, file, &format!("feat: add {file}"))
        .await;

    (session_id, commit)
}

/// The commit `main` points at in the project repository.
async fn main_commit(fixture: &Fixture) -> String {
    refs::resolve(
        &fixture.paths().project_repo(fixture.project.id),
        &GitRef::parse("main").expect("a ref name"),
    )
    .await
    .expect("main resolves")
    .commit
}

/// `git rev-parse` in the project repository.
async fn rev_parse(fixture: &Fixture, name: &str) -> String {
    run_git(
        &fixture.paths().project_repo(fixture.project.id),
        &["rev-parse", "--verify", "--end-of-options", name],
    )
    .await
    .trim()
    .to_string()
}

/// `main` at R2, then task one's hand-off A1 fast-forwarded onto it and task
/// two's B1 merged by a merge commit M: `main` is `M — A1 — R2 — R1` on its
/// first-parent line, and a revert to R2 takes both tasks back.
struct Arranged {
    user: AuthenticatedUser,
    task_one: Task,
    task_two: Task,
    handoff_one: Uuid,
    handoff_two: Uuid,
    r2: String,
    a1: String,
    b1: String,
    m: String,
}

async fn arrange(fixture: &Fixture) -> Arranged {
    let user = fixture.signed_in();
    let r2 = main_commit(fixture).await;

    let (session_a, a1) = session_with_file(fixture, "A.md").await;
    let (session_b, b1) = session_with_file(fixture, "B.md").await;

    let task_one = fixture.task("the first task", "ready").await;
    let task_two = fixture.task("the second task", "ready").await;
    let handoff_one = put(
        fixture,
        &user,
        &task_one,
        json!({ "state": "review", "handoff": {
            "kind": "revision", "source_session_id": session_a,
            "commit": a1, "comment": "ready for review",
        }}),
    )
    .await
    .handoff
    .expect("the hand-off is published")
    .id;
    let handoff_two = put(
        fixture,
        &user,
        &task_two,
        json!({ "state": "review", "handoff": {
            "kind": "revision", "source_session_id": session_b,
            "commit": b1, "comment": "ready for review",
        }}),
    )
    .await
    .handoff
    .expect("the hand-off is published")
    .id;

    let service = GitService::from_state(&fixture.app.state);
    let actor = GitActor::User(user.user.id);
    service
        .merge_branch(fixture.project.id, &a1, "main", None, &actor)
        .await
        .expect("A merges");
    let m = service
        .merge_branch(fixture.project.id, &b1, "main", None, &actor)
        .await
        .expect("B merges")
        .commit;

    Arranged {
        user,
        task_one,
        task_two,
        handoff_one,
        handoff_two,
        r2,
        a1,
        b1,
        m,
    }
}

/// The project's last task event sequence.
async fn cursor(fixture: &Fixture) -> i64 {
    events_after(fixture, 0)
        .await
        .last()
        .map(|(seq, _, _)| *seq)
        .unwrap_or(0)
}

/// The project's task events after `after`, as `(seq, kind, payload)`.
async fn events_after(fixture: &Fixture, after: i64) -> Vec<(i64, String, Value)> {
    TaskRepository::new(&fixture.app.pool)
        .list_task_events_after(fixture.project.id, after, 1000)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| (row.seq, row.kind, row.payload))
        .collect()
}

/// The task as the drawer reads it.
async fn detail(fixture: &Fixture, user: &AuthenticatedUser, task_id: Uuid) -> TaskDetailDto {
    let response = fixture
        .app
        .get_as(
            user,
            &format!("/api/projects/{}/tasks/{task_id}", fixture.project.id),
        )
        .await;
    response.assert_status(StatusCode::OK);

    response.json::<TaskDetailDto>()
}

fn task_ids(entry: &HistoryEntry) -> Vec<Uuid> {
    entry.tasks.iter().map(|task| task.id).collect()
}

// ---- the happy paths ----

#[tokio::test]
async fn a_revert_writes_one_commit_with_to_s_tree_and_moves_no_task() {
    let fixture = Fixture::create("revert-plain").await;
    let a = arrange(&fixture).await;
    let before_one = fixture.task_row(a.task_one.id).await;
    let before_two = fixture.task_row(a.task_two.id).await;
    let cursor = cursor(&fixture).await;

    let response = revert(
        &fixture,
        &a.user,
        json!({ "branch": "main", "to": a.r2, "expected_head": a.m }),
    )
    .await;
    response.assert_status(StatusCode::OK);
    let body = response.json::<RevertResponse>();

    // Forward, never back: one new commit on top of the old head.
    assert_eq!(main_commit(&fixture).await, body.commit);
    assert_eq!(rev_parse(&fixture, &format!("{}^", body.commit)).await, a.m);
    assert_eq!(
        rev_parse(&fixture, &format!("{}^{{tree}}", body.commit)).await,
        rev_parse(&fixture, &format!("{}^{{tree}}", a.r2)).await,
        "the tree is to's"
    );

    // The first-parent range `to..head`, attributed as the history is.
    let commits: Vec<&str> = body.reverted.iter().map(|e| e.commit.as_str()).collect();
    assert_eq!(commits, [a.m.as_str(), &a.a1]);
    assert_eq!(task_ids(&body.reverted[0]), [a.task_two.id]);
    assert_eq!(body.reverted[0].tasks[0].handoff_id, a.handoff_two);
    assert_eq!(task_ids(&body.reverted[1]), [a.task_one.id]);
    assert_eq!(body.reverted[1].tasks[0].handoff_id, a.handoff_one);
    assert!(
        body.reverted
            .iter()
            .all(|entry| entry.reverted_by.as_deref() == Some(body.commit.as_str())),
        "what the revert took back names it"
    );
    assert!(body.reopened.is_empty());

    // The history now starts with the revert commit, by the bot, requested by
    // the user, listing what it took back; the entries it undid name it, and
    // the one it went back to has the head's tree.
    let history = fixture
        .app
        .get_as(
            &a.user,
            &format!("/api/projects/{}/git/history", fixture.project.id),
        )
        .await
        .json::<Vec<HistoryEntry>>();
    let marks: Vec<(&str, Option<&str>)> = history
        .iter()
        .take(4)
        .map(|entry| (entry.commit.as_str(), entry.reverted_by.as_deref()))
        .collect();
    assert_eq!(
        marks,
        [
            (body.commit.as_str(), None),
            (a.m.as_str(), Some(body.commit.as_str())),
            (a.a1.as_str(), Some(body.commit.as_str())),
            (a.r2.as_str(), None),
        ]
    );
    assert_eq!(history[3].tree, history[0].tree);
    // A page after the revert's own still names it.
    let second = fixture
        .app
        .get_as(
            &a.user,
            &format!(
                "/api/projects/{}/git/history?limit=1&before={}",
                fixture.project.id, body.commit
            ),
        )
        .await
        .json::<Vec<HistoryEntry>>();
    assert_eq!(second[0].commit, a.m);
    assert_eq!(second[0].reverted_by.as_deref(), Some(body.commit.as_str()));
    assert_ne!(history[1].tree, history[0].tree);
    assert_eq!(history[0].commit, body.commit);
    assert_eq!(
        history[0].subject,
        format!("Revert main to {}", &a.r2[..12])
    );
    assert_eq!(
        history[0].requested_by.as_deref(),
        Some(format!("user:{}", a.user.user.id).as_str())
    );
    let message = run_git(
        &fixture.paths().project_repo(fixture.project.id),
        &["log", "-1", "--format=%B", "--end-of-options", &body.commit],
    )
    .await;
    assert!(message.contains(&a.m[..12]), "{message}");
    assert!(message.contains(&a.a1[..12]), "{message}");

    // No task moved, and nothing was emitted.
    assert_eq!(fixture.task_row(a.task_one.id).await, before_one);
    assert_eq!(fixture.task_row(a.task_two.id).await, before_two);
    assert!(events_after(&fixture, cursor).await.is_empty());
}

#[tokio::test]
async fn reopen_moves_the_terminal_tasks_drops_their_hand_offs_and_comments_twice() {
    let fixture = Fixture::create("revert-reopen").await;
    let a = arrange(&fixture).await;

    // Task one closed with its hand-off; task two still in review.
    put(&fixture, &a.user, &a.task_one, json!({ "state": "done" })).await;
    let before_two = fixture.task_row(a.task_two.id).await;
    let comments_one = detail(&fixture, &a.user, a.task_one.id)
        .await
        .comments
        .len();
    let cursor = cursor(&fixture).await;

    let response = revert(
        &fixture,
        &a.user,
        json!({
            "branch": "main", "to": a.r2, "expected_head": a.m,
            "reopen": { "state": "ready", "comment": "built on the wrong base" },
        }),
    )
    .await;
    response.assert_status(StatusCode::OK);
    let body = response.json::<RevertResponse>();

    // Only the terminal task is reopened, into `ready`, without its hand-off.
    assert_eq!(body.reopened.len(), 1);
    let reopened = &body.reopened[0];
    assert_eq!(reopened.id, a.task_one.id);
    assert_eq!(reopened.state, "ready");
    assert!(reopened.handoff.is_none());
    let row = fixture.task_row(a.task_one.id).await;
    assert_eq!(row.closed_at, None);
    assert_eq!(row.current_handoff_id, None);

    // Task two is listed in `reverted` but not moved.
    assert_eq!(task_ids(&body.reverted[0]), [a.task_two.id]);
    assert_eq!(fixture.task_row(a.task_two.id).await, before_two);

    // The user's comment once, then the system line naming the commit; the
    // dropped hand-off stays in the history.
    let detail_one = detail(&fixture, &a.user, a.task_one.id).await;
    let comments = &detail_one.comments[comments_one..];
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].body, "built on the wrong base");
    assert_eq!(comments[0].author_user_id, Some(a.user.user.id));
    assert!(!comments[0].system);
    assert!(comments[1].system);
    assert!(
        comments[1].body.contains(&body.commit),
        "{}",
        comments[1].body
    );
    assert!(
        detail_one
            .handoffs
            .iter()
            .any(|handoff| handoff.id == a.handoff_one)
    );

    // The events: the move, the drop's `updated` and the user's comment, then
    // the system comment — all task one's.
    let events = events_after(&fixture, cursor).await;
    let kinds: Vec<&str> = events.iter().map(|(_, kind, _)| kind.as_str()).collect();
    assert_eq!(
        kinds,
        ["state_changed", "updated", "commented", "commented"]
    );
    for (_, _, payload) in &events {
        assert_eq!(payload["task"]["id"], json!(a.task_one.id));
    }
    assert_eq!(events[0].2["from"], json!("done"));
    assert_eq!(events[0].2["to"], json!("ready"));
    assert_eq!(
        events[2].2["actor"],
        json!({ "kind": "user", "user_id": a.user.user.id })
    );
    assert_eq!(events[3].2["actor"], json!({ "kind": "system" }));
}

#[tokio::test]
async fn a_terminal_task_without_a_current_hand_off_is_reopened_all_the_same() {
    let fixture = Fixture::create("revert-no-handoff").await;
    let a = arrange(&fixture).await;

    // Task two's hand-off dropped before it was closed: the drop verb would
    // refuse it, and the revert must not.
    let dropped = fixture
        .app
        .post_as(
            &a.user,
            &format!(
                "/api/projects/{}/tasks/{}/drop-handoff",
                fixture.project.id, a.task_two.id
            ),
        )
        .json(&json!({ "comment": "not this one" }))
        .await;
    dropped.assert_status(StatusCode::OK);
    put(&fixture, &a.user, &a.task_two, json!({ "state": "done" })).await;
    put(
        &fixture,
        &a.user,
        &a.task_one,
        json!({ "state": "cancelled" }),
    )
    .await;
    let comments_two = detail(&fixture, &a.user, a.task_two.id)
        .await
        .comments
        .len();

    let response = revert(
        &fixture,
        &a.user,
        json!({
            "branch": "main", "to": a.r2, "expected_head": a.m,
            "reopen": { "state": "needs_human", "comment": "look again" },
        }),
    )
    .await;
    response.assert_status(StatusCode::OK);
    let body = response.json::<RevertResponse>();

    // Newest entry first: task two (M), then task one (A1).
    let reopened: Vec<(Uuid, &str)> = body
        .reopened
        .iter()
        .map(|task| (task.id, task.state.as_str()))
        .collect();
    assert_eq!(
        reopened,
        [
            (a.task_two.id, "needs_human"),
            (a.task_one.id, "needs_human")
        ]
    );

    let comments = detail(&fixture, &a.user, a.task_two.id).await.comments;
    let comments = &comments[comments_two..];
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].body, "look again");
    assert!(comments[1].system);
}

// ---- the refusals ----

#[tokio::test]
async fn a_terminal_or_unknown_reopen_state_and_an_empty_comment_are_400() {
    let fixture = Fixture::create("revert-reopen-400").await;
    let a = arrange(&fixture).await;
    put(&fixture, &a.user, &a.task_one, json!({ "state": "done" })).await;
    let cursor = cursor(&fixture).await;

    let refusal = |reopen: Value| json!({ "branch": "main", "to": a.r2, "expected_head": a.m, "reopen": reopen });

    let response = revert(
        &fixture,
        &a.user,
        refusal(json!({ "state": "cancelled", "comment": "why" })),
    )
    .await;
    assert_error(&response, StatusCode::BAD_REQUEST, REOPEN_STATE_NOT_OPEN);

    let response = revert(
        &fixture,
        &a.user,
        refusal(json!({ "state": "nowhere", "comment": "why" })),
    )
    .await;
    assert_status(&response, StatusCode::BAD_REQUEST);

    let response = revert(
        &fixture,
        &a.user,
        refusal(json!({ "state": "ready", "comment": "  " })),
    )
    .await;
    assert_error(
        &response,
        StatusCode::BAD_REQUEST,
        "comment body must not be empty",
    );

    assert_eq!(main_commit(&fixture).await, a.m, "the head is untouched");
    assert!(events_after(&fixture, cursor).await.is_empty());
}

#[tokio::test]
async fn a_stale_expected_head_is_409_and_leaves_the_head() {
    let fixture = Fixture::create("revert-stale").await;
    let a = arrange(&fixture).await;

    // The user confirmed against A1; main has since moved to M.
    let response = revert(
        &fixture,
        &a.user,
        json!({ "branch": "main", "to": a.r2, "expected_head": a.a1 }),
    )
    .await;
    assert_error(&response, StatusCode::CONFLICT, "branch has moved");

    assert_eq!(main_commit(&fixture).await, a.m);
}

#[tokio::test]
async fn a_revert_that_would_change_nothing_is_409_and_writes_nothing() {
    let fixture = Fixture::create("revert-noop").await;
    let a = arrange(&fixture).await;
    put(&fixture, &a.user, &a.task_one, json!({ "state": "done" })).await;
    let response = revert(
        &fixture,
        &a.user,
        json!({ "branch": "main", "to": a.r2, "expected_head": a.m }),
    )
    .await;
    response.assert_status(StatusCode::OK);
    let head = response.json::<RevertResponse>().commit;
    let before_one = fixture.task_row(a.task_one.id).await;
    let cursor = cursor(&fixture).await;

    // `main` already has R2's tree: reverting to it again would add an empty
    // commit and reopen what is already reverted.
    let response = revert(
        &fixture,
        &a.user,
        json!({
            "branch": "main", "to": a.r2, "expected_head": head,
            "reopen": { "state": "ready", "comment": "once more" },
        }),
    )
    .await;
    assert_error(
        &response,
        StatusCode::CONFLICT,
        &format!("nothing to revert: main already matches {}", &a.r2[..12]),
    );

    assert_eq!(main_commit(&fixture).await, head, "no commit, no ref move");
    assert_eq!(fixture.task_row(a.task_one.id).await, before_one);
    assert!(events_after(&fixture, cursor).await.is_empty(), "no reopen");
}

#[tokio::test]
async fn a_to_that_is_not_an_older_first_parent_commit_is_400() {
    let fixture = Fixture::create("revert-to").await;
    let a = arrange(&fixture).await;

    for to in [
        // Only the merged side branch reaches B1.
        a.b1.clone(),
        // The head itself is not strictly older.
        a.m.clone(),
        // Not in the repository at all.
        "0123456789abcdef0123456789abcdef01234567".to_string(),
        // Not a full object id.
        "main".to_string(),
    ] {
        let response = revert(
            &fixture,
            &a.user,
            json!({ "branch": "main", "to": to, "expected_head": a.m }),
        )
        .await;
        assert_status(&response, StatusCode::BAD_REQUEST);
    }

    let session = fixture.seed_session().await;
    for branch in [
        "origin/main".to_string(),
        format!("refs/sessions/{session}"),
        "no-such-branch".to_string(),
    ] {
        let response = revert(
            &fixture,
            &a.user,
            json!({ "branch": branch, "to": a.r2, "expected_head": a.m }),
        )
        .await;
        assert_status(&response, StatusCode::BAD_REQUEST);
    }

    assert_eq!(main_commit(&fixture).await, a.m);
}

#[tokio::test]
async fn no_token_is_401_and_an_unknown_project_is_404() {
    let fixture = Fixture::create("revert-auth").await;
    let a = arrange(&fixture).await;
    let body = json!({ "branch": "main", "to": a.r2, "expected_head": a.m });

    let response = fixture
        .app
        .server
        .post(&format!("/api/projects/{}/git/revert", fixture.project.id))
        .json(&body)
        .await;
    assert_status(&response, StatusCode::UNAUTHORIZED);

    let response = fixture
        .app
        .post_as(
            &a.user,
            &format!("/api/projects/{}/git/revert", Uuid::new_v4()),
        )
        .json(&body)
        .await;
    assert_status(&response, StatusCode::NOT_FOUND);

    assert_eq!(main_commit(&fixture).await, a.m);
}
