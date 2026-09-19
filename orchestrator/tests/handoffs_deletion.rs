//! What deletion does to retained hand-off refs (`ARCHITECTURE.md`, "Git
//! model" → Serialization and "Task tracker" → "Code hand-offs";
//! `docs/data-model.md`, `task_handoffs`).
//!
//! Three deletions, three different answers, and this suite is where they are
//! told apart:
//!
//! - **task** deletion takes the project git lock, deletes the rows and then
//!   removes every `refs/handoffs/<id>` the task had — the commits themselves
//!   stay reachable through `refs/sessions/<sid>`, because nothing runs `gc`;
//! - **project** deletion needs no per-ref work at all: the whole repository
//!   goes with the project directory, and the test here is what says so;
//! - **session** deletion touches neither the hand-off row nor its ref:
//!   "historical rows and their git refs survive source-session deletion"
//!   (`docs/data-model.md`), with `source_session_id` left null by the
//!   foreign key.
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"): the fixture is
//! `common::handoffs::Fixture`, so the refs asserted about are real refs in a
//! real repository, published through the real `PUT` endpoint.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use common::AuthenticatedUser;
use common::handoffs::Fixture;
use mars_orchestrator::git::testutil::run_git;
use mars_orchestrator::git::{GitRef, refs};
use mars_orchestrator::models::{
    NewSession, ProfileKind, Session, SessionState, StateChange, Task, TaskRef,
};
use mars_orchestrator::repositories::{SessionRepository, TaskRepository};
use mars_orchestrator::tracker::TaskDto;
use serde_json::{Value, json};
use uuid::Uuid;

// ---- helpers ----

fn task_path(fixture: &Fixture, task_id: Uuid) -> String {
    format!("/api/projects/{}/tasks/{}", fixture.project.id, task_id)
}

/// Publish a revision of `commit` from `session_id`, moving the task to
/// `state`.
async fn revision(
    fixture: &Fixture,
    user: &AuthenticatedUser,
    task: &Task,
    state: &str,
    session_id: Uuid,
    commit: &str,
) -> TaskDto {
    put_handoff(
        fixture,
        user,
        task,
        state,
        json!({
            "kind": "revision",
            "source_session_id": session_id,
            "commit": commit,
            "comment": "ready for review",
        }),
    )
    .await
}

/// Forward the task's current hand-off, moving the task to `state`.
async fn forward(
    fixture: &Fixture,
    user: &AuthenticatedUser,
    task: &Task,
    state: &str,
    handoff_id: Uuid,
) -> TaskDto {
    put_handoff(
        fixture,
        user,
        task,
        state,
        json!({
            "kind": "forward",
            "handoff_id": handoff_id,
            "comment": "passing it on",
        }),
    )
    .await
}

async fn put_handoff(
    fixture: &Fixture,
    user: &AuthenticatedUser,
    task: &Task,
    state: &str,
    handoff: Value,
) -> TaskDto {
    let response = fixture
        .app
        .put_as(user, &task_path(fixture, task.id))
        .json(&json!({ "state": state, "handoff": handoff }))
        .await;

    response.assert_status(StatusCode::OK);
    response.json::<TaskDto>()
}

/// The id of the task's current hand-off, as the published task carries it.
fn current_handoff(published: &TaskDto) -> Uuid {
    published
        .handoff
        .as_ref()
        .expect("the published task carries its hand-off")
        .id
}

/// A session row launched *for* a task and a hand-off, the way
/// `POST /projects/{pid}/sessions` with a `task_id` records one: `handoff_id`
/// names the record it checked out and `base_ref` holds that commit
/// (`docs/data-model.md`, `sessions`).
async fn session_launched_for(
    fixture: &Fixture,
    task_id: Uuid,
    handoff_id: Uuid,
    commit: &str,
) -> Session {
    let mut new_session = NewSession::new(
        fixture.project.id,
        fixture.profile_id,
        ProfileKind::Conversational,
        commit,
        // Not a credential: a fake stand-in for the hashed MCP token (rule 3).
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );
    new_session.created_by = Some(fixture.user.id);
    new_session.task_id = Some(task_id);
    new_session.handoff_id = Some(handoff_id);

    let mut tx = fixture
        .app
        .pool
        .begin()
        .await
        .expect("a transaction begins");
    let inserted = SessionRepository::new(&fixture.app.pool)
        .insert(&mut tx, &new_session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
}

/// Move a session to a state `DELETE` accepts, the way its own lifecycle
/// would.
async fn fail_session(fixture: &Fixture, session_id: Uuid) {
    let mut tx = fixture
        .app
        .pool
        .begin()
        .await
        .expect("a transaction begins");
    SessionRepository::new(&fixture.app.pool)
        .set_state(
            &mut tx,
            session_id,
            SessionState::Failed,
            &StateChange::failed("arranged by the test"),
        )
        .await
        .expect("the transition is legal");
    tx.commit().await.expect("the transaction commits");
}

/// Every task event kind of this project, oldest first.
async fn event_kinds(fixture: &Fixture) -> Vec<String> {
    TaskRepository::new(&fixture.app.pool)
        .list_task_events_after(fixture.project.id, 0, 100)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| row.kind)
        .collect()
}

/// Assert `commit` is still an object in the project repository.
async fn commit_exists(fixture: &Fixture, commit: &str) {
    let resolved = run_git(
        &fixture.paths().project_repo(fixture.project.id),
        &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
    )
    .await;

    assert_eq!(resolved.trim(), commit, "the commit object is gone");
}

/// A task with three hand-offs: two revisions and a forward of the second.
///
/// Three rows, three ids, three refs — the arrangement the deletion has to
/// clear completely.
async fn task_with_three_handoffs(
    fixture: &Fixture,
    user: &AuthenticatedUser,
) -> (Task, Uuid, Vec<Uuid>) {
    let (session_id, first) = fixture.session_with_commit("feat: the first pass").await;
    let task = fixture.task("implement it", "ready").await;

    let published = revision(fixture, user, &task, "review", session_id, &first).await;
    let one = current_handoff(&published);

    let second = fixture
        .commit_in_work_clone(session_id, "MORE.md", "feat: the second pass")
        .await;
    let published = revision(fixture, user, &task, "ready", session_id, &second).await;
    let two = current_handoff(&published);

    let published = forward(fixture, user, &task, "review", two).await;
    let three = current_handoff(&published);

    (task, session_id, vec![one, two, three])
}

// ---- task deletion ----

#[tokio::test]
async fn a_task_deletion_removes_every_hand_off_ref_and_leaves_the_commits() {
    let fixture = Fixture::create("deletion-refs").await;
    let user = fixture.signed_in();
    let (task, session_id, handoff_ids) = task_with_three_handoffs(&fixture, &user).await;

    let before: Vec<Uuid> = fixture
        .handoff_refs()
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    for id in &handoff_ids {
        assert!(
            before.contains(id),
            "the arrangement pinned no ref for {id}"
        );
    }
    let commit = fixture
        .handoff_refs()
        .await
        .into_iter()
        .find(|(id, _)| *id == handoff_ids[0])
        .expect("the first hand-off has a ref")
        .1;

    let response = fixture
        .app
        .delete_as(&user, &task_path(&fixture, task.id))
        .await;
    response.assert_status(StatusCode::NO_CONTENT);
    response.assert_text("");

    let after: Vec<Uuid> = fixture
        .handoff_refs()
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    for id in &handoff_ids {
        assert!(!after.contains(id), "the ref for {id} is still pinned");
    }

    // Only the refs went: the session ref still holds the work, and the
    // commit the first hand-off pinned is still an object (no `gc`).
    let session_ref = refs::resolve(
        &fixture.paths().project_repo(fixture.project.id),
        &GitRef::Session(session_id),
    )
    .await
    .expect("the session ref resolves");
    assert!(!session_ref.commit.is_empty());
    commit_exists(&fixture, &commit).await;

    // The rows went with the task, and the deletion was announced.
    assert!(
        TaskRepository::new(&fixture.app.pool)
            .find_task(fixture.project.id, TaskRef::Id(task.id))
            .await
            .expect("the lookup runs")
            .is_none(),
        "the task row is left",
    );
    assert!(
        TaskRepository::new(&fixture.app.pool)
            .list_handoffs(fixture.project.id, task.id)
            .await
            .expect("the hand-offs read")
            .is_empty(),
        "the hand-off rows did not cascade",
    );
    assert_eq!(
        event_kinds(&fixture).await.last().map(String::as_str),
        Some("deleted"),
        "the deletion emitted no `deleted` event",
    );
}

#[tokio::test]
async fn a_task_deletion_leaves_a_session_that_used_its_hand_off_with_a_null_handoff_id() {
    let fixture = Fixture::create("deletion-session-fk").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;

    let published = revision(&fixture, &user, &task, "review", session_id, &commit).await;
    let handoff_id = current_handoff(&published);
    let launched = session_launched_for(&fixture, task.id, handoff_id, &commit).await;
    assert_eq!(launched.handoff_id, Some(handoff_id));

    let response = fixture
        .app
        .delete_as(&user, &task_path(&fixture, task.id))
        .await;
    response.assert_status(StatusCode::NO_CONTENT);

    // `sessions.handoff_id` is `ON DELETE SET NULL` and `sessions.task_id`
    // likewise; the commit string in `base_ref` is a plain column and stays
    // (`docs/data-model.md`, `sessions`).
    let reread = SessionRepository::new(&fixture.app.pool)
        .find(launched.id)
        .await
        .expect("the lookup runs")
        .expect("the session survives its task");
    assert_eq!(reread.handoff_id, None);
    assert_eq!(reread.base_ref, commit);
}

#[tokio::test]
async fn a_task_without_hand_offs_is_still_deleted() {
    let fixture = Fixture::create("deletion-no-handoffs").await;
    let user = fixture.signed_in();
    let task = fixture.task("nothing to hand off", "ready").await;

    let response = fixture
        .app
        .delete_as(&user, &task_path(&fixture, task.id))
        .await;
    response.assert_status(StatusCode::NO_CONTENT);

    assert!(
        TaskRepository::new(&fixture.app.pool)
            .find_task(fixture.project.id, TaskRef::Id(task.id))
            .await
            .expect("the lookup runs")
            .is_none(),
        "the task row is left",
    );
    assert!(
        fixture.handoff_refs().await.is_empty(),
        "a project with no hand-offs grew a ref",
    );
}

#[tokio::test]
async fn an_unknown_task_is_not_found_and_leaves_the_refs_alone() {
    let fixture = Fixture::create("deletion-unknown").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;
    revision(&fixture, &user, &task, "review", session_id, &commit).await;

    let before = fixture.handoff_refs().await;

    let response = fixture
        .app
        .delete_as(&user, &task_path(&fixture, Uuid::new_v4()))
        .await;
    response.assert_status(StatusCode::NOT_FOUND);

    assert_eq!(
        before,
        fixture.handoff_refs().await,
        "a refused deletion touched the refs",
    );
}

// ---- project deletion ----

#[tokio::test]
async fn a_project_deletion_takes_the_repository_and_its_hand_off_refs_with_it() {
    let fixture = Fixture::create("deletion-project").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;
    revision(&fixture, &user, &task, "review", session_id, &commit).await;
    assert_eq!(fixture.handoff_refs().await.len(), 1);

    // A project with a live session is refused (`SPEC.md`, "Projects"), and
    // the fixture's session is still `creating`.
    fail_session(&fixture, session_id).await;

    let response = fixture
        .app
        .delete_as(&user, &format!("/api/projects/{}", fixture.project.id))
        .await;
    response.assert_status(StatusCode::NO_CONTENT);

    // No per-ref work: the whole repository goes with the project directory,
    // which is why project deletion needs nothing from
    // `tracker::handoffs` (`ARCHITECTURE.md`, "Storage").
    assert!(
        !fixture.paths().project_repo(fixture.project.id).exists(),
        "the project repository is left behind",
    );
}

// ---- session deletion ----

#[tokio::test]
async fn a_session_deletion_keeps_the_hand_off_row_and_its_ref() {
    let fixture = Fixture::create("deletion-source-session").await;
    let user = fixture.signed_in();
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;

    let published = revision(&fixture, &user, &task, "review", session_id, &commit).await;
    let handoff_id = current_handoff(&published);

    fail_session(&fixture, session_id).await;
    let response = fixture
        .app
        .delete_as(&user, &format!("/api/sessions/{session_id}"))
        .await;
    response.assert_status(StatusCode::NO_CONTENT);

    // The record survives with `source_session_id` cleared by the foreign key,
    // and its ref still pins the commit (`docs/data-model.md`,
    // `task_handoffs`).
    let handoffs = TaskRepository::new(&fixture.app.pool)
        .list_handoffs(fixture.project.id, task.id)
        .await
        .expect("the hand-offs read");
    assert_eq!(handoffs.len(), 1);
    assert_eq!(handoffs[0].id, handoff_id);
    assert_eq!(handoffs[0].source_session_id, None);
    assert_eq!(handoffs[0].commit, commit);

    assert_eq!(
        fixture.handoff_refs().await,
        vec![(handoff_id, commit.clone())],
        "the hand-off ref went with the source session",
    );
    commit_exists(&fixture, &commit).await;
}
