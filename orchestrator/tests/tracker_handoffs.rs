//! `tracker::handoffs::prepare`: the git half of publishing a code hand-off
//! (`ARCHITECTURE.md`, "Task tracker" → "Code hand-offs"; `SPEC.md`, "Code
//! hand-offs and review"; `docs/data-model.md`, `task_handoffs`; ADR 0018).
//!
//! What is asserted here is everything that has to be true *before* the
//! tracker transaction opens, and that nothing in the database moves while it
//! is decided:
//!
//! - a revision syncs the source session branch and pins the requested commit
//!   at `refs/handoffs/<new id>`, with the row id and the ref name the same;
//! - a commit that is not the fetched tip is a conflict, and no ref is
//!   written — "sync must produce that exact tip or return 409";
//! - a session caller must hold the lease; a user caller need not;
//! - a source session of another project is refused before any git work;
//! - a forward copies the current record's session, branch and commit and
//!   pins a second ref at the same commit without syncing, which is why it
//!   still works once the work clone is gone;
//! - a forward naming anything but the task's current hand-off is a conflict;
//! - `discard_prepared` removes the ref the failed publication pinned, twice
//!   over.
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"): every case
//! builds a real upstream, a real project repository and a real session work
//! clone under the `TestApp`'s own `DATA_DIR`.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use common::handoffs::Fixture;
use mars_orchestrator::git::refs;
use mars_orchestrator::git::testutil::run_git;
use mars_orchestrator::models::{
    HandoffCaller, ReviewDecision, ReviewStatus, TaskError, ValidatedHandoff,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::tracker::handoffs::{ReviewCarry, discard_prepared, prepare};
use uuid::Uuid;

/// An obviously fake but well-formed object id, for the "not in the
/// repository" case.
const ABSENT_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

#[tokio::test]
async fn a_revision_syncs_the_session_and_pins_the_commit() {
    let fixture = Fixture::create("revision").await;
    let (session_id, commit) = fixture.session_with_commit("docs: notes").await;
    let task = fixture.task("implement it", "ready").await;
    let task = fixture.claim(task.id, session_id).await;

    let guard = fixture.guard().await;
    let prepared = prepare(
        &fixture.app.state,
        &guard,
        fixture.project.id,
        &task,
        &fixture.state("ready").await,
        &fixture.state("review").await,
        &ValidatedHandoff::Revision {
            source_session_id: session_id,
            commit: commit.clone(),
            comment: "please review".into(),
        },
        &HandoffCaller::Session { session_id },
    )
    .await
    .expect("the hand-off prepares");

    assert_eq!(prepared.task_id, task.id);
    assert_eq!(prepared.source_session_id, Some(session_id));
    assert_eq!(prepared.source_branch, format!("session/{session_id}"));
    assert_eq!(prepared.commit, commit);
    assert_eq!(prepared.comment, "please review");
    assert_eq!(prepared.previous_handoff_id, None);
    assert_eq!(prepared.review, ReviewCarry::Fresh);
    assert_eq!(prepared.ref_name, format!("refs/handoffs/{}", prepared.id));

    // The ref name and the row id are the same value, which is what the
    // orphan-cleanup job compares.
    assert_eq!(fixture.handoff_refs().await, vec![(prepared.id, commit)]);
}

#[tokio::test]
async fn a_commit_that_is_not_the_synced_tip_is_a_conflict_and_pins_nothing() {
    let fixture = Fixture::create("mismatch").await;
    let (session_id, first) = fixture.session_with_commit("docs: first").await;

    // The agent commits again after telling the orchestrator about `first`.
    let work = fixture.paths().session_work(session_id);
    std::fs::write(work.join("MORE.md"), "more\n").expect("the file is written");
    run_git(&work, &["add", "--", "MORE.md"]).await;
    run_git(&work, &["commit", "--quiet", "-m", "docs: second"]).await;
    let second = run_git(&work, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_string();

    let task = fixture.task("implement it", "ready").await;
    let task = fixture.claim(task.id, session_id).await;

    let guard = fixture.guard().await;
    let error = prepare(
        &fixture.app.state,
        &guard,
        fixture.project.id,
        &task,
        &fixture.state("ready").await,
        &fixture.state("review").await,
        &ValidatedHandoff::Revision {
            source_session_id: session_id,
            commit: first.clone(),
            comment: "please review".into(),
        },
        &HandoffCaller::Session { session_id },
    )
    .await
    .expect_err("the stale commit is refused");

    match error {
        Error::Conflict(message) => {
            assert!(message.contains(&second), "{message}");
            assert!(message.contains(&first), "{message}");
        }
        other => panic!("expected a conflict, got {other:?}"),
    }

    assert!(fixture.handoff_refs().await.is_empty());
}

#[tokio::test]
async fn a_session_that_does_not_hold_the_lease_is_a_conflict() {
    let fixture = Fixture::create("no-lease").await;
    let (session_id, commit) = fixture.session_with_commit("docs: notes").await;
    let other = fixture.seed_session().await;
    let task = fixture.task("implement it", "ready").await;
    let task = fixture.claim(task.id, other).await;

    let guard = fixture.guard().await;
    let error = prepare(
        &fixture.app.state,
        &guard,
        fixture.project.id,
        &task,
        &fixture.state("ready").await,
        &fixture.state("review").await,
        &ValidatedHandoff::Revision {
            source_session_id: session_id,
            commit,
            comment: "please review".into(),
        },
        &HandoffCaller::Session { session_id },
    )
    .await
    .expect_err("a session that holds nothing is refused");

    assert!(
        matches!(&error, Error::Conflict(message) if message == "task is not held by the calling session"),
        "{error:?}"
    );
    assert!(fixture.handoff_refs().await.is_empty());
}

#[tokio::test]
async fn a_user_may_publish_a_task_another_session_holds() {
    let fixture = Fixture::create("user-caller").await;
    let (session_id, commit) = fixture.session_with_commit("docs: notes").await;
    let task = fixture.task("implement it", "ready").await;
    let task = fixture.claim(task.id, session_id).await;

    let guard = fixture.guard().await;
    let prepared = prepare(
        &fixture.app.state,
        &guard,
        fixture.project.id,
        &task,
        &fixture.state("ready").await,
        &fixture.state("review").await,
        &ValidatedHandoff::Revision {
            source_session_id: session_id,
            commit: commit.clone(),
            comment: "please review".into(),
        },
        &fixture.user_caller(),
    )
    .await
    .expect("a user is not lease-bound");

    assert_eq!(prepared.commit, commit);
}

#[tokio::test]
async fn a_source_session_of_another_project_is_a_bad_request() {
    let fixture = Fixture::create("foreign-session").await;
    let other = Fixture::create("other-project").await;
    let foreign = other.seed_session().await;

    let task = fixture.task("implement it", "ready").await;

    let guard = fixture.guard().await;
    let error = prepare(
        &fixture.app.state,
        &guard,
        fixture.project.id,
        &task,
        &fixture.state("ready").await,
        &fixture.state("review").await,
        &ValidatedHandoff::Revision {
            source_session_id: foreign,
            commit: ABSENT_COMMIT.into(),
            comment: "please review".into(),
        },
        &fixture.user_caller(),
    )
    .await
    .expect_err("another project's session is refused");

    assert!(
        matches!(&error, Error::BadRequest(message)
            if message == "source_session_id must name a session of this project"),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_session_with_no_synced_branch_is_a_bad_request() {
    let fixture = Fixture::create("creating-session").await;
    let session_id = fixture.seed_session().await;
    let task = fixture.task("implement it", "ready").await;

    let guard = fixture.guard().await;
    let error = prepare(
        &fixture.app.state,
        &guard,
        fixture.project.id,
        &task,
        &fixture.state("ready").await,
        &fixture.state("review").await,
        &ValidatedHandoff::Revision {
            source_session_id: session_id,
            commit: ABSENT_COMMIT.into(),
            comment: "please review".into(),
        },
        &fixture.user_caller(),
    )
    .await
    .expect_err("a session with no branch is refused");

    assert!(
        matches!(&error, Error::BadRequest(message) if message == "session has no synced branch yet"),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_hand_off_that_does_not_move_the_task_is_refused() {
    let fixture = Fixture::create("same-state").await;
    let (session_id, commit) = fixture.session_with_commit("docs: notes").await;
    let task = fixture.task("implement it", "ready").await;

    let guard = fixture.guard().await;
    let error = prepare(
        &fixture.app.state,
        &guard,
        fixture.project.id,
        &task,
        &fixture.state("ready").await,
        &fixture.state("ready").await,
        &ValidatedHandoff::Revision {
            source_session_id: session_id,
            commit,
            comment: "please review".into(),
        },
        &fixture.user_caller(),
    )
    .await
    .expect_err("a hand-off needs a different target state");

    assert!(
        matches!(&error, Error::Task(TaskError::HandoffRequiresStateChange)),
        "{error:?}"
    );
    assert!(fixture.handoff_refs().await.is_empty());
}

#[tokio::test]
async fn a_forward_pins_a_second_ref_at_the_same_commit_without_syncing() {
    let fixture = Fixture::create("forward").await;
    let (session_id, commit) = fixture.session_with_commit("docs: notes").await;
    let task = fixture.task("implement it", "review").await;
    let branch = format!("session/{session_id}");
    let (task, handoff_id) = fixture
        .current_handoff(&task, Some(session_id), &branch, &commit)
        .await;

    // The revision's own sync and ref, so the commit is in the mirror.
    fixture.sync(session_id).await;
    refs::retain_handoff(
        &fixture.paths().project_repo(fixture.project.id),
        handoff_id,
        &commit,
    )
    .await
    .expect("the first ref is written");

    // The reviewer's session is gone, work clone and all: a forward must not
    // need it.
    std::fs::remove_dir_all(fixture.paths().session_work(session_id))
        .expect("the work clone is removed");

    let guard = fixture.guard().await;
    let prepared = prepare(
        &fixture.app.state,
        &guard,
        fixture.project.id,
        &task,
        &fixture.state("review").await,
        &fixture.state("merge").await,
        &ValidatedHandoff::Forward {
            handoff_id,
            comment: "looks good".into(),
            review: Some(ReviewDecision::Approved),
        },
        &fixture.user_caller(),
    )
    .await
    .expect("the forward prepares");

    assert_ne!(prepared.id, handoff_id);
    assert_eq!(prepared.source_session_id, Some(session_id));
    assert_eq!(prepared.source_branch, branch);
    assert_eq!(prepared.commit, commit);
    assert_eq!(prepared.previous_handoff_id, Some(handoff_id));
    assert_eq!(
        prepared.review,
        ReviewCarry::Decision(ReviewDecision::Approved)
    );

    let mut pinned = fixture.handoff_refs().await;
    pinned.sort();
    let mut expected = vec![(handoff_id, commit.clone()), (prepared.id, commit)];
    expected.sort();
    assert_eq!(pinned, expected);
}

#[tokio::test]
async fn a_forward_without_a_decision_carries_the_previous_review() {
    let fixture = Fixture::create("carry").await;
    let (session_id, commit) = fixture.session_with_commit("docs: notes").await;
    let task = fixture.task("implement it", "review").await;
    let branch = format!("session/{session_id}");
    let (task, handoff_id) = fixture
        .current_handoff(&task, Some(session_id), &branch, &commit)
        .await;
    fixture.sync(session_id).await;
    refs::retain_handoff(
        &fixture.paths().project_repo(fixture.project.id),
        handoff_id,
        &commit,
    )
    .await
    .expect("the first ref is written");

    let guard = fixture.guard().await;
    let prepared = prepare(
        &fixture.app.state,
        &guard,
        fixture.project.id,
        &task,
        &fixture.state("review").await,
        &fixture.state("ready").await,
        &ValidatedHandoff::Forward {
            handoff_id,
            comment: "carry on".into(),
            review: None,
        },
        &fixture.user_caller(),
    )
    .await
    .expect("the forward prepares");

    match prepared.review {
        ReviewCarry::CarriedFrom(previous) => {
            assert_eq!(previous.id, handoff_id);
            assert_eq!(previous.review_status, ReviewStatus::Unreviewed);
        }
        other => panic!("expected the previous record, got {other:?}"),
    }
}

#[tokio::test]
async fn a_forward_of_anything_but_the_current_hand_off_is_a_conflict() {
    let fixture = Fixture::create("stale-forward").await;
    let (session_id, commit) = fixture.session_with_commit("docs: notes").await;
    let task = fixture.task("implement it", "review").await;
    let branch = format!("session/{session_id}");
    let (task, _current) = fixture
        .current_handoff(&task, Some(session_id), &branch, &commit)
        .await;

    let guard = fixture.guard().await;
    let error = prepare(
        &fixture.app.state,
        &guard,
        fixture.project.id,
        &task,
        &fixture.state("review").await,
        &fixture.state("merge").await,
        &ValidatedHandoff::Forward {
            handoff_id: Uuid::new_v4(),
            comment: "looks good".into(),
            review: None,
        },
        &fixture.user_caller(),
    )
    .await
    .expect_err("a stale hand-off id is refused");

    assert!(
        matches!(&error, Error::Conflict(message)
            if message == "handoff_id is not the task's current hand-off"),
        "{error:?}"
    );
    assert!(fixture.handoff_refs().await.is_empty());
}

#[tokio::test]
async fn a_forward_on_a_task_with_no_hand_off_is_a_conflict() {
    let fixture = Fixture::create("no-current").await;
    let task = fixture.task("implement it", "review").await;

    let guard = fixture.guard().await;
    let error = prepare(
        &fixture.app.state,
        &guard,
        fixture.project.id,
        &task,
        &fixture.state("review").await,
        &fixture.state("merge").await,
        &ValidatedHandoff::Forward {
            handoff_id: Uuid::new_v4(),
            comment: "looks good".into(),
            review: None,
        },
        &fixture.user_caller(),
    )
    .await
    .expect_err("a task with no hand-off has nothing to forward");

    assert!(
        matches!(&error, Error::Conflict(message)
            if message == "handoff_id is not the task's current hand-off"),
        "{error:?}"
    );
}

#[tokio::test]
async fn discarding_a_prepared_hand_off_removes_its_ref_and_repeats_cleanly() {
    let fixture = Fixture::create("discard").await;
    let (session_id, commit) = fixture.session_with_commit("docs: notes").await;
    let task = fixture.task("implement it", "ready").await;
    let task = fixture.claim(task.id, session_id).await;

    let guard = fixture.guard().await;
    let prepared = prepare(
        &fixture.app.state,
        &guard,
        fixture.project.id,
        &task,
        &fixture.state("ready").await,
        &fixture.state("review").await,
        &ValidatedHandoff::Revision {
            source_session_id: session_id,
            commit,
            comment: "please review".into(),
        },
        &HandoffCaller::Session { session_id },
    )
    .await
    .expect("the hand-off prepares");

    assert_eq!(fixture.handoff_refs().await.len(), 1);

    discard_prepared(&fixture.app.state, &guard, &prepared).await;
    assert!(fixture.handoff_refs().await.is_empty());

    // Idempotent: the orphan-cleanup job may have got there first.
    discard_prepared(&fixture.app.state, &guard, &prepared).await;
    assert!(fixture.handoff_refs().await.is_empty());
}
