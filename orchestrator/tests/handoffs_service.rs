//! `tracker::handoffs::HandoffService::update_with_handoff`: the composition
//! of the two halves of publishing a code hand-off (`ARCHITECTURE.md`, "Task
//! tracker" → "Code hand-offs" and "One mutation at a time per project",
//! "Git model" → Serialization; `SPEC.md`, "Tasks" and "Code hand-offs and
//! review"; ADR 0018, 0021, 0030).
//!
//! `tests/tracker_handoffs.rs` covers the git half on its own and
//! `tests/handoffs_transaction.rs` the database half on its own. What is
//! asserted here is the seam between them, end to end over real repositories:
//!
//! - a revision by the holding session pins the ref, writes the record, the
//!   comment and the events, moves the task and clears the lease;
//! - every way of being refused — a commit that is not the session tip, a
//!   stale `handoff_id`, a task that moved while the commit was being pinned,
//!   a project that is not `ready`, an unknown state, an unknown task — leaves
//!   the task, the lease and `refs/handoffs/*` exactly as they were, with no
//!   `task_events`, no `task_sessions` rows and no hand-off rows;
//! - two sessions publishing on one task, only one of which holds the lease,
//!   produce exactly one publication.
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"): the fixture is
//! `common::handoffs::Fixture`, which builds a real upstream, a real project
//! repository and real session work clones.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use common::handoffs::Fixture;
use mars_orchestrator::events::{TaskEvent, TaskEventKind};
use mars_orchestrator::git::refs;
use mars_orchestrator::git::testutil::run_git;
use mars_orchestrator::models::{
    HandoffCaller, HandoffInput, ProjectStatus, ReviewStatus, Task, TaskRef,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{ProjectRepository, TaskRepository};
use mars_orchestrator::tracker::handoffs::{HANDOFF_RECHECK_FAILED, HandoffService};
use mars_orchestrator::tracker::{TaskDto, UpdateTaskInput};
use uuid::Uuid;

/// An obviously fake but well-formed object id, for the cases that name a
/// commit the repository does not have (rule 3).
const ABSENT_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// How long a raced case waits for the other half to reach its lock before it
/// gives up and fails rather than hanging the harness.
const RACE_TIMEOUT: Duration = Duration::from_secs(20);

/// The state-only update every hand-off carries at minimum.
fn move_to(state: &str) -> UpdateTaskInput {
    UpdateTaskInput {
        state: Some(state.to_string()),
        ..UpdateTaskInput::default()
    }
}

/// The revision input a session publishes: it never names its own session.
fn revision(commit: &str) -> HandoffInput {
    HandoffInput::Revision {
        source_session_id: None,
        commit: commit.to_string(),
        comment: "ready for review".to_string(),
        review: None,
    }
}

/// Everything a refused publication must have left alone.
struct Snapshot {
    task: Task,
    events: Vec<TaskEvent>,
    handoffs: usize,
    links: usize,
    refs: Vec<(Uuid, String)>,
}

impl Fixture {
    fn service(&self) -> HandoffService {
        HandoffService::from_state(&self.app.state)
    }

    async fn snapshot(&self, task_id: Uuid) -> Snapshot {
        let repository = TaskRepository::new(&self.app.pool);

        Snapshot {
            task: repository
                .find_task(self.project.id, TaskRef::Id(task_id))
                .await
                .expect("the task reads")
                .expect("the task exists"),
            events: self.events().await,
            handoffs: repository
                .list_handoffs(self.project.id, task_id)
                .await
                .expect("the hand-offs read")
                .len(),
            links: repository
                .list_task_sessions(task_id)
                .await
                .expect("the links read")
                .len(),
            refs: self.retained_refs().await,
        }
    }

    /// Every task event of this project, oldest first.
    async fn events(&self) -> Vec<TaskEvent> {
        TaskRepository::new(&self.app.pool)
            .list_task_events_after(self.project.id, 0, 100)
            .await
            .expect("the events read")
            .into_iter()
            .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
            .collect()
    }

    /// Every retained hand-off ref in the project repository.
    async fn retained_refs(&self) -> Vec<(Uuid, String)> {
        refs::list_handoffs(&self.paths().project_repo(self.project.id))
            .await
            .expect("the hand-off refs list")
    }

    /// Move the project out of `ready`, the way a failed clone would.
    async fn set_project_status(&self, status: ProjectStatus) {
        let mut tx = self.app.pool.begin().await.expect("a transaction begins");
        ProjectRepository::new(&self.app.pool)
            .set_status(&mut tx, self.project.id, status, None)
            .await
            .expect("the status is set")
            .expect("the project exists");
        tx.commit().await.expect("the transaction commits");
    }
}

/// Assert that nothing moved between two snapshots of a refused publication.
fn unchanged(before: &Snapshot, after: &Snapshot) {
    assert_eq!(before.task.state_id, after.task.state_id, "the state moved");
    assert_eq!(
        before.task.lease_holder_session_id, after.task.lease_holder_session_id,
        "the lease moved",
    );
    assert_eq!(
        before.task.current_handoff_id, after.task.current_handoff_id,
        "the current hand-off moved",
    );
    assert_eq!(
        before.events.len(),
        after.events.len(),
        "a refused publication wrote task events",
    );
    assert_eq!(
        before.handoffs, after.handoffs,
        "a refused publication wrote a hand-off row",
    );
    assert_eq!(
        before.links, after.links,
        "a refused publication wrote a task_sessions row",
    );
    assert_eq!(
        before.refs, after.refs,
        "a refused publication left a retained ref behind",
    );
}

#[tokio::test]
async fn a_revision_by_the_holding_session_publishes_everything_at_once() {
    let fixture = Fixture::create("service-revision").await;
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;
    let task = fixture.claim(task.id, session_id).await;

    let published: TaskDto = fixture
        .service()
        .update_with_handoff(
            fixture.project.id,
            TaskRef::Id(task.id),
            move_to("review"),
            revision(&commit),
            HandoffCaller::Session { session_id },
        )
        .await
        .expect("the publication succeeds");

    // The task moved and the lease went with the move (`SPEC.md`, "Tasks").
    assert_eq!(published.state, "review");
    assert_eq!(published.lease_holder_session_id, None);
    assert_eq!(published.attempts, 0);

    // The record the response points at is the one that was written, and the
    // ref pinning its commit carries the same id (ADR 0018).
    let handoff = published
        .handoff
        .expect("the task carries its new hand-off");
    assert_eq!(handoff.commit, commit);
    assert_eq!(handoff.source_session_id, Some(session_id));
    assert_eq!(handoff.review_status, ReviewStatus::Unreviewed);
    assert_eq!(handoff.created_by_session_id, Some(session_id));
    assert_eq!(
        fixture.retained_refs().await,
        vec![(handoff.id, commit.clone())],
    );

    let repository = TaskRepository::new(&fixture.app.pool);
    let rows = repository
        .list_handoffs(fixture.project.id, task.id)
        .await
        .expect("the hand-offs read");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, handoff.id);

    // The comment the hand-off carried, authored by the publishing session.
    let comments = repository
        .list_comments(fixture.project.id, task.id)
        .await
        .expect("the comments read");
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].body, "ready for review");
    assert_eq!(comments[0].author_session_id, Some(session_id));
    assert_eq!(rows[0].comment_id, Some(comments[0].id));

    // The state event first, carrying the task with its new hand-off, then the
    // comment event (`SPEC.md`, "Code hand-offs and review").
    let kinds: Vec<TaskEventKind> = fixture
        .events()
        .await
        .iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        kinds,
        // The `claimed` the arranging claim wrote, then the publication's own
        // two, in the documented order.
        vec![
            TaskEventKind::Claimed,
            TaskEventKind::StateChanged,
            TaskEventKind::Commented,
        ],
    );

    // The code's origin is linked to the task.
    let links: Vec<Uuid> = repository
        .list_task_sessions(task.id)
        .await
        .expect("the links read")
        .into_iter()
        .map(|link| link.session_id)
        .collect();
    assert_eq!(links, vec![session_id]);
}

#[tokio::test]
async fn a_commit_that_is_not_the_session_tip_is_refused_with_nothing_written() {
    let fixture = Fixture::create("service-tip").await;
    let (session_id, first) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;
    let task = fixture.claim(task.id, session_id).await;

    // The agent commits again after naming `first`, so the branch tip has
    // moved on: "sync must produce that exact tip or return 409".
    let work = fixture.paths().session_work(session_id);
    std::fs::write(work.join("MORE.md"), "more\n").expect("the file is written");
    run_git(&work, &["add", "--", "MORE.md"]).await;
    run_git(&work, &["commit", "--quiet", "-m", "feat: more"]).await;

    let before = fixture.snapshot(task.id).await;
    let error = fixture
        .service()
        .update_with_handoff(
            fixture.project.id,
            TaskRef::Id(task.id),
            move_to("review"),
            revision(&first),
            HandoffCaller::Session { session_id },
        )
        .await
        .expect_err("the stale commit is refused");

    assert!(matches!(error, Error::Conflict(_)), "{error:?}");
    unchanged(&before, &fixture.snapshot(task.id).await);
    assert!(fixture.retained_refs().await.is_empty());
}

#[tokio::test]
async fn a_forward_of_a_handoff_that_is_not_current_is_refused() {
    let fixture = Fixture::create("service-forward").await;
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    fixture.sync(session_id).await;
    let task = fixture.task("review it", "review").await;
    let (task, _current) = fixture
        .current_handoff(
            &task,
            Some(session_id),
            &format!("session/{session_id}"),
            &commit,
        )
        .await;

    let before = fixture.snapshot(task.id).await;
    let error = fixture
        .service()
        .update_with_handoff(
            fixture.project.id,
            TaskRef::Id(task.id),
            move_to("ready"),
            HandoffInput::Forward {
                // A hand-off id that was never this task's current one: the
                // stale id a reviewer retries with after somebody else
                // published a newer revision.
                handoff_id: Uuid::new_v4(),
                comment: "changes please".to_string(),
                review: None,
            },
            fixture.user_caller(),
        )
        .await
        .expect_err("the stale forward is refused");

    assert!(matches!(error, Error::Conflict(_)), "{error:?}");
    unchanged(&before, &fixture.snapshot(task.id).await);
    // The one ref the seeded hand-off's commit is pinned under is the only
    // one: the refused forward pinned none of its own.
    assert_eq!(fixture.retained_refs().await.len(), before.refs.len());
}

#[tokio::test]
async fn a_task_that_moves_while_the_commit_is_being_pinned_is_a_recheck_conflict() {
    let fixture = Fixture::create("service-recheck").await;
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;
    let task = fixture.claim(task.id, session_id).await;
    let before = fixture.snapshot(task.id).await;

    // The interleaving is chosen rather than hoped for: another connection
    // holds the project row the tracker transaction is about to take — the
    // lock every mutation of this project queues on — and moves the task
    // inside that transaction. The publication below therefore prepares
    // against the old row, blocks at `TrackerMutation::begin`, and reads the
    // moved row once this commits.
    let mut holder = fixture
        .app
        .pool
        .begin()
        .await
        .expect("a transaction begins");
    sqlx::query("SELECT id FROM projects WHERE id = $1 FOR UPDATE")
        .bind(fixture.project.id)
        .execute(&mut *holder)
        .await
        .expect("the project row locks");
    sqlx::query("UPDATE tasks SET state_id = $1 WHERE id = $2")
        .bind(fixture.state("needs_human").await.id)
        .bind(task.id)
        .execute(&mut *holder)
        .await
        .expect("the task moves");

    let state = fixture.app.state.clone();
    let project_id = fixture.project.id;
    let published = tokio::spawn(async move {
        HandoffService::from_state(&state)
            .update_with_handoff(
                project_id,
                TaskRef::Id(task.id),
                move_to("review"),
                revision(&commit),
                HandoffCaller::Session { session_id },
            )
            .await
    });

    // Preparation is over exactly when the ref it pins appears, which is also
    // when the call is waiting for the project row this test is holding.
    tokio::time::timeout(RACE_TIMEOUT, async {
        while fixture.retained_refs().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the publication prepares its ref");

    holder.commit().await.expect("the racing move commits");

    let error = tokio::time::timeout(RACE_TIMEOUT, published)
        .await
        .expect("the publication answers")
        .expect("the task does not panic")
        .expect_err("the moved task is refused");

    match error {
        Error::Conflict(message) => assert_eq!(message, HANDOFF_RECHECK_FAILED),
        other => panic!("expected a recheck conflict, got {other:?}"),
    }

    // The ref the failed publication pinned is gone again, and the racing move
    // is the only thing that changed.
    assert!(fixture.retained_refs().await.is_empty());
    let after = fixture.snapshot(task.id).await;
    assert_eq!(after.events.len(), before.events.len());
    assert_eq!(after.handoffs, before.handoffs);
    assert_eq!(after.links, before.links);
    assert_eq!(
        after.task.lease_holder_session_id,
        before.task.lease_holder_session_id,
    );
}

#[tokio::test]
async fn a_project_that_is_not_ready_is_refused_before_any_git_work() {
    let fixture = Fixture::create("service-cloning").await;
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;
    let task = fixture.claim(task.id, session_id).await;
    fixture.set_project_status(ProjectStatus::Cloning).await;

    let before = fixture.snapshot(task.id).await;
    let error = fixture
        .service()
        .update_with_handoff(
            fixture.project.id,
            TaskRef::Id(task.id),
            move_to("review"),
            revision(&commit),
            HandoffCaller::Session { session_id },
        )
        .await
        .expect_err("an unready project is refused");

    match error {
        Error::Conflict(message) => assert_eq!(message, "project is not ready"),
        other => panic!("expected a conflict, got {other:?}"),
    }

    unchanged(&before, &fixture.snapshot(task.id).await);
    // Nothing was synced either: the session branch is still absent from the
    // project repository, which is what "before any git work" means.
    assert!(fixture.retained_refs().await.is_empty());
}

#[tokio::test]
async fn an_unknown_state_names_the_valid_states_and_publishes_nothing() {
    let fixture = Fixture::create("service-state").await;
    let (session_id, commit) = fixture.session_with_commit("feat: the work").await;
    let task = fixture.task("implement it", "ready").await;
    let task = fixture.claim(task.id, session_id).await;

    let before = fixture.snapshot(task.id).await;
    let error = fixture
        .service()
        .update_with_handoff(
            fixture.project.id,
            TaskRef::Id(task.id),
            move_to("nowhere"),
            revision(&commit),
            HandoffCaller::Session { session_id },
        )
        .await
        .expect_err("an unknown state is refused");

    match error {
        Error::BadRequest(message) => {
            assert!(message.contains("unknown state \"nowhere\""), "{message}");
            assert!(message.contains("review"), "{message}");
        }
        other => panic!("expected a bad request, got {other:?}"),
    }

    unchanged(&before, &fixture.snapshot(task.id).await);
}

#[tokio::test]
async fn a_task_this_project_does_not_have_is_not_found() {
    let fixture = Fixture::create("service-unknown-task").await;
    let session_id = fixture.seed_session().await;

    let error = fixture
        .service()
        .update_with_handoff(
            fixture.project.id,
            TaskRef::Id(Uuid::new_v4()),
            move_to("review"),
            revision(ABSENT_COMMIT),
            HandoffCaller::Session { session_id },
        )
        .await
        .expect_err("an unknown task is refused");

    assert!(matches!(error, Error::NotFound), "{error:?}");
    assert!(fixture.retained_refs().await.is_empty());
}

#[tokio::test]
async fn two_sessions_publishing_one_task_produce_exactly_one_hand_off() {
    let fixture = Fixture::create("service-race").await;
    let (holder, holder_commit) = fixture.session_with_commit("feat: the work").await;
    let (other, other_commit) = fixture.session_with_commit("feat: the other work").await;
    let task = fixture.task("implement it", "ready").await;
    // Only one of the two sessions holds the lease, which is what a second
    // agent publishing on the same task runs into.
    let task = fixture.claim(task.id, holder).await;

    let one = {
        let state = fixture.app.state.clone();
        let project_id = fixture.project.id;
        let task_id = task.id;
        tokio::spawn(async move {
            HandoffService::from_state(&state)
                .update_with_handoff(
                    project_id,
                    TaskRef::Id(task_id),
                    move_to("review"),
                    revision(&holder_commit),
                    HandoffCaller::Session { session_id: holder },
                )
                .await
        })
    };
    let two = {
        let state = fixture.app.state.clone();
        let project_id = fixture.project.id;
        let task_id = task.id;
        tokio::spawn(async move {
            HandoffService::from_state(&state)
                .update_with_handoff(
                    project_id,
                    TaskRef::Id(task_id),
                    move_to("review"),
                    revision(&other_commit),
                    HandoffCaller::Session { session_id: other },
                )
                .await
        })
    };

    let (first, second) = tokio::time::timeout(RACE_TIMEOUT, async { tokio::join!(one, two) })
        .await
        .expect("both publications answer");
    let results = [
        first.expect("the first task does not panic"),
        second.expect("the second task does not panic"),
    ];

    let published: Vec<&TaskDto> = results
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .collect();
    assert_eq!(published.len(), 1, "exactly one publication succeeds");

    // One record, one ref, and the refused caller left nothing behind.
    let rows = TaskRepository::new(&fixture.app.pool)
        .list_handoffs(fixture.project.id, task.id)
        .await
        .expect("the hand-offs read");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        published[0].handoff.as_ref().expect("it carries one").id,
        rows[0].id,
    );
    assert_eq!(
        fixture.retained_refs().await,
        vec![(rows[0].id, rows[0].commit.clone())],
    );
}
