//! Publishing a code hand-off: the git half, the database half, and the one
//! entry point that composes them under the project git lock.
//!
//! `SPEC.md`, "Code hand-offs and review" gives `PUT
//! /projects/{pid}/tasks/{id}` and the MCP `update` tool a `handoff` field: a
//! revision publishes a commit under an internal immutable ref and moves the
//! task, a forward re-uses the current hand-off and optionally records a
//! review decision.
//!
//! "Git and Postgres cannot share a transaction" (`ARCHITECTURE.md`, "Task
//! tracker"), so publication is two halves with one lock order between them,
//! which [`HandoffService::update_with_handoff`] takes in this order:
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
//! Step 4 is [`publish_in_transaction`], which does the whole database half in
//! one function: the recheck, the comment, the record, the pointer, the move
//! and the events.
//!
//! [`HandoffService::update_with_handoff`] is the *composition* of the two
//! halves and the only code path that publishes or forwards a hand-off: the
//! REST `PUT /projects/{pid}/tasks/{id}` handler and the MCP `update` tool
//! both call it when their body carries a `handoff`, with the
//! [`HandoffCaller`] each of them authenticated.
//!
//! **Concurrency.** Two callers publishing on the same project serialise
//! twice: first on the project git lock, then — inside it — on the project
//! row. The second one therefore does its preparation *after* the first one's
//! transaction has committed, re-reads the task under both locks, and either
//! succeeds against the new state or is answered [`Error::Conflict`] by
//! preparation (a stale `handoff_id`, a lease it no longer holds, a session
//! tip that has moved) or by the recheck
//! ([`HANDOFF_RECHECK_FAILED`]). A partial change is never published: the
//! database half is one transaction, and the ref the git half pinned is
//! discarded when that transaction does not commit (`SPEC.md`, "Tasks";
//! ADR 0021).

use chrono::Utc;
use uuid::Uuid;

use crate::events::TaskActor;
use crate::git::service::NOT_READY;
use crate::git::{DataPaths, GitError, GitRef, GitService, ProjectGitGuard, refs};
use crate::models::{
    HandoffCaller, HandoffInput, NewTaskComment, NewTaskHandoff, ProjectStatus, ReviewDecision,
    ReviewStatus, Task, TaskError, TaskHandoff, TaskRef, TaskState, ValidatedHandoff,
};
use crate::prelude::*;
use crate::repositories::tasks::StateFields;
use crate::repositories::{ProjectRepository, SessionRepository, TaskRepository, unique_violation};
use crate::tracker::state::{resolve_state, resolve_state_in_pool};
use crate::tracker::tasks::{UpdateTaskInput, update_task};
use crate::tracker::{CommentDto, TaskDto, TrackerMutation, commit_and_notify};

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
    /// The state the task was in at preparation time, for the recheck.
    pub state_id: Uuid,
    /// The lease holder at preparation time, for the recheck.
    pub lease_holder_session_id: Option<Uuid>,
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
///   [`HandoffService::update_with_handoff`] checks this before taking the git
///   lock; it is re-checked here so that the rule holds for every caller.
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
                // The pointer matched a moment ago, so a missing row is a
                // deletion racing this read: the caller's id is stale, which
                // is the documented 409, not a 404 for the task it addressed.
                .ok_or_else(|| {
                    Error::Conflict("handoff_id is not the task's current hand-off".into())
                })?;

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
        state_id: task.state_id,
        lease_holder_session_id: task.lease_holder_session_id,
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

/// What a task that moved between [`prepare`] and publication is told (409).
///
/// One message for all four rechecks — the state, the holder, the caller's
/// hold and the previous current hand-off — because they mean the same thing
/// to the caller: the task is no longer the task the ref was pinned for, and
/// the answer is to re-read it (`ARCHITECTURE.md`, "Task tracker" → "Code
/// hand-offs").
pub const HANDOFF_RECHECK_FAILED: &str =
    "task changed during hand-off publication; re-read it and retry";

/// Publish a [`PreparedHandoff`]: the database half, inside `begin_mutation`.
///
/// Step 4 of the order in the module documentation, and the whole of it. `m`
/// holds the project row lock and carries the actor matching `caller`; `task`
/// is the row as it is under that lock, read with
/// `TaskRepository::find_task_for_update`, exactly as
/// [`update_task`](crate::tracker::update_task) and
/// [`change_state`](crate::tracker::state::change_state) require. Everything
/// below commits together with the caller's mutation or not at all, and any
/// error here leaves the caller to roll back and [`discard_prepared`] the ref
/// (ADR 0021, 0028).
///
/// **The recheck comes first.** [`prepare`] validated against plain pool reads
/// and then spent time in git, so the authoritative reading of the three
/// values it decided on is taken here, under the lock: the task's `state_id`,
/// its `lease_holder_session_id` — which for a [`HandoffCaller::Session`] must
/// still be the caller — and its `current_handoff_id`, which must be the
/// `previous_handoff_id` the preparation saw. Any difference is
/// [`Error::Conflict`] with [`HANDOFF_RECHECK_FAILED`], before a row is
/// written.
///
/// Then, in this order, because each step needs the one before it:
///
/// 1. the **comment** row, authored by `caller` and never `system`: a hand-off
///    always carries its message to the next worker, and `task_handoffs` owes
///    it a `comment_id` (`docs/data-model.md`, `task_handoffs`);
/// 2. the **`task_handoffs`** row with the prepared id, so that the row and
///    the retained `refs/handoffs/<id>` match. Its review fields are
///    [`ReviewCarry`]'s: `Fresh` is `unreviewed` with no reviewer and no time,
///    `Decision` is the verdict with `caller` as the reviewer and the
///    transaction's own timestamp, and `CarriedFrom` copies the previous
///    record's four review fields verbatim — including a reviewer pair that
///    deletion has nulled (`NewTaskHandoff::carry_review`);
/// 3. **`tasks.current_handoff_id`**, set *before* the move, because
///    `state_changed` carries "the full task after the change" and the board
///    reads the new hand-off out of that payload (`SPEC.md`, "Code hand-offs
///    and review");
/// 4. the **move**, through [`update_task`](crate::tracker::update_task) — not
///    a copy of it — so that a hand-off may carry ordinary field updates in the
///    same request and so that the lease release, the `attempts` reset,
///    `closed_at`, the dependants' `blocked` recompute and the parent closure
///    are the same rules every other caller gets. `update.state` names the
///    target, which must differ from the state the task is in
///    ([`TaskError::HandoffRequiresStateChange`](crate::models::TaskError));
/// 5. the **`commented`** event, after the state events, which is the order
///    `SPEC.md`, "Code hand-offs and review" gives: the state event carries the
///    task including its new hand-off, and the accompanying `commented` event
///    carries the comment;
/// 6. the **`task_sessions`** links: the source session, when the record still
///    names one, and the calling session. `TrackerMutation::touch`
///    deduplicates the pair a revision by its own holder writes twice.
///
/// **No `released` event.** The lease clears as part of the state change, not
/// as a release, so there is nothing separate to announce (`SPEC.md`,
/// "TaskEvent").
///
/// **A move into the human state is a `state_changed`**, not an `escalated`:
/// this goes through `update_task`, which uses
/// [`StateEventKind::StateChanged`](crate::tracker::StateEventKind), and only
/// the escalation paths — which also owe an email — say `escalated`
/// (`tracker::state`). Publishing code into `needs_human` is a hand-off to a
/// person, not an agent running out of attempts.
pub async fn publish_in_transaction(
    m: &mut TrackerMutation<'_>,
    task: &Task,
    prepared: &PreparedHandoff,
    caller: &HandoffCaller,
    update: UpdateTaskInput,
) -> Result<TaskDto> {
    let project_id = m.project_id();

    if task.id != prepared.task_id {
        error!(
            task_id = %task.id,
            prepared_task_id = %prepared.task_id,
            "a hand-off was prepared for a different task than the one being published",
        );
        return Err(Error::Internal("hand-off task mismatch".into()));
    }

    // (0) The recheck, under the lock, before anything is written.
    let holder_still_the_caller = match caller {
        HandoffCaller::Session { session_id } => task.lease_holder_session_id == Some(*session_id),
        HandoffCaller::User { .. } => true,
    };
    if task.state_id != prepared.state_id
        || task.lease_holder_session_id != prepared.lease_holder_session_id
        || task.current_handoff_id != prepared.previous_handoff_id
        || !holder_still_the_caller
    {
        debug!(
            project_id = %project_id,
            task_id = %task.id,
            handoff_id = %prepared.id,
            "the task changed between hand-off preparation and publication",
        );
        return Err(Error::Conflict(HANDOFF_RECHECK_FAILED.into()));
    }

    // The target state, so that a hand-off that does not move the task is
    // refused here as well as at the transports (`SPEC.md`).
    let target = match update.state.as_deref() {
        Some(name) => resolve_state(m, name).await?,
        None => return Err(TaskError::HandoffRequiresStateChange.into()),
    };
    if target.id == task.state_id {
        return Err(TaskError::HandoffRequiresStateChange.into());
    }

    let repository = TaskRepository::new(m.pool());

    // (1) The comment the hand-off is published with.
    let new_comment = match caller {
        HandoffCaller::User { user_id } => {
            NewTaskComment::from_user(task.id, *user_id, &prepared.comment)
        }
        HandoffCaller::Session { session_id } => {
            NewTaskComment::from_session(task.id, *session_id, &prepared.comment)
        }
    };
    let comment = CommentDto::from(
        repository
            .insert_comment(m.conn(), project_id, &new_comment)
            .await?,
    );

    // (2) The record itself, with the id the ref was pinned under.
    let mut new_handoff = NewTaskHandoff::new(
        task.id,
        prepared.source_branch.clone(),
        prepared.commit.clone(),
        comment.id,
    );
    new_handoff.id = prepared.id;
    new_handoff.source_session_id = prepared.source_session_id;
    match caller {
        HandoffCaller::User { user_id } => new_handoff.created_by_user_id = Some(*user_id),
        HandoffCaller::Session { session_id } => {
            new_handoff.created_by_session_id = Some(*session_id);
        }
    }
    match &prepared.review {
        // A new revision is unreviewed, which is what `new` already built.
        ReviewCarry::Fresh => {}
        ReviewCarry::Decision(decision) => {
            new_handoff.review_status = ReviewStatus::from(*decision);
            new_handoff.reviewed_at = Some(Utc::now());
            match caller {
                HandoffCaller::User { user_id } => {
                    new_handoff.reviewed_by_user_id = Some(*user_id);
                }
                HandoffCaller::Session { session_id } => {
                    new_handoff.reviewed_by_session_id = Some(*session_id);
                }
            }
        }
        ReviewCarry::CarriedFrom(previous) => new_handoff.carry_review(previous),
    }

    let handoff = repository
        .insert_handoff(m.conn(), project_id, &new_handoff)
        .await
        .map_err(|err| reused_handoff_id(err, prepared.id))?;

    // (3) The pointer, before the move: `state_changed` carries the task as it
    // is after the change, and the new hand-off is part of that.
    repository
        .set_task_state_fields(
            m.conn(),
            project_id,
            task.id,
            &StateFields {
                current_handoff_id: Some(Some(handoff.id)),
                ..StateFields::default()
            },
        )
        .await?;

    // (4) The move, with whatever ordinary field changes came with it.
    let outcome = update_task(m, task, update).await?;

    // (5) The comment event, after the state events it accompanies.
    m.emit_comment(&outcome.task, &comment)?;

    // (6) The links: the code's origin and the caller, deduplicated.
    if let Some(source_session_id) = prepared.source_session_id {
        m.touch(task.id, source_session_id);
    }
    if let HandoffCaller::Session { session_id } = caller {
        m.touch(task.id, *session_id);
    }

    info!(
        project_id = %project_id,
        task_id = %task.id,
        handoff_id = %handoff.id,
        review_status = %handoff.review_status,
        state = %target.name,
        "hand-off published",
    );

    Ok(outcome.task)
}

/// Publishing a code hand-off, both halves, in the one documented order.
///
/// Cheap to build for the reason [`GitService`] gives — the state is a handful
/// of `Arc`s and a pool handle — so a route or a tool builds one per request
/// with [`HandoffService::from_state`] rather than [`AppState`] carrying
/// another field.
#[derive(Clone)]
pub struct HandoffService {
    state: AppState,
}

impl std::fmt::Debug for HandoffService {
    /// The state holds key material and connection details, so none of it is
    /// rendered here.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandoffService").finish_non_exhaustive()
    }
}

impl HandoffService {
    /// The service over an [`AppState`]'s pool, paths, git locks and
    /// credentials, exactly as [`GitService::from_state`] takes them.
    pub fn from_state(state: &AppState) -> Self {
        Self {
            state: state.clone(),
        }
    }

    /// Update a task *with* a hand-off: the whole documented order, once.
    ///
    /// The only code path that publishes or forwards a hand-off. `task` is
    /// already parsed, because REST takes it from a path segment and MCP from
    /// a tool argument; `update` is the ordinary update the same request
    /// carries, whose `state` names the target the hand-off moves the task to.
    ///
    /// In order (`ARCHITECTURE.md`, "Task tracker" → "Code hand-offs"):
    ///
    /// 1. the task — [`Error::NotFound`] when this project has no such task;
    /// 2. its current state and the target `update.state` names —
    ///    [`Error::BadRequest`] naming the valid states for an unknown one;
    /// 3. [`HandoffInput::require_state_change`] and
    ///    [`HandoffInput::validate`], the rules that need neither lock;
    /// 4. the project must be `ready` — [`Error::Conflict`] otherwise, before
    ///    any git work, because a project that is still cloning holds the git
    ///    lock for the length of that clone;
    /// 5. the project **git lock**;
    /// 6. [`prepare`]: sync, tip check, `refs/handoffs/<id>`;
    /// 7. [`TrackerMutation::begin`]: the project row lock;
    /// 8. the task again, `FOR UPDATE`, and [`publish_in_transaction`];
    /// 9. the commit, with the escalation mail it may owe;
    /// 10. the git lock is released — the guard lives across the whole
    ///     transaction, and the transaction never waits for it (ADR 0021);
    /// 11. the task, as it now is.
    ///
    /// Steps 1 to 4 take no lock and have no side effect, so a request refused
    /// there leaves no ref, no event and no row (ADR 0030). **Any** failure of
    /// steps 7 to 9 — a recheck conflict, a deleted task, a serialisation
    /// error, a pool timeout, a connection lost at commit time — discards the
    /// prepared ref first and then returns its *own* error, never the outcome
    /// of the cleanup: the publication failed for its own reason, and the
    /// orphan-cleanup job is the backstop for a ref that could not be dropped.
    #[instrument(skip_all, fields(project_id = %project_id))]
    pub async fn update_with_handoff(
        &self,
        project_id: Uuid,
        task: TaskRef,
        update: UpdateTaskInput,
        handoff: HandoffInput,
        caller: HandoffCaller,
    ) -> Result<TaskDto> {
        let state = &self.state;
        let repository = TaskRepository::new(&state.pool);

        // (1) The task, before anything else: an unknown one is a 404 whatever
        // else the body got wrong.
        let existing = repository
            .find_task(project_id, task)
            .await?
            .ok_or(Error::NotFound)?;

        // (2) The states, by id and by name, both off the pool: nothing is
        // being changed yet.
        let current_state = repository
            .find_state(project_id, existing.state_id)
            .await?
            .ok_or_else(|| {
                // Unreachable by construction: `tasks.state_id` references
                // `task_states` and a state is never deleted out from under a
                // task (`docs/data-model.md`, `task_states`).
                error!(
                    project_id = %project_id,
                    task_id = %existing.id,
                    "the task's state row is missing",
                );
                Error::Internal("task state is missing".into())
            })?;
        let target_name = update
            .state
            .as_deref()
            .ok_or(TaskError::HandoffRequiresStateChange)?;
        let target_state = resolve_state_in_pool(&state.pool, project_id, target_name).await?;

        // (3) The two input rules: the hand-off has to move the task, and the
        // input has to match the caller.
        HandoffInput::require_state_change(Some(&target_state.name), &current_state.name)?;
        let validated = handoff.validate(&caller)?;

        // (4) A project with no usable repository, before the lock.
        let project = ProjectRepository::new(&state.pool)
            .find(project_id)
            .await?
            .ok_or(Error::NotFound)?;
        if project.status != ProjectStatus::Ready {
            return Err(Error::Conflict(NOT_READY.to_string()));
        }

        // (5) The git lock, held through step 10.
        let guard = state.git_locks.lock(project_id).await;

        // (6) Sync and pin. A failure here has pinned nothing to discard.
        let prepared = prepare(
            state,
            &guard,
            project_id,
            &existing,
            &current_state,
            &target_state,
            &validated,
            &caller,
        )
        .await?;

        // (7) to (9), with the ref discarded on every way out but success.
        match self.publish(project_id, &prepared, &caller, update).await {
            Ok(published) => {
                info!(
                    project_id = %project_id,
                    task_id = %prepared.task_id,
                    handoff_id = %prepared.id,
                    kind = handoff_kind(&validated),
                    "hand-off published with the task update",
                );
                // (10) The guard is dropped here, after the commit.
                Ok(published)
            }
            Err(err) => {
                discard_prepared(state, &guard, &prepared).await;
                Err(err)
            }
        }
    }

    /// Steps 7 to 9: the tracker transaction around [`publish_in_transaction`].
    ///
    /// Split out so that the caller has exactly one `Result` to attach the ref
    /// cleanup to, and so that the commit — which can fail on its own, with the
    /// rows already written but unconfirmed — is inside that same `Result`.
    async fn publish(
        &self,
        project_id: Uuid,
        prepared: &PreparedHandoff,
        caller: &HandoffCaller,
        update: UpdateTaskInput,
    ) -> Result<TaskDto> {
        let state = &self.state;

        let mut mutation = TrackerMutation::begin(&state.pool, project_id, actor(caller)).await?;

        let published = async {
            // The task under the project row lock, which is the row every rule
            // is decided against. Gone since step 1 is a 404, and the prepared
            // ref is discarded by the caller.
            let task = TaskRepository::new(mutation.pool())
                .find_task_for_update(mutation.conn(), project_id, TaskRef::Id(prepared.task_id))
                .await?
                .ok_or(Error::NotFound)?;

            publish_in_transaction(&mut mutation, &task, prepared, caller, update).await
        }
        .await;

        match published {
            Ok(published) => {
                commit_and_notify(mutation, state).await?;
                Ok(published)
            }
            Err(err) => {
                // Rolled back explicitly rather than by dropping the mutation,
                // so the project row is free again before this returns: the
                // caller behind it is waiting on that row, not on a `Drop`
                // (ADR 0021, 0030 — nothing was written either way). Its own
                // failure never replaces the refusal being reported: dropping
                // the transaction rolls it back regardless.
                if let Err(rollback) = mutation.no_change().await {
                    warn!(
                        project_id = %project_id,
                        error = %rollback,
                        "a refused hand-off's transaction could not be rolled back",
                    );
                }
                Err(err)
            }
        }
    }
}

/// The tracker actor a hand-off caller acts as.
fn actor(caller: &HandoffCaller) -> TaskActor {
    match caller {
        HandoffCaller::User { user_id } => TaskActor::User { user_id: *user_id },
        HandoffCaller::Session { session_id } => TaskActor::Session {
            session_id: *session_id,
        },
    }
}

/// `"revision"` or `"forward"`, for the one success log line.
///
/// The comment is deliberately not a field of it: a hand-off comment is user
/// and agent content, which the orchestrator stores and displays but does not
/// copy into its own logs (`CLAUDE.md`, "Backend conventions").
fn handoff_kind(validated: &ValidatedHandoff) -> &'static str {
    match validated {
        ValidatedHandoff::Revision { .. } => "revision",
        ValidatedHandoff::Forward { .. } => "forward",
    }
}

/// The one insert failure that is ours rather than the caller's.
///
/// The id comes from [`prepare`], which generates it, so a primary key
/// collision means an id was reused — a bug, not something a caller can
/// provoke or retry — and it is answered generically with the detail in the
/// log (`CLAUDE.md`, "Backend conventions").
fn reused_handoff_id(error: Error, handoff_id: Uuid) -> Error {
    if let Error::Database(ref db_error) = error
        && let Some("task_handoffs_pkey") = unique_violation(db_error)
    {
        error!(handoff_id = %handoff_id, "a prepared hand-off id was already taken");
        return Error::Internal("hand-off could not be recorded".into());
    }

    error
}
