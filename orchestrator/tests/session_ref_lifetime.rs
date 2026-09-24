//! The session ref at the end of a session: kept only when the session made
//! commits beyond the commit it was cloned from that no integration head or
//! hand-off ref already holds (`ARCHITECTURE.md`, "Git model", Ref ownership
//! and Fetch-back; "Session lifecycle"; `SPEC.md`, "Sessions", "Git";
//! ADR 0050).
//!
//! A planner, or a reviewer that forwards a hand-off without committing, ends
//! with `session/<sid>` exactly at its base; an implementer that handed off
//! its last commit, or a session merged into `main`, ends with a tip another
//! retained ref contains. The end-of-session fetch-back then writes no
//! `refs/sessions/<sid>` and deletes one an earlier sync left. What this suite
//! asserts, over the real end path and against a real project repository:
//!
//! - a session that ends without committing leaves no ref, and one that
//!   committed keeps it;
//! - a session whose tip a hand-off ref or an integration head contains leaves
//!   no ref, one with commits past its last hand-off keeps it, and one whose
//!   tip only an upstream-tracking ref or a tag contains keeps it;
//! - an ended session whose ref was dropped because a hand-off holds its work
//!   still shows that work in its diff;
//! - a ref an earlier sync wrote at the base goes when the session ends;
//! - the base is the commit recorded at launch (`sessions.base_commit`), not
//!   the branch the session was launched from, which may have moved;
//! - a session with no recorded base commit — launched before the column —
//!   is judged by containment alone;
//! - a ref kept at the end is judged again by a later fetch-back of the
//!   ended session, and by nothing else;
//! - every reader of a session ref answers an ended session without one as
//!   documented: an empty diff, no entry in either listing, a 400 for a launch
//!   from it, a revision hand-off that pins the base commit, an explicit sync
//!   that does not bring the ref back — and a retried session, live again,
//!   gets it back on its next fetch-back.
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"): the fixture is
//! `common::handoffs::Fixture`, a real upstream and project repository with
//! real session work clones. The engine is `MockEngine`.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use axum::http::StatusCode;
use common::handoffs::Fixture;
use mars_orchestrator::git::testutil::{run_git, test_identity};
use mars_orchestrator::git::{
    GitActor, GitRef, GitService, create_work_clone, refs, resolve_base, session_branch,
};
use mars_orchestrator::models::{Branch, BranchKind, Diff, SessionBranch, SessionState};
use mars_orchestrator::repositories::{SessionRepository, Transition};
use mars_orchestrator::tracker::TaskDto;
use serde_json::{Value, json};
use uuid::Uuid;

/// A session launched the way the launcher launches one, as far as git and
/// the row are concerned.
struct Launched {
    id: Uuid,
    /// The commit the work clone was created at.
    base: String,
}

impl Fixture {
    /// Seed a session, clone its work tree from `main`, record the base
    /// commit when `record` says so — the launcher's own write — and move it
    /// to `parked`, the rest state of a conversational session.
    async fn launch_recording(&self, record: bool) -> Launched {
        self.launch_from(None, record).await
    }

    /// As [`Fixture::launch_recording`], from `base_ref` (the project default
    /// when `None`).
    async fn launch_from(&self, base_ref: Option<&str>, record: bool) -> Launched {
        let id = self.seed_session().await;
        let paths = self.paths();

        let base = {
            let guard = self.guard().await;
            let base = resolve_base(&guard, &paths, base_ref, "main")
                .await
                .expect("the base resolves");
            create_work_clone(&guard, &paths, id, &base, &test_identity())
                .await
                .expect("the work clone is created");
            base.commit
        };

        if record {
            let mut tx = self.app.pool.begin().await.expect("a transaction begins");
            SessionRepository::new(&self.app.pool)
                .set_base_commit(&mut tx, id, &base)
                .await
                .expect("the base commit is recorded");
            tx.commit().await.expect("the transaction commits");
        }

        self.move_session(id, SessionState::Creating, SessionState::Running)
            .await;
        self.move_session(id, SessionState::Running, SessionState::Parked)
            .await;

        Launched { id, base }
    }

    async fn launch(&self) -> Launched {
        self.launch_recording(true).await
    }

    async fn move_session(&self, id: Uuid, from: SessionState, to: SessionState) {
        let mut change = Transition::new(from, to, "for the test");
        if to == SessionState::Failed {
            change = change.with_error("the test failed it");
        }

        let mut tx = self.app.pool.begin().await.expect("a transaction begins");
        SessionRepository::new(&self.app.pool)
            .transition(&mut tx, id, &change)
            .await
            .expect("the session transitions");
        tx.commit().await.expect("the transaction commits");
    }

    /// `POST /api/sessions/{id}/end`, expecting the session back.
    async fn end(&self, id: Uuid) -> Value {
        let response = self
            .app
            .post_as(&self.signed_in(), &format!("/api/sessions/{id}/end"))
            .await;
        response.assert_status(StatusCode::OK);

        response.json::<Value>()
    }

    /// `POST /api/sessions/{id}/sync`, answering its body.
    async fn explicit_sync(&self, id: Uuid) -> Value {
        let response = self
            .app
            .post_as(&self.signed_in(), &format!("/api/sessions/{id}/sync"))
            .await;
        response.assert_status(StatusCode::OK);

        response.json::<Value>()
    }

    /// Where `refs/sessions/<id>` points, or `None` when there is no such ref.
    async fn session_ref(&self, id: Uuid) -> Option<String> {
        let repo = self.paths().project_repo(self.project.id);
        let listed = refs::list_sessions(&repo)
            .await
            .expect("the session refs list");
        if !listed.contains(&id) {
            return None;
        }

        Some(
            refs::resolve(&repo, &GitRef::Session(id))
                .await
                .expect("a listed session ref resolves")
                .commit,
        )
    }

    /// Point `refs/heads/main` at `commit`, as a merge into it would.
    async fn move_main(&self, commit: &str) {
        let _guard = self.guard().await;
        refs::update(
            &self.paths().project_repo(self.project.id),
            "refs/heads/main",
            commit,
            None,
        )
        .await
        .expect("main moves");
    }

    /// Point `full_name` — an upstream-tracking ref or a tag — at `commit`,
    /// as a fetch would.
    async fn point(&self, full_name: &str, commit: &str) {
        let _guard = self.guard().await;
        refs::update(
            &self.paths().project_repo(self.project.id),
            full_name,
            commit,
            None,
        )
        .await
        .expect("the ref is written");
    }

    /// Publish a revision hand-off of `commit` from `source` over REST, moving
    /// `task` to `review`, and answer the task.
    async fn publish(&self, task: Uuid, source: Uuid, commit: &str) -> TaskDto {
        let response = self
            .app
            .put_as(
                &self.signed_in(),
                &format!("/api/projects/{}/tasks/{task}", self.project.id),
            )
            .json(&json!({
                "state": "review",
                "handoff": {
                    "kind": "revision",
                    "source_session_id": source,
                    "commit": commit,
                    "comment": "the work so far",
                },
            }))
            .await;
        response.assert_status(StatusCode::OK);

        response.json::<TaskDto>()
    }

    /// `GET /api/projects/{pid}/git/diff?head=<id>`.
    async fn diff(&self, id: Uuid) -> Diff {
        let response = self
            .app
            .get_as(
                &self.signed_in(),
                &format!("/api/projects/{}/git/diff?head={id}", self.project.id),
            )
            .await;
        response.assert_status(StatusCode::OK);

        response.json::<Diff>()
    }

    /// The `git` events of a session's transcript, oldest first.
    async fn git_events(&self, id: Uuid) -> Vec<Value> {
        SessionRepository::new(&self.app.pool)
            .list_events(id, None, 100)
            .await
            .expect("the events are read")
            .0
            .into_iter()
            .filter(|row| row.kind == "git")
            .map(|row| row.payload)
            .collect()
    }
}

#[tokio::test]
async fn a_session_that_ends_without_committing_keeps_no_ref() {
    let fixture = Fixture::create("ends-empty").await;
    let session = fixture.launch().await;

    let ended = fixture.end(session.id).await;

    assert_eq!(ended["state"], "done");
    assert_eq!(
        fixture.session_ref(session.id).await,
        None,
        "a session with nothing beyond its base keeps no ref"
    );
    assert!(
        fixture.paths().session_work(session.id).is_dir(),
        "the work clone stays until the session is deleted"
    );

    // The end still records its sync, naming the commit the branch is at.
    let events = fixture.git_events(session.id).await;
    let [event] = events.as_slice() else {
        panic!("expected one git event, got {events:?}");
    };
    assert_eq!(event["op"], "sync");
    assert_eq!(event["ok"], true);
    assert_eq!(event["detail"]["commit"], session.base.as_str());
}

#[tokio::test]
async fn a_session_that_committed_keeps_its_ref_at_its_tip() {
    let fixture = Fixture::create("ends-with-work").await;
    let session = fixture.launch().await;
    let tip = fixture
        .commit_in_work_clone(session.id, "WORK.md", "the agent's work")
        .await;

    fixture.end(session.id).await;

    assert_eq!(fixture.session_ref(session.id).await, Some(tip));
}

#[tokio::test]
async fn a_ref_synced_mid_session_goes_when_the_session_ends_with_nothing_past_its_base() {
    let fixture = Fixture::create("synced-then-empty").await;
    let session = fixture.launch().await;

    // A live session keeps its ref whatever it holds.
    let synced = fixture.explicit_sync(session.id).await;
    assert_eq!(synced["commit"], session.base.as_str());
    assert_eq!(
        fixture.session_ref(session.id).await,
        Some(session.base.clone())
    );

    fixture.end(session.id).await;

    assert_eq!(fixture.session_ref(session.id).await, None);
}

#[tokio::test]
async fn a_session_that_rewound_its_branch_to_the_base_loses_the_ref_it_had_synced() {
    let fixture = Fixture::create("rewound").await;
    let session = fixture.launch().await;
    fixture
        .commit_in_work_clone(session.id, "DRAFT.md", "a draft")
        .await;
    fixture.explicit_sync(session.id).await;

    // The agent threw its work away before the end.
    let work = fixture.paths().session_work(session.id);
    run_git(&work, &["reset", "--quiet", "--hard", &session.base]).await;

    fixture.end(session.id).await;

    assert_eq!(fixture.session_ref(session.id).await, None);
}

#[tokio::test]
async fn the_base_is_the_commit_recorded_at_launch_not_the_branch_it_came_from() {
    let fixture = Fixture::create("moved-base").await;
    let session = fixture.launch().await;

    // `main` moves on after the launch: another session's work is merged.
    let other = fixture.launch().await;
    let merged = fixture
        .commit_in_work_clone(other.id, "OTHER.md", "someone else's work")
        .await;
    fixture.sync(other.id).await;
    fixture.move_main(&merged).await;

    fixture.end(session.id).await;

    // Judged against `main` as it is now, the untouched branch would look
    // like it held something `main` does not; it is at its recorded base.
    assert_eq!(fixture.session_ref(session.id).await, None);
}

#[tokio::test]
async fn a_session_with_no_recorded_base_commit_is_judged_by_containment_alone() {
    let fixture = Fixture::create("no-base-commit").await;
    let at_base = fixture.launch_recording(false).await;
    let worked = fixture.launch_recording(false).await;
    let tip = fixture
        .commit_in_work_clone(worked.id, "WORK.md", "the agent's work")
        .await;

    fixture.end(at_base.id).await;
    fixture.end(worked.id).await;

    assert_eq!(
        fixture.session_ref(at_base.id).await,
        None,
        "with no base commit to compare, `main` still holds the untouched tip"
    );
    assert_eq!(fixture.session_ref(worked.id).await, Some(tip));
}

#[tokio::test]
async fn an_implementer_that_handed_off_its_tip_keeps_no_ref() {
    let fixture = Fixture::create("handed-off").await;
    let session = fixture.launch().await;
    let tip = fixture
        .commit_in_work_clone(session.id, "GREETING.md", "hello")
        .await;
    let task = fixture.task("Add greeting", "ready").await;
    let published = fixture.publish(task.id, session.id, &tip).await;

    // Publishing syncs a live session, which keeps its ref whatever it holds.
    assert_eq!(fixture.session_ref(session.id).await, Some(tip.clone()));

    fixture.end(session.id).await;

    assert_eq!(
        fixture.session_ref(session.id).await,
        None,
        "the hand-off ref holds every commit the session made"
    );
    let handoff = published.handoff.expect("the task has a current hand-off");
    assert_eq!(
        fixture.handoff_refs().await,
        vec![(handoff.id, tip.clone())]
    );

    // Its Changes still show its work: the diff reads the work clone's tip,
    // which the hand-off keeps in the project repository.
    let diff = fixture.diff(session.id).await;
    assert_eq!(diff.merge_base, session.base);
    let paths: Vec<&str> = diff.files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, vec!["GREETING.md"]);
    assert!(diff.patch.contains("+hello"), "{}", diff.patch);
    assert_eq!(
        fixture.session_ref(session.id).await,
        None,
        "reading the diff does not bring the ref back"
    );
}

#[tokio::test]
async fn an_implementer_that_committed_past_its_last_hand_off_keeps_its_ref() {
    let fixture = Fixture::create("past-hand-off").await;
    let session = fixture.launch().await;
    let handed = fixture
        .commit_in_work_clone(session.id, "GREETING.md", "hello")
        .await;
    let task = fixture.task("Add greeting", "ready").await;
    fixture.publish(task.id, session.id, &handed).await;
    let tip = fixture
        .commit_in_work_clone(session.id, "FAREWELL.md", "goodbye")
        .await;

    fixture.end(session.id).await;

    assert_eq!(
        fixture.session_ref(session.id).await,
        Some(tip),
        "a commit after the last hand-off is work only the session holds"
    );
}

#[tokio::test]
async fn a_reviewer_launched_from_a_hand_off_that_committed_nothing_keeps_no_ref() {
    let fixture = Fixture::create("reviewer").await;
    let implementer = fixture.launch().await;
    let tip = fixture
        .commit_in_work_clone(implementer.id, "GREETING.md", "hello")
        .await;
    let task = fixture.task("Add greeting", "ready").await;
    fixture.publish(task.id, implementer.id, &tip).await;

    // Launched from the hand-off's commit, as a launch for the task is, and
    // synced once while it reviewed.
    let reviewer = fixture.launch_from(Some(&tip), true).await;
    assert_eq!(reviewer.base, tip);
    fixture.explicit_sync(reviewer.id).await;
    assert_eq!(fixture.session_ref(reviewer.id).await, Some(tip.clone()));

    fixture.end(reviewer.id).await;

    assert_eq!(fixture.session_ref(reviewer.id).await, None);
}

#[tokio::test]
async fn a_session_merged_into_main_before_it_ends_keeps_no_ref() {
    let fixture = Fixture::create("merged").await;
    let session = fixture.launch().await;
    fixture
        .commit_in_work_clone(session.id, "GREETING.md", "hello")
        .await;

    let response = fixture
        .app
        .post_as(
            &fixture.signed_in(),
            &format!("/api/projects/{}/git/merge", fixture.project.id),
        )
        .json(&json!({ "source": session.id, "target": "main" }))
        .await;
    response.assert_status(StatusCode::OK);
    assert!(
        fixture.session_ref(session.id).await.is_some(),
        "the merge synced the live session, which keeps its ref"
    );

    fixture.end(session.id).await;

    assert_eq!(fixture.session_ref(session.id).await, None);
}

#[tokio::test]
async fn a_tip_held_only_by_an_upstream_tracking_ref_or_a_tag_keeps_its_ref() {
    let fixture = Fixture::create("upstream-only").await;
    let tracked = fixture.launch().await;
    let tracked_tip = fixture
        .commit_in_work_clone(tracked.id, "PUSHED.md", "pushed upstream")
        .await;
    let tagged = fixture.launch().await;
    let tagged_tip = fixture
        .commit_in_work_clone(tagged.id, "TAGGED.md", "tagged")
        .await;

    // In the project repository, and named there only by refs Mars does not
    // own: a fetch can prune either (ADR 0017).
    fixture.explicit_sync(tracked.id).await;
    fixture.explicit_sync(tagged.id).await;
    fixture
        .point("refs/remotes/origin/main", &tracked_tip)
        .await;
    fixture.point("refs/tags/v-tagged", &tagged_tip).await;

    fixture.end(tracked.id).await;
    fixture.end(tagged.id).await;

    assert_eq!(fixture.session_ref(tracked.id).await, Some(tracked_tip));
    assert_eq!(fixture.session_ref(tagged.id).await, Some(tagged_tip));
}

#[tokio::test]
async fn a_ref_kept_at_the_end_is_judged_again_only_by_a_later_fetch_back() {
    let fixture = Fixture::create("judged-again").await;
    let session = fixture.launch().await;
    let tip = fixture
        .commit_in_work_clone(session.id, "WORK.md", "the agent's work")
        .await;
    fixture.end(session.id).await;
    assert_eq!(fixture.session_ref(session.id).await, Some(tip.clone()));

    // The work lands on `main` after the end. Nothing chases it: the ref
    // stays until something fetches the ended session back again.
    fixture.move_main(&tip).await;
    assert_eq!(fixture.session_ref(session.id).await, Some(tip.clone()));

    let synced = fixture.explicit_sync(session.id).await;

    assert_eq!(synced["commit"], tip.as_str());
    assert_eq!(fixture.session_ref(session.id).await, None);
}

#[tokio::test]
async fn an_ended_session_without_a_ref_has_an_empty_diff_against_its_base() {
    let fixture = Fixture::create("diff-empty").await;
    let session = fixture.launch().await;
    fixture.end(session.id).await;

    let response = fixture
        .app
        .get_as(
            &fixture.signed_in(),
            &format!(
                "/api/projects/{}/git/diff?head={}",
                fixture.project.id, session.id
            ),
        )
        .await;
    response.assert_status(StatusCode::OK);
    let diff = response.json::<Diff>();

    assert_eq!(diff.head, session.id.to_string());
    assert_eq!(diff.merge_base, session.base);
    assert!(diff.files.is_empty(), "no changes: {:?}", diff.files);
    assert!(diff.patch.is_empty());
    assert_eq!(
        fixture.session_ref(session.id).await,
        None,
        "reading the diff does not bring the ref back"
    );
}

#[tokio::test]
async fn an_ended_session_without_a_ref_is_in_neither_branch_listing() {
    let fixture = Fixture::create("listings").await;
    let empty = fixture.launch().await;
    let worked = fixture.launch().await;
    fixture
        .commit_in_work_clone(worked.id, "KEPT.md", "kept")
        .await;
    fixture.end(empty.id).await;
    fixture.end(worked.id).await;
    let user = fixture.signed_in();

    let response = fixture
        .app
        .get_as(
            &user,
            &format!("/api/projects/{}/git/session-branches", fixture.project.id),
        )
        .await;
    response.assert_status(StatusCode::OK);
    let listed: Vec<Uuid> = response
        .json::<Vec<SessionBranch>>()
        .into_iter()
        .map(|branch| branch.session_id)
        .collect();
    assert_eq!(listed, vec![worked.id]);

    let response = fixture
        .app
        .get_as(
            &user,
            &format!("/api/projects/{}/branches", fixture.project.id),
        )
        .await;
    response.assert_status(StatusCode::OK);
    let sessions: Vec<Option<Uuid>> = response
        .json::<Vec<Branch>>()
        .into_iter()
        .filter(|branch| branch.kind == BranchKind::Session)
        .map(|branch| branch.session_id)
        .collect();
    assert_eq!(sessions, vec![Some(worked.id)]);

    // The session itself still names the branch its work clone holds.
    let response = fixture
        .app
        .get_as(&user, &format!("/api/sessions/{}", empty.id))
        .await;
    response.assert_status(StatusCode::OK);
    assert_eq!(
        response.json::<Value>()["branch"],
        session_branch(empty.id).as_str(),
        "`Session.branch` is the work clone's branch, not the project ref"
    );
}

#[tokio::test]
async fn a_launch_from_an_ended_session_without_a_ref_is_a_bad_request() {
    let fixture = Fixture::create("launch-from").await;
    let session = fixture.launch().await;
    fixture.end(session.id).await;

    let response = fixture
        .app
        .post_as(
            &fixture.signed_in(),
            &format!("/api/projects/{}/sessions", fixture.project.id),
        )
        .json(&json!({
            "profile_id": fixture.profile_id,
            "base_ref": session.id.to_string(),
        }))
        .await;

    response.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json::<Value>()["error"],
        mars_orchestrator::session::UNRESOLVED_BASE
    );
}

#[tokio::test]
async fn an_explicit_sync_of_an_ended_session_does_not_bring_the_ref_back() {
    let fixture = Fixture::create("sync-after-end").await;
    let session = fixture.launch().await;
    fixture.end(session.id).await;

    let synced = fixture.explicit_sync(session.id).await;

    assert_eq!(synced["commit"], session.base.as_str());
    assert_eq!(fixture.session_ref(session.id).await, None);
}

#[tokio::test]
async fn a_revision_hand_off_from_an_ended_session_without_a_ref_pins_its_base_commit() {
    let fixture = Fixture::create("handoff-after-end").await;
    let session = fixture.launch().await;
    fixture.end(session.id).await;
    let task = fixture.task("Review the plan", "ready").await;

    let response = fixture
        .app
        .put_as(
            &fixture.signed_in(),
            &format!("/api/projects/{}/tasks/{}", fixture.project.id, task.id),
        )
        .json(&json!({
            "state": "review",
            "handoff": {
                "kind": "revision",
                "source_session_id": session.id,
                "commit": session.base,
                "comment": "the plan, with no code of its own",
            },
        }))
        .await;
    response.assert_status(StatusCode::OK);

    let handoff = response
        .json::<TaskDto>()
        .handoff
        .expect("the task has a current hand-off");
    assert_eq!(handoff.commit, session.base);
    assert_eq!(
        fixture.handoff_refs().await,
        vec![(handoff.id, session.base.clone())],
        "the hand-off's own ref holds the commit"
    );
    assert_eq!(
        fixture.session_ref(session.id).await,
        None,
        "publishing syncs the ended session under its end rule"
    );
}

#[tokio::test]
async fn a_retried_session_gets_its_ref_back_on_its_next_fetch_back() {
    let fixture = Fixture::create("retried").await;
    let session = fixture.launch().await;
    fixture
        .move_session(session.id, SessionState::Parked, SessionState::Failed)
        .await;

    // A failed session is ended: its fetch-back keeps no ref at the base.
    let service = GitService::from_state(&fixture.app.state);
    service
        .sync_session(fixture.project.id, session.id, &GitActor::System)
        .await
        .expect("the sync succeeds");
    assert_eq!(fixture.session_ref(session.id).await, None);

    let response = fixture
        .app
        .post_as(
            &fixture.signed_in(),
            &format!("/api/sessions/{}/retry", session.id),
        )
        .await;
    response.assert_status(StatusCode::OK);
    assert_eq!(response.json::<Value>()["state"], "parked");

    fixture.explicit_sync(session.id).await;

    assert_eq!(fixture.session_ref(session.id).await, Some(session.base));
}
