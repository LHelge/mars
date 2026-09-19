//! Publishing a code hand-off: the git half, under the project git lock.
//!
//! `SPEC.md`, "Code hand-offs and review" gives `PUT
//! /projects/{pid}/tasks/{id}` and the MCP `update` tool a `handoff` field: a
//! revision publishes a commit under an internal immutable ref and moves the
//! task, a forward re-uses the current hand-off and optionally records a
//! review decision.
//!
//! "Git and Postgres cannot share a transaction" (`ARCHITECTURE.md`, "Task
//! tracker"), so publication is two halves with one lock order between them,
//! and the caller — not this module — owns both locks:
//!
//! 1. take the project **git lock**;
//! 2. [`prepare`]: validate the task's state, holder and current hand-off with
//!    plain pool reads, sync the source session branch, require the fetched
//!    tip to equal the requested commit, and pin it at `refs/handoffs/<new
//!    id>`;
//! 3. `TrackerMutation::begin` — the **project row** lock;
//! 4. the task row `FOR UPDATE`, recheck, write the comment, the
//!    `task_handoffs` row, the state change and the events;
//! 5. commit;
//! 6. release the git lock.
//!
//! The git lock is held *through* the database commit so that a task merge's
//! approval check, taken under the same lock, cannot interleave with a
//! publication. It is never taken from inside the database lock: a transaction
//! holding the project row must never wait for the git lock
//! (`ARCHITECTURE.md`, "Git model" → Serialization).
//!
//! [`prepare`] therefore opens no transaction and takes no lock of its own; it
//! is handed the [`ProjectGitGuard`] as proof that the caller holds one. When
//! the database half then fails, the caller calls [`discard_prepared`] to drop
//! the ref it pinned. A crash in between leaves an unreferenced
//! `refs/handoffs/<id>`, which is expected and which the orphan-cleanup job
//! removes (ADR 0018).
//!
//! **What is still missing**: [`publish`] — the database half that consumes a
//! [`PreparedHandoff`] — is the stub it has been since the tracker epic, and
//! refuses every hand-off with [`HANDOFFS_UNAVAILABLE`].

use uuid::Uuid;

use crate::git::{DataPaths, GitError, GitRef, GitService, ProjectGitGuard, refs};
use crate::models::{
    HandoffCaller, HandoffInput, ReviewDecision, Task, TaskHandoff, TaskState, ValidatedHandoff,
};
use crate::prelude::*;
use crate::repositories::{SessionRepository, TaskRepository};
use crate::tracker::TrackerMutation;

/// What a `handoff` is answered with until the database half lands.
pub const HANDOFFS_UNAVAILABLE: &str = "code hand-offs are not available yet";

/// The review status a prepared hand-off will be written with.
///
/// The three cases `SPEC.md`, "Code hand-offs and review" distinguishes, kept
/// apart rather than flattened to a [`ReviewStatus`](crate::models::ReviewStatus)
/// because the carried case also carries the *reviewer* and the review
/// timestamp of the record it came from: "no decision means the previous
/// review status and reviewer are carried forward".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewCarry {
    /// A revision: unreviewed, always. "A new revision always resets review
    /// status to `unreviewed`; old approvals stay in history."
    Fresh,
    /// A forward with an explicit decision on the commit being passed on.
    Decision(ReviewDecision),
    /// A forward with no decision: the previous record's status, reviewer and
    /// timestamp come along unchanged. Boxed because a `TaskHandoff` is an
    /// order of magnitude larger than the other two variants and every
    /// [`PreparedHandoff`] would otherwise carry the difference.
    CarriedFrom(Box<TaskHandoff>),
}

/// Everything the database half needs, with the git half already done.
///
/// The commit is pinned at [`PreparedHandoff::ref_name`] by the time this
/// exists, so the tracker transaction never touches git: it inserts the
/// comment, the `task_handoffs` row with this `id`, and the state change.
/// Because the id is generated in [`prepare`], the ref name and the row id
/// match, which is what lets the orphan-cleanup job compare `refs/handoffs/*`
/// with `task_handoffs` (`docs/data-model.md`, `task_handoffs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedHandoff {
    /// The new hand-off's id, and the name of its retained ref.
    pub id: Uuid,
    /// The task being handed off.
    pub task_id: Uuid,
    /// The session the code comes from, or `None` when a forward's original
    /// session has since been deleted.
    pub source_session_id: Option<Uuid>,
    /// A snapshot of the source session's branch name (`session/<id>`).
    pub source_branch: String,
    /// The full object id the ref pins.
    pub commit: String,
    /// The trimmed, non-empty comment written with the hand-off.
    pub comment: String,
    /// The task's current hand-off at preparation time, for the recheck the
    /// database half makes under the task row lock.
    pub previous_handoff_id: Option<Uuid>,
    /// What the new record's review fields will say.
    pub review: ReviewCarry,
    /// `refs/handoffs/<id>`, as it was written.
    pub ref_name: String,
}

/// Validate a hand-off and pin its commit, with the project git lock held.
///
/// Step 2 of the order in the module documentation. Nothing in the database
/// changes here and no transaction is opened: every read is a plain pool read,
/// deliberately, because the authoritative check happens again under the task
/// row lock in the database half. What this establishes is only what the
/// database cannot: that the commit exists, that it *is* the session tip, and
/// that it is retained before the task moves (`ARCHITECTURE.md`, "Task
/// tracker" → "Code hand-offs").
///
/// Pre-validation, in order:
///
/// - a [`HandoffCaller::Session`] must hold the task's lease, otherwise
///   [`Error::Conflict`] — an agent may only hand off work it is holding. A
///   [`HandoffCaller::User`] is not lease-bound;
/// - `target_state` must differ from `current_state`
///   ([`TaskError::HandoffRequiresStateChange`](crate::models::TaskError)).
///   The routes check this before any git work; it is re-checked here so that
///   the rule holds for every caller.
///
/// Then, per kind:
///
/// - **revision**: the source session must be one of this project's
///   ([`Error::BadRequest`] otherwise, which is also the answer when it has
///   been deleted, because project membership can no longer be verified); its
///   branch is synced silently; the resulting `refs/sessions/<id>` tip must
///   equal the requested commit, or [`Error::Conflict`] naming the tip, with
///   no ref created. A session that has neither a work tree nor a session ref
///   — one still `creating` — is [`Error::BadRequest`];
/// - **forward**: `handoff_id` must equal `tasks.current_handoff_id`
///   ([`Error::Conflict`] otherwise, including when the task has no current
///   hand-off), and the source session, branch and commit are copied from that
///   record. Nothing is synced and no session is looked up, so a forward works
///   after the original session has ended or been deleted.
///
/// Both kinds finish at [`refs::retain_handoff`], which verifies that the
/// commit is present in the project repository *and* is a commit before
/// writing the ref; a commit that is neither is [`Error::Conflict`].
#[allow(clippy::too_many_arguments)]
pub async fn prepare(
    state: &AppState,
    guard: &ProjectGitGuard,
    project_id: Uuid,
    task: &Task,
    current_state: &TaskState,
    target_state: &TaskState,
    validated: &ValidatedHandoff,
    caller: &HandoffCaller,
) -> Result<PreparedHandoff> {
    if let HandoffCaller::Session { session_id } = caller
        && task.lease_holder_session_id != Some(*session_id)
    {
        return Err(Error::Conflict(
            "task is not held by the calling session".into(),
        ));
    }

    HandoffInput::require_state_change(Some(&target_state.name), &current_state.name)?;

    let paths = DataPaths::from_config(&state.config);
    let mirror = paths.project_repo(project_id);

    let (source_session_id, source_branch, commit, review) = match validated {
        ValidatedHandoff::Revision {
            source_session_id,
            commit,
            ..
        } => {
            let session = SessionRepository::new(&state.pool)
                .find_in_project(project_id, *source_session_id)
                .await?
                .ok_or_else(|| {
                    Error::BadRequest(
                        "source_session_id must name a session of this project".into(),
                    )
                })?;

            // Silent: publication is not a git operation the agent asked for,
            // so it records no `git` event of its own (`ARCHITECTURE.md`, "MCP
            // design", Side effects). A session whose work directory is gone
            // but whose `refs/sessions/<id>` is still there skips the fetch,
            // which is what lets work from an ended session be published.
            GitService::from_state(state)
                .sync_session_silent(guard, *source_session_id)
                .await
                .map_err(|err| match err {
                    Error::Git(GitError::UnknownRef(_)) => {
                        Error::BadRequest("session has no synced branch yet".into())
                    }
                    other => other,
                })?;

            let tip = refs::resolve(&mirror, &GitRef::Session(*source_session_id))
                .await?
                .commit;
            if tip != *commit {
                return Err(Error::Conflict(format!(
                    "session branch tip {tip} does not match commit {commit}"
                )));
            }

            (
                Some(session.id),
                session.branch,
                commit.clone(),
                ReviewCarry::Fresh,
            )
        }
        ValidatedHandoff::Forward {
            handoff_id, review, ..
        } => {
            if task.current_handoff_id != Some(*handoff_id) {
                return Err(Error::Conflict(
                    "handoff_id is not the task's current hand-off".into(),
                ));
            }

            let current = TaskRepository::new(&state.pool)
                .find_handoff(project_id, *handoff_id)
                .await?
                .ok_or(Error::NotFound)?;

            let source_session_id = current.source_session_id;
            let source_branch = current.source_branch.clone();
            let commit = current.commit.clone();

            let carry = match review {
                Some(decision) => ReviewCarry::Decision(*decision),
                None => ReviewCarry::CarriedFrom(Box::new(current)),
            };

            (source_session_id, source_branch, commit, carry)
        }
    };

    let id = Uuid::new_v4();
    refs::retain_handoff(&mirror, id, &commit)
        .await
        .map_err(|err| {
            debug!(
                task_id = %task.id,
                handoff_id = %id,
                commit = %commit,
                error = %err,
                "the hand-off commit could not be retained",
            );
            Error::Conflict("commit is not present in the project repository".into())
        })?;

    debug!(
        task_id = %task.id,
        handoff_id = %id,
        commit = %commit,
        "hand-off prepared",
    );

    Ok(PreparedHandoff {
        id,
        task_id: task.id,
        source_session_id,
        source_branch,
        commit,
        comment: comment_of(validated).to_string(),
        previous_handoff_id: task.current_handoff_id,
        review,
        ref_name: refs::handoff_ref(id),
    })
}

/// Drop the ref [`prepare`] pinned, because the database half did not commit.
///
/// Called with the same git lock still held, so nothing can have started
/// reading the ref in between. [`refs::remove_handoff`] is idempotent, and a
/// failure here is not the caller's to report: the publication has already
/// failed for its own reason, and the orphan-cleanup job is the backstop for
/// exactly this ref (`docs/data-model.md`, `task_handoffs`). So this returns
/// nothing and only warns.
pub async fn discard_prepared(
    state: &AppState,
    guard: &ProjectGitGuard,
    prepared: &PreparedHandoff,
) {
    let project_id = guard.project_id();
    let mirror = DataPaths::from_config(&state.config).project_repo(project_id);

    if let Err(err) = refs::remove_handoff(&mirror, prepared.id).await {
        warn!(
            project_id = %project_id,
            handoff_id = %prepared.id,
            error = %err,
            "the prepared hand-off ref could not be removed; orphan cleanup will take it",
        );
    }
}

/// The trimmed comment either kind of validated input carries.
fn comment_of(validated: &ValidatedHandoff) -> &str {
    match validated {
        ValidatedHandoff::Revision { comment, .. } | ValidatedHandoff::Forward { comment, .. } => {
            comment
        }
    }
}

/// Publish a hand-off for this task inside the caller's mutation.
///
/// The stub: every call is [`Error::BadRequest`] with
/// [`HANDOFFS_UNAVAILABLE`], and nothing is written — the caller's mutation
/// rolls back with the refusal, so a rejected hand-off leaves neither a task
/// change nor an event (ADR 0021).
///
/// The signature is deliberately the shape the real one needs: the open
/// mutation, the task as it is under the lock, and the caller's comment. What
/// replaces it takes the [`PreparedHandoff`] beside these rather than instead
/// of them.
#[allow(unused_variables)]
pub async fn publish(m: &mut TrackerMutation<'_>, task: &Task, comment: &str) -> Result<()> {
    Err(Error::BadRequest(HANDOFFS_UNAVAILABLE.into()))
}
