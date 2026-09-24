//! The auto-merge job (`ARCHITECTURE.md`, "Task tracker" → "Automatic
//! merges" and "Background jobs"; `SPEC.md`, "TaskEvent"; ADR 0045).
//!
//! The job is invoked directly, one run at a time, over real repositories:
//! `common::handoffs::Fixture` builds a bare upstream, the project repository
//! and session work clones, and every hand-off is published and reviewed
//! through `PUT /projects/{pid}/tasks/{id}` the way a user does it. Git is
//! never mocked (`CLAUDE.md`, "Testing expectations").
//!
//! What is asserted:
//!
//! - an approved hand-off in an auto-merge state is merged into the default
//!   branch with a `Requested-By: system` merge commit, and the task moves to
//!   the first terminal state with `Merged <commit> into <branch>.` — which
//!   closes its parent and unblocks its dependant, as any terminal move does;
//! - a conflict leaves the default branch where it was and sends the task to
//!   the conflict state with the paths, keeping the approved hand-off
//!   current; at the round limit the same send-back escalates instead;
//! - a task with an unapproved hand-off, or none, is escalated;
//! - held and blocked tasks and paused projects are left alone;
//! - a hand-off already on the default branch (a run that crashed before its
//!   tracker commit) only moves the task;
//! - a task a person moved out between the merge and the tracker commit gets
//!   the "had left" comment and keeps its state.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use chrono::Utc;
use common::AuthenticatedUser;
use common::handoffs::Fixture;
use common::tracker::{block, hold, in_mutation, locked};
use mars_orchestrator::cron::JobReport;
use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::git::GitRef;
use mars_orchestrator::git::refs;
use mars_orchestrator::git::testutil::{run_git, test_identity};
use mars_orchestrator::git::{create_work_clone, resolve_base};
use mars_orchestrator::models::{
    AutoMergeInput, MaxRounds, NewTask, ProjectUpdate, ReviewStatus, Task, TaskDependencyKind,
    TaskRef,
};
use mars_orchestrator::repositories::{ProjectRepository, TaskRepository};
use mars_orchestrator::tracker::state::change_state;
use mars_orchestrator::tracker::{
    StateChangeOptions, StateUpdate, TaskDto, TrackerMutation, add_dependency, update_state,
};
use serde_json::{Value, json};
use uuid::Uuid;

// ---- arrangement ----

/// A fixture whose `merge` state has auto-merge on, with `ready` as its
/// conflict state — what a new project is seeded with.
async fn fixture(name: &str) -> Fixture {
    let fixture = Fixture::create(name).await;

    in_mutation(
        &fixture.app.pool,
        fixture.project.id,
        TaskActor::System,
        async |m| {
            update_state(
                m,
                "merge",
                StateUpdate {
                    auto_merge: Some(AutoMergeInput {
                        auto_merge: true,
                        conflict_state: Some("ready".to_string()),
                    }),
                    ..StateUpdate::default()
                },
            )
            .await
        },
    )
    .await
    .expect("auto-merge turns on");

    fixture
}

/// `PUT` a task with a hand-off as the fixture's user.
async fn publish(
    fixture: &Fixture,
    user: &AuthenticatedUser,
    task_id: Uuid,
    state: &str,
    handoff: Value,
) -> TaskDto {
    let response = fixture
        .app
        .put_as(
            user,
            &format!("/api/projects/{}/tasks/{}", fixture.project.id, task_id),
        )
        .json(&json!({ "state": state, "handoff": handoff }))
        .await;

    response.assert_status(StatusCode::OK);
    response.json::<TaskDto>()
}

/// A task in `ready` whose revision (one commit to `NOTES.md`) was published
/// into `review`. Returns the task, its hand-off id and the pinned commit.
async fn under_review(fixture: &Fixture, user: &AuthenticatedUser, task: &Task) -> (Uuid, String) {
    let (session_id, pinned) = fixture.session_with_commit("feat: the work").await;
    let published = publish(
        fixture,
        user,
        task.id,
        "review",
        json!({
            "kind": "revision",
            "source_session_id": session_id,
            "commit": pinned,
            "comment": "ready for review",
        }),
    )
    .await;

    (
        published.handoff.expect("the revision publishes").id,
        pinned,
    )
}

/// Approve the current hand-off and forward the task into `merge`.
async fn approve_into_merge(
    fixture: &Fixture,
    user: &AuthenticatedUser,
    task_id: Uuid,
    handoff_id: Uuid,
) -> Uuid {
    publish(
        fixture,
        user,
        task_id,
        "merge",
        json!({
            "kind": "forward",
            "handoff_id": handoff_id,
            "review": "approved",
            "comment": "looks good",
        }),
    )
    .await
    .handoff
    .expect("the forward publishes")
    .id
}

/// A task in `ready` whose approved hand-off sits in `merge`, pinning a commit
/// to `NOTES.md`.
async fn approved(fixture: &Fixture, user: &AuthenticatedUser, title: &str) -> (Task, String) {
    let task = fixture.task(title, "ready").await;
    let (handoff_id, pinned) = under_review(fixture, user, &task).await;
    approve_into_merge(fixture, user, task.id, handoff_id).await;

    (task, pinned)
}

/// Land another session's commit to `NOTES.md` on `main` through the branch
/// merge, so the approved hand-off can no longer merge cleanly.
async fn conflicting_work_on_main(fixture: &Fixture, user: &AuthenticatedUser) {
    let (other, _) = fixture.session_with_commit("feat: the other work").await;
    fixture
        .app
        .post_as(
            user,
            &format!("/api/projects/{}/git/merge", fixture.project.id),
        )
        .json(&json!({ "target": "main", "source": other.to_string() }))
        .await
        .assert_status(StatusCode::OK);
}

/// A session with a work clone holding one commit to `file`, independent of
/// the `NOTES.md` line of work.
async fn session_with_file(fixture: &Fixture, file: &str) -> Uuid {
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
    fixture.commit_in_work_clone(session_id, file, file).await;

    session_id
}

async fn run(fixture: &Fixture) -> JobReport {
    fixture
        .app
        .cron()
        .auto_merge(Utc::now())
        .await
        .expect("the job runs")
}

async fn commit_of(fixture: &Fixture, name: &str) -> String {
    let git_ref = GitRef::parse(name).expect("a parsable ref name");
    refs::resolve(&fixture.paths().project_repo(fixture.project.id), &git_ref)
        .await
        .unwrap_or_else(|err| panic!("{name} resolves: {err}"))
        .commit
}

async fn files_in(fixture: &Fixture, name: &str) -> Vec<String> {
    run_git(
        &fixture.paths().project_repo(fixture.project.id),
        &["ls-tree", "--name-only", "-r", name],
    )
    .await
    .lines()
    .map(str::to_string)
    .collect()
}

/// The name of the state a task is in now.
async fn state_of(fixture: &Fixture, task_id: Uuid) -> String {
    let task = fixture.task_row(task_id).await;
    TaskRepository::new(&fixture.app.pool)
        .list_states(fixture.project.id)
        .await
        .expect("the states read")
        .into_iter()
        .find(|state| state.id == task.state_id)
        .expect("the task's state exists")
        .name
}

/// The task's system comments, oldest first.
async fn system_comments(fixture: &Fixture, task_id: Uuid) -> Vec<String> {
    TaskRepository::new(&fixture.app.pool)
        .list_comments(fixture.project.id, task_id)
        .await
        .expect("the comments read")
        .into_iter()
        .filter(|comment| comment.system)
        .map(|comment| comment.body)
        .collect()
}

async fn events_after(fixture: &Fixture, seq: i64) -> Vec<TaskEvent> {
    TaskRepository::new(&fixture.app.pool)
        .list_task_events_after(fixture.project.id, seq, 1000)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
        .collect()
}

async fn cursor(fixture: &Fixture) -> i64 {
    events_after(fixture, 0)
        .await
        .last()
        .map(|event| event.seq)
        .unwrap_or(0)
}

/// Insert a task of this project with `parent_id`, through a mutation.
async fn child_of(fixture: &Fixture, parent: Uuid, title: &str) -> Task {
    let mut new = NewTask::new(fixture.project.id, title).expect("the title parses");
    new.state_id = Some(fixture.state("ready").await.id);
    new.parent_id = Some(parent);

    let mut mutation =
        TrackerMutation::begin(&fixture.app.pool, fixture.project.id, TaskActor::System)
            .await
            .expect("the mutation opens");
    let inserted = TaskRepository::new(&fixture.app.pool)
        .insert_task(mutation.conn(), fixture.project.id, &new)
        .await
        .expect("the task inserts");
    mutation.commit().await.expect("the mutation commits");

    inserted
}

async fn update_project(fixture: &Fixture, update: ProjectUpdate) {
    let mut tx = fixture
        .app
        .pool
        .begin()
        .await
        .expect("a transaction begins");
    ProjectRepository::new(&fixture.app.pool)
        .update(&mut tx, fixture.project.id, &update)
        .await
        .expect("the project updates")
        .expect("the project exists");
    tx.commit().await.expect("the transaction commits");
}

// ---- the merge ----

#[tokio::test]
async fn an_approved_handoff_is_merged_and_the_task_closes_its_parent_and_unblocks_its_dependant() {
    let fixture = fixture("auto-merge-approved").await;
    let user = fixture.signed_in();

    let parent = fixture.task("the epic", "backlog").await;
    let task = child_of(&fixture, parent.id, "the only child").await;
    let dependant = fixture.task("waits for it", "ready").await;
    in_mutation(
        &fixture.app.pool,
        fixture.project.id,
        TaskActor::System,
        async |m| {
            let dependant = locked(m, dependant.id).await?;
            let task = locked(m, task.id).await?;
            add_dependency(m, &dependant, &task, TaskDependencyKind::Blocks).await
        },
    )
    .await
    .expect("the dependency is added");
    assert!(fixture.task_row(dependant.id).await.blocked);

    let (handoff_id, pinned) = under_review(&fixture, &user, &task).await;
    approve_into_merge(&fixture, &user, task.id, handoff_id).await;
    let from = cursor(&fixture).await;

    let report = run(&fixture).await;
    assert_eq!(report.items, 1, "{report:?}");
    assert_eq!(report.failures, 0, "{report:?}");

    // The code is on `main`.
    let files = files_in(&fixture, "main").await;
    assert!(files.contains(&"NOTES.md".to_string()), "{files:?}");

    // The task is closed with the documented comment.
    assert_eq!(state_of(&fixture, task.id).await, "done");
    assert!(fixture.task_row(task.id).await.closed_at.is_some());
    assert_eq!(
        system_comments(&fixture, task.id).await.last(),
        Some(&format!("Merged {pinned} into main.")),
    );

    // Everything a terminal move composes followed.
    assert_eq!(
        state_of(&fixture, parent.id).await,
        "done",
        "the parent closed"
    );
    assert!(
        !fixture.task_row(dependant.id).await.blocked,
        "the dependant is unblocked",
    );

    let events = events_after(&fixture, from).await;
    let moved = events
        .iter()
        .find(|event| event.kind == TaskEventKind::StateChanged && event.task_id == Some(task.id))
        .expect("the task's move is an event");
    assert_eq!(moved.actor, TaskActor::System);
    assert_eq!(moved.from.as_deref(), Some("merge"));
    assert_eq!(moved.to.as_deref(), Some("done"));
    assert!(
        events
            .iter()
            .any(|event| event.kind == TaskEventKind::Unblocked
                && event.task_id == Some(dependant.id)),
        "the dependant's unblocked is in the stream",
    );

    // A second run finds nothing.
    assert_eq!(run(&fixture).await, JobReport::default());
}

#[tokio::test]
async fn a_merge_that_needs_a_merge_commit_carries_the_system_trailer() {
    let fixture = fixture("auto-merge-trailer").await;
    let user = fixture.signed_in();
    let (task, pinned) = approved(&fixture, &user, "behind main").await;

    // Unrelated work lands on `main` first: behind is not a conflict, and the
    // merge is a real merge commit whose message can be read.
    let other = session_with_file(&fixture, "OTHER.md").await;
    fixture
        .app
        .post_as(
            &user,
            &format!("/api/projects/{}/git/merge", fixture.project.id),
        )
        .json(&json!({ "target": "main", "source": other.to_string() }))
        .await
        .assert_status(StatusCode::OK);

    let report = run(&fixture).await;
    assert_eq!(report.items, 1, "{report:?}");

    let main = commit_of(&fixture, "main").await;
    assert_ne!(main, pinned, "a merge commit was expected");
    let message = run_git(
        &fixture.paths().project_repo(fixture.project.id),
        &["log", "-1", "--format=%B", "main"],
    )
    .await;
    assert!(
        message.contains("Requested-By: system"),
        "the merge commit names the system: {message}",
    );
    let files = files_in(&fixture, "main").await;
    assert!(
        files.contains(&"NOTES.md".to_string()) && files.contains(&"OTHER.md".to_string()),
        "{files:?}",
    );
    assert_eq!(state_of(&fixture, task.id).await, "done");
}

// ---- conflicts ----

#[tokio::test]
async fn a_conflict_sends_the_task_back_with_the_paths_and_leaves_main_alone() {
    let fixture = fixture("auto-merge-conflict").await;
    let user = fixture.signed_in();
    let (task, _) = approved(&fixture, &user, "conflicting").await;
    let handoff_before = fixture.task_row(task.id).await.current_handoff_id;

    conflicting_work_on_main(&fixture, &user).await;
    let main_before = commit_of(&fixture, "main").await;
    let from = cursor(&fixture).await;

    let report = run(&fixture).await;
    assert_eq!(report.items, 1, "{report:?}");
    assert_eq!(report.failures, 0, "{report:?}");

    assert_eq!(commit_of(&fixture, "main").await, main_before);
    assert_eq!(state_of(&fixture, task.id).await, "ready");
    assert_eq!(
        system_comments(&fixture, task.id).await.last(),
        Some(
            &"Merge into main conflicted; bring the branch up to date and hand off a new \
              revision. Conflicting paths:\nNOTES.md"
                .to_string()
        ),
    );

    // The approved hand-off stays current, so the next implementer starts
    // from it.
    let row = fixture.task_row(task.id).await;
    assert_eq!(row.current_handoff_id, handoff_before);
    let handoff = TaskRepository::new(&fixture.app.pool)
        .find_handoff(fixture.project.id, row.current_handoff_id.expect("current"))
        .await
        .expect("the hand-off reads")
        .expect("the hand-off exists");
    assert_eq!(handoff.review_status, ReviewStatus::Approved);

    let events = events_after(&fixture, from).await;
    let moved = events
        .iter()
        .find(|event| event.task_id == Some(task.id) && event.kind == TaskEventKind::StateChanged)
        .expect("the send-back is a state change");
    assert_eq!(moved.actor, TaskActor::System);
    assert_eq!(moved.to.as_deref(), Some("ready"));
}

#[tokio::test]
async fn a_conflict_at_the_round_limit_escalates() {
    let fixture = fixture("auto-merge-limit").await;
    let user = fixture.signed_in();
    update_project(
        &fixture,
        ProjectUpdate {
            max_rounds: Some(MaxRounds::parse(1).expect("the limit validates")),
            ..ProjectUpdate::default()
        },
    )
    .await;

    // One revision publication: `rounds` is 1, at the limit.
    let (task, _) = approved(&fixture, &user, "one round too many").await;
    assert_eq!(fixture.task_row(task.id).await.rounds, 1);
    conflicting_work_on_main(&fixture, &user).await;
    let main_before = commit_of(&fixture, "main").await;
    let from = cursor(&fixture).await;

    let report = run(&fixture).await;
    assert_eq!(report.items, 1, "{report:?}");

    assert_eq!(commit_of(&fixture, "main").await, main_before);
    assert_eq!(state_of(&fixture, task.id).await, "needs_human");
    let reason = fixture
        .task_row(task.id)
        .await
        .needs_human_reason
        .expect("a reason is recorded");
    assert!(
        reason.starts_with("round limit reached (1/1): Merge into main conflicted;"),
        "{reason}",
    );

    let events = events_after(&fixture, from).await;
    assert!(
        events
            .iter()
            .any(|event| event.task_id == Some(task.id) && event.kind == TaskEventKind::Escalated),
        "the redirected send-back is an escalation",
    );
}

// ---- nothing approved ----

#[tokio::test]
async fn an_unapproved_handoff_is_escalated_and_nothing_is_merged() {
    let fixture = fixture("auto-merge-unapproved").await;
    let user = fixture.signed_in();
    let task = fixture.task("moved by hand", "ready").await;
    let (session_id, pinned) = fixture.session_with_commit("feat: unreviewed").await;
    // Published straight into `merge`, unreviewed.
    publish(
        &fixture,
        &user,
        task.id,
        "merge",
        json!({
            "kind": "revision",
            "source_session_id": session_id,
            "commit": pinned,
            "comment": "skipping review",
        }),
    )
    .await;
    let main_before = commit_of(&fixture, "main").await;
    let from = cursor(&fixture).await;

    let report = run(&fixture).await;
    assert_eq!(report.items, 1, "{report:?}");

    assert_eq!(commit_of(&fixture, "main").await, main_before);
    assert_eq!(state_of(&fixture, task.id).await, "needs_human");
    let reason = "merge is an auto-merge state and the task has no approved hand-off";
    assert_eq!(
        fixture
            .task_row(task.id)
            .await
            .needs_human_reason
            .as_deref(),
        Some(reason),
    );

    let events = events_after(&fixture, from).await;
    let escalated = events
        .iter()
        .find(|event| event.task_id == Some(task.id) && event.kind == TaskEventKind::Escalated)
        .expect("the move is an escalation");
    assert_eq!(escalated.actor, TaskActor::System);
    assert_eq!(escalated.reason.as_deref(), Some(reason));
}

#[tokio::test]
async fn a_task_with_no_handoff_is_escalated() {
    let fixture = fixture("auto-merge-no-handoff").await;
    let task = fixture.task("dragged into merge", "merge").await;

    let report = run(&fixture).await;
    assert_eq!(report.items, 1, "{report:?}");

    assert_eq!(state_of(&fixture, task.id).await, "needs_human");
    assert_eq!(
        fixture
            .task_row(task.id)
            .await
            .needs_human_reason
            .as_deref(),
        Some("merge is an auto-merge state and the task has no approved hand-off"),
    );
}

// ---- left alone ----

#[tokio::test]
async fn held_and_blocked_tasks_are_skipped() {
    let fixture = fixture("auto-merge-held").await;
    let user = fixture.signed_in();
    let (held, _) = approved(&fixture, &user, "held").await;
    let session_id = fixture.seed_session().await;
    hold(&fixture.app.pool, fixture.project.id, held.id, session_id).await;

    let blocked = fixture.task("blocked", "merge").await;
    block(&fixture.app.pool, fixture.project.id, blocked.id).await;

    let main_before = commit_of(&fixture, "main").await;
    let report = run(&fixture).await;
    assert_eq!(report, JobReport::default());

    assert_eq!(commit_of(&fixture, "main").await, main_before);
    assert_eq!(state_of(&fixture, held.id).await, "merge");
    assert_eq!(state_of(&fixture, blocked.id).await, "merge");
}

#[tokio::test]
async fn a_paused_project_is_skipped() {
    let fixture = fixture("auto-merge-paused").await;
    let user = fixture.signed_in();
    let (task, _) = approved(&fixture, &user, "paused").await;
    update_project(
        &fixture,
        ProjectUpdate {
            automation_paused: Some(true),
            ..ProjectUpdate::default()
        },
    )
    .await;
    let main_before = commit_of(&fixture, "main").await;

    let report = run(&fixture).await;
    assert_eq!(report.items, 0, "{report:?}");
    assert_eq!(report.skipped, 1, "{report:?}");

    assert_eq!(commit_of(&fixture, "main").await, main_before);
    assert_eq!(state_of(&fixture, task.id).await, "merge");
}

// ---- repeats and races ----

#[tokio::test]
async fn a_handoff_already_on_main_only_moves_the_task() {
    let fixture = fixture("auto-merge-crash").await;
    let user = fixture.signed_in();
    let (task, pinned) = approved(&fixture, &user, "merged before a crash").await;

    // What a run that crashed after its merge left behind: the code on `main`
    // and the task still in `merge`.
    let handoff_id = fixture
        .task_row(task.id)
        .await
        .current_handoff_id
        .expect("a current hand-off");
    fixture
        .app
        .post_as(
            &user,
            &format!("/api/projects/{}/git/merge", fixture.project.id),
        )
        .json(&json!({ "target": "main", "task_id": task.id, "handoff_id": handoff_id }))
        .await
        .assert_status(StatusCode::OK);
    let main_before = commit_of(&fixture, "main").await;

    let report = run(&fixture).await;
    assert_eq!(report.items, 1, "{report:?}");

    assert_eq!(
        commit_of(&fixture, "main").await,
        main_before,
        "nothing is merged twice",
    );
    assert_eq!(state_of(&fixture, task.id).await, "done");
    assert_eq!(
        system_comments(&fixture, task.id).await.last(),
        Some(&format!("Merged {pinned} into main.")),
    );
}

#[tokio::test]
async fn a_task_moved_out_during_the_merge_gets_the_comment_and_keeps_its_state() {
    let fixture = fixture("auto-merge-race").await;
    let user = fixture.signed_in();
    let (task, pinned) = approved(&fixture, &user, "moved away").await;
    let main_before = commit_of(&fixture, "main").await;

    // A person's move holds the project row and takes no git lock. Opened
    // before the run, it lets the job re-read and merge, and then makes its
    // tracker mutation wait until the move has committed.
    let mut person = TrackerMutation::begin(
        &fixture.app.pool,
        fixture.project.id,
        TaskActor::User {
            user_id: fixture.user.id,
        },
    )
    .await
    .expect("the move opens");

    let cron = fixture.app.cron();
    let job = tokio::spawn(async move { cron.auto_merge(Utc::now()).await });

    // Wait for the merge to land, polling the ref rather than sleeping for a
    // fixed period.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while commit_of(&fixture, "main").await == main_before {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the job never merged",
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let row = TaskRepository::new(&fixture.app.pool)
        .find_task_for_update(person.conn(), fixture.project.id, TaskRef::Id(task.id))
        .await
        .expect("the task reads")
        .expect("the task exists");
    let ready = fixture.state("ready").await;
    change_state(&mut person, &row, &ready, StateChangeOptions::default())
        .await
        .expect("the person's move");
    person.commit().await.expect("the move commits");

    let report = job.await.expect("the job joins").expect("the job runs");
    assert_eq!(report.items, 1, "{report:?}");
    assert_eq!(report.failures, 0, "{report:?}");

    assert_eq!(state_of(&fixture, task.id).await, "ready");
    assert_eq!(
        system_comments(&fixture, task.id).await.last(),
        Some(&format!(
            "Merged {pinned} into main, but the task had left merge; its state is unchanged."
        )),
    );
}
