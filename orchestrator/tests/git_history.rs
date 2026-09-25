//! `GET /api/projects/{pid}/git/history` and the attribution under it
//! (`SPEC.md`, "Git": `HistoryEntry`; `ARCHITECTURE.md`, "Git model",
//! History).
//!
//! The walk, the pagination cursor and the per-entry ranges on bare
//! repositories alone are `git::history`'s unit tests. What is asserted here
//! is the whole path on a real project repository with real hand-offs:
//!
//! - a fast-forwarded hand-off commit is its own entry, attributed to its
//!   task and its source session;
//! - a merge commit is attributed to the task whose hand-off is its second
//!   parent, to that hand-off's source session and to the session its
//!   `Requested-By` trailer names;
//! - a forwarded hand-off is named by its newest record;
//! - a commit no hand-off pinned has empty lists;
//! - pages follow `before`, and [`GitService::attribute_range`] names every
//!   task merged after a point;
//! - a `branch` that is not an integration head, an unknown `before` and a
//!   zero `limit` are 400, no token is 401 and an unknown project 404.
//!
//! Hand-offs are published through `PUT /projects/{pid}/tasks/{id}` as a user
//! publishes them, and merges go through [`GitService`], so every row, ref
//! and trailer is what production writes. Git is never mocked (`CLAUDE.md`,
//! "Testing expectations").
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use axum_test::TestResponse;
use common::AuthenticatedUser;
use common::handoffs::Fixture;
use mars_orchestrator::git::testutil::{TEST_AUTHOR_NAME, test_identity};
use mars_orchestrator::git::{GitActor, GitRef, GitService, create_work_clone, refs, resolve_base};
use mars_orchestrator::models::{HistoryEntry, Task};
use mars_orchestrator::tracker::TaskDto;
use serde_json::{Value, json};
use uuid::Uuid;

/// The documented error body (`SPEC.md`, "REST API").
fn assert_status(response: &TestResponse, status: StatusCode) {
    response.assert_status(status);
    assert_eq!(
        response.json::<Value>()["status"],
        json!(status.as_u16()),
        "{}",
        response.text()
    );
}

/// The history of the fixture's project with these query parameters.
async fn history(fixture: &Fixture, user: &AuthenticatedUser, query: &str) -> TestResponse {
    fixture
        .app
        .get_as(
            user,
            &format!("/api/projects/{}/git/history{query}", fixture.project.id),
        )
        .await
}

/// `PUT` a task with a hand-off as the user, and answer the new record's id.
async fn publish(
    fixture: &Fixture,
    user: &AuthenticatedUser,
    task: &Task,
    state: &str,
    handoff: Value,
) -> Uuid {
    let response = fixture
        .app
        .put_as(
            user,
            &format!("/api/projects/{}/tasks/{}", fixture.project.id, task.id),
        )
        .json(&json!({ "state": state, "handoff": handoff }))
        .await;

    response.assert_status(StatusCode::OK);
    response
        .json::<TaskDto>()
        .handoff
        .expect("the hand-off is published")
        .id
}

/// A revision hand-off body.
fn revision(source_session_id: Uuid, commit: &str) -> Value {
    json!({
        "kind": "revision",
        "source_session_id": source_session_id,
        "commit": commit,
        "comment": "ready for review",
    })
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

/// The arrangement every scenario below reads.
///
/// `main` starts at the upstream's two commits, R1 and R2. Task one's session
/// A commits `A.md` on R2 and hands it off; task two's session B does the same
/// with `B.md`. A is merged first and fast-forwards `main`; B is merged second
/// by session C, as an agent would, which makes a merge commit M with parents
/// `[A1, B1]` and `Requested-By: session:<C>`. Task one's hand-off is then
/// forwarded with an approval, which adds a second record for the same
/// commit.
struct History {
    user: AuthenticatedUser,
    task_one: Task,
    task_two: Task,
    session_a: Uuid,
    session_b: Uuid,
    session_c: Uuid,
    /// Task one's forwarded record: the newest one pinning A1.
    handoff_a: Uuid,
    handoff_b: Uuid,
    r2: String,
    a1: String,
    b1: String,
    m: String,
}

async fn arrange(fixture: &Fixture) -> History {
    let user = fixture.signed_in();
    let r2 = main_commit(fixture).await;

    let (session_a, a1) = session_with_file(fixture, "A.md").await;
    let (session_b, b1) = session_with_file(fixture, "B.md").await;
    let session_c = fixture.seed_session().await;

    let task_one = fixture.task("the first task", "ready").await;
    let task_two = fixture.task("the second task", "ready").await;
    let first_a = publish(
        fixture,
        &user,
        &task_one,
        "review",
        revision(session_a, &a1),
    )
    .await;
    let handoff_b = publish(
        fixture,
        &user,
        &task_two,
        "review",
        revision(session_b, &b1),
    )
    .await;
    let handoff_a = publish(
        fixture,
        &user,
        &task_one,
        "merge",
        json!({
            "kind": "forward",
            "handoff_id": first_a,
            "review": "approved",
            "comment": "approved",
        }),
    )
    .await;
    assert_ne!(handoff_a, first_a, "a forward writes a record of its own");

    let service = GitService::from_state(&fixture.app.state);
    let fast_forward = service
        .merge_branch(
            fixture.project.id,
            &a1,
            "main",
            None,
            &GitActor::User(user.user.id),
        )
        .await
        .expect("A merges");
    assert!(fast_forward.fast_forward, "A fast-forwards main");
    assert_eq!(fast_forward.commit, a1);

    let merge = service
        .merge_branch(
            fixture.project.id,
            &b1,
            "main",
            None,
            &GitActor::Session(session_c),
        )
        .await
        .expect("B merges");
    assert!(!merge.fast_forward, "B needs a merge commit");

    History {
        user,
        task_one,
        task_two,
        session_a,
        session_b,
        session_c,
        handoff_a,
        handoff_b,
        r2,
        a1,
        b1,
        m: merge.commit,
    }
}

fn commits(entries: &[HistoryEntry]) -> Vec<&str> {
    entries.iter().map(|entry| entry.commit.as_str()).collect()
}

fn ids<T>(items: &[T], id: impl Fn(&T) -> Uuid) -> Vec<Uuid> {
    items.iter().map(id).collect()
}

#[tokio::test]
async fn each_entry_names_the_tasks_and_sessions_behind_it() {
    let fixture = Fixture::create("history-attribution").await;
    let h = arrange(&fixture).await;

    let response = history(&fixture, &h.user, "").await;
    response.assert_status(StatusCode::OK);
    let entries = response.json::<Vec<HistoryEntry>>();

    assert_eq!(entries.len(), 4, "M, A1, R2 and R1");
    assert_eq!(commits(&entries[..3]), [h.m.as_str(), &h.a1, &h.r2]);

    // The merge commit: task two by its second parent, B as the hand-off's
    // source and C as the trailer's session.
    let merge = &entries[0];
    assert_eq!(merge.parents, [h.a1.clone(), h.b1.clone()]);
    assert_eq!(
        merge.requested_by.as_deref(),
        Some(format!("session:{}", h.session_c).as_str())
    );
    assert_eq!(ids(&merge.tasks, |task| task.id), [h.task_two.id]);
    assert_eq!(merge.tasks[0].handoff_id, h.handoff_b);
    assert_eq!(merge.tasks[0].number, h.task_two.number);
    assert_eq!(merge.tasks[0].title, "the second task");
    assert_eq!(
        ids(&merge.sessions, |session| session.id),
        [h.session_b, h.session_c]
    );

    // The fast-forwarded hand-off tip is its own entry, and names the newest
    // of the two records that pin it.
    let fast_forward = &entries[1];
    assert_eq!(fast_forward.requested_by, None);
    assert_eq!(fast_forward.subject, "feat: add A.md");
    assert_eq!(fast_forward.author_name, TEST_AUTHOR_NAME);
    assert_eq!(ids(&fast_forward.tasks, |task| task.id), [h.task_one.id]);
    assert_eq!(fast_forward.tasks[0].handoff_id, h.handoff_a);
    assert_eq!(
        ids(&fast_forward.sessions, |session| session.id),
        [h.session_a]
    );

    // Upstream's own commits: nothing pinned them.
    for entry in &entries[2..] {
        assert!(entry.tasks.is_empty(), "{entry:?}");
        assert!(entry.sessions.is_empty(), "{entry:?}");
    }
    assert!(entries[3].parents.is_empty(), "R1 is the root");

    // The same answer spelled explicitly, with the head named.
    let named = history(&fixture, &h.user, "?branch=refs/heads/main").await;
    named.assert_status(StatusCode::OK);
    assert_eq!(named.json::<Vec<HistoryEntry>>(), entries);
}

#[tokio::test]
async fn pages_follow_the_cursor_and_a_range_names_what_came_after_a_point() {
    let fixture = Fixture::create("history-pages").await;
    let h = arrange(&fixture).await;

    let first = history(&fixture, &h.user, "?limit=2").await;
    first.assert_status(StatusCode::OK);
    let first = first.json::<Vec<HistoryEntry>>();
    assert_eq!(commits(&first), [h.m.as_str(), &h.a1]);

    let second = history(&fixture, &h.user, &format!("?limit=2&before={}", h.a1)).await;
    second.assert_status(StatusCode::OK);
    let second = second.json::<Vec<HistoryEntry>>();
    assert_eq!(second.len(), 2);
    assert_eq!(second[0].commit, h.r2);

    let last = history(&fixture, &h.user, &format!("?before={}", second[1].commit)).await;
    last.assert_status(StatusCode::OK);
    assert!(last.json::<Vec<HistoryEntry>>().is_empty());

    // What a revert to R2 would take back: both tasks. To A1: task two only.
    let service = GitService::from_state(&fixture.app.state);
    let after_r2 = service
        .attribute_range(fixture.project.id, &h.m, Some(&h.r2))
        .await
        .expect("the range attributes");
    let mut tasks: Vec<Uuid> = after_r2
        .values()
        .flatten()
        .map(|handoff| handoff.task_id)
        .collect();
    tasks.sort();
    tasks.dedup();
    let mut expected = vec![h.task_one.id, h.task_two.id];
    expected.sort();
    assert_eq!(tasks, expected);
    assert_eq!(
        after_r2.get(&h.a1).map(Vec::len),
        Some(2),
        "both records pinning A1"
    );

    let after_a1 = service
        .attribute_range(fixture.project.id, &h.m, Some(&h.a1))
        .await
        .expect("the range attributes");
    assert_eq!(after_a1.keys().collect::<Vec<_>>(), [&h.b1]);
}

#[tokio::test]
async fn a_branch_that_is_not_an_integration_head_is_400() {
    let fixture = Fixture::create("history-branch").await;
    let user = fixture.signed_in();
    let session = fixture.seed_session().await;

    for query in [
        "?branch=origin/main".to_string(),
        format!("?branch={session}"),
        format!("?branch=refs/sessions/{session}"),
        "?branch=refs/tags/v1".to_string(),
        "?branch=no-such-branch".to_string(),
        "?branch=..".to_string(),
    ] {
        let response = history(&fixture, &user, &query).await;
        assert_status(&response, StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn an_unknown_cursor_and_a_zero_limit_are_400() {
    let fixture = Fixture::create("history-cursor").await;
    let h = arrange(&fixture).await;

    for query in [
        // Only the side branch reaches B1: it is not on main's own line.
        format!("?before={}", h.b1),
        "?before=0123456789abcdef0123456789abcdef01234567".to_string(),
        "?before=main".to_string(),
        "?limit=0".to_string(),
    ] {
        let response = history(&fixture, &h.user, &query).await;
        assert_status(&response, StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn no_token_is_401_and_an_unknown_project_is_404() {
    let fixture = Fixture::create("history-auth").await;
    let user = fixture.signed_in();

    let response = fixture
        .app
        .server
        .get(&format!("/api/projects/{}/git/history", fixture.project.id))
        .await;
    assert_status(&response, StatusCode::UNAUTHORIZED);

    let response = fixture
        .app
        .get_as(
            &user,
            &format!("/api/projects/{}/git/history", Uuid::new_v4()),
        )
        .await;
    assert_status(&response, StatusCode::NOT_FOUND);
}
