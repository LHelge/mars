//! Reverting an integration head and, in the same confirmed action, reopening
//! the tasks merged since (`SPEC.md`, "Git": `POST /projects/{pid}/git/revert`;
//! `ARCHITECTURE.md`, "Git model", Revert; ADR 0053).
//!
//! The git half is [`GitService::revert_locked`]; this module composes it with
//! the tracker half in the documented lock order: the project **git lock**
//! first, held through both halves, then one [`TrackerMutation`] — the project
//! row — for every task that is reopened (`ARCHITECTURE.md`, "Git model",
//! Serialization; ADR 0021). The git lock staying held is what makes the list
//! of tasks the mutation reads the list the revert took back: no merge can
//! land on the head, and no hand-off be published, between the two.
//!
//! **What a reopened task gets.** Every task attributed in `reverted` whose
//! state is terminal, read under the mutation's lock, is
//!
//! 1. moved to the requested state through [`change_state`], which clears
//!    `closed_at`, the lease and `attempts` and recomputes what it blocks;
//! 2. relieved of its current hand-off through [`drop_handoff`], which writes
//!    the user's `comment` itself — or, for a task with no current hand-off,
//!    given that same comment through [`add_comment`], since the verb would
//!    refuse it with 409 and a revert must not;
//! 3. given one **system** comment naming the revert commit.
//!
//! So each reopened task carries the user's words exactly once, as the user's
//! comment, and the orchestrator's record of what happened as a comment of its
//! own: the reason is the user's to give and the commit is a fact, and folding
//! the second into the first would put words in the user's mouth that the
//! drawer shows under their name. A task in `reverted` that is not terminal is
//! listed there and left alone.
//!
//! **When the tracker half fails** after the head was written, the revert
//! stays: a failed push never rolls a merge back either. The whole mutation
//! rolls back, the failure is logged at `error` naming the commit, and the
//! caller gets 500 — the head has moved, and a user reading "internal error"
//! reloads the history and sees the revert there.
//!
//! Users only: nothing here is reachable over MCP (ADR 0053).

use uuid::Uuid;

use crate::events::TaskActor;
use crate::git::{GitService, RevertOutcome, revert};
use crate::models::{TaskRef, TaskState, TaskStateKind};
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::tracker::state::{
    StateChangeOptions, change_state, resolve_state, resolve_state_in_pool,
};
use crate::tracker::{
    CommentAuthor, TaskDto, TrackerMutation, add_comment, commit_and_notify, drop_handoff,
    retry_on_serialization_failure,
};

/// What a reopen whose target state is terminal is told (400).
pub const REOPEN_STATE_NOT_OPEN: &str = "reopen state must be a queue or human state";

/// What an empty reopen comment is told (400): the words every other comment
/// refusal uses.
const EMPTY_COMMENT: &str = "comment body must not be empty";

/// `POST /projects/{pid}/git/revert`'s input, as the route parsed it.
#[derive(Debug, Clone)]
pub struct RevertRequest {
    /// The integration head to revert.
    pub branch: String,
    /// The first-parent ancestor whose tree the head goes back to.
    pub to: String,
    /// The head the user confirmed against.
    pub expected_head: String,
    /// Reopen the terminal tasks of the reverted range, when present.
    pub reopen: Option<Reopen>,
}

/// Where the reverted tasks go, and why.
#[derive(Debug, Clone)]
pub struct Reopen {
    /// A queue or human state of the project, by name.
    pub state: String,
    /// The user's reason, written on every reopened task.
    pub comment: String,
}

/// What the revert did (`SPEC.md`, "Git").
#[derive(Debug, Clone)]
pub struct RevertResult {
    /// The revert commit and the attributed range it took back.
    pub outcome: RevertOutcome,
    /// The tasks reopened, as they stand after the mutation.
    pub reopened: Vec<TaskDto>,
}

/// Revert `request.branch` to `request.to` as `user_id`, and reopen the
/// reverted range's terminal tasks when `request.reopen` asks to.
///
/// Every refusal that needs no lock is decided first, so a malformed request,
/// an unknown or terminal reopen state, an empty comment and a project that
/// is not ready change nothing (400, 400, 400 and 409; 404 for an unknown
/// project). Under the git lock the head is compared with `expected_head`
/// (409 `branch has moved`) and `to` checked (400), still before anything is
/// written.
pub async fn revert_and_reopen(
    state: &AppState,
    project_id: Uuid,
    user_id: Uuid,
    request: RevertRequest,
) -> Result<RevertResult> {
    GitService::check_revert_request(&request.branch, &request.to, &request.expected_head)?;
    let service = GitService::from_state(state);
    service.require_ready(project_id).await?;
    if let Some(reopen) = &request.reopen {
        check_reopen(&state.pool, project_id, reopen).await?;
    }

    let guard = state.git_locks.lock(project_id).await;
    let outcome = service
        .revert_locked(
            &guard,
            &request.branch,
            &request.to,
            &request.expected_head,
            user_id,
        )
        .await?;

    let Some(reopen) = &request.reopen else {
        return Ok(RevertResult {
            outcome,
            reopened: Vec::new(),
        });
    };

    let candidates = reverted_tasks(&outcome);
    let reopened = if candidates.is_empty() {
        Vec::new()
    } else {
        let line = system_line(&request.branch, &request.to, &outcome.commit);
        let reopened = retry_on_serialization_failure("revert_reopen", || async {
            reopen_tasks(state, project_id, user_id, &candidates, reopen, &line).await
        })
        .await;

        match reopened {
            Ok(reopened) => reopened,
            Err(err) => {
                error!(
                    project_id = %project_id,
                    commit = %outcome.commit,
                    error = %err,
                    "the revert was written but reopening its tasks failed; the revert stays",
                );
                return Err(Error::Internal(format!(
                    "reopening the tasks of revert {} failed",
                    outcome.commit
                )));
            }
        }
    };

    // The guard goes here, after the tracker commit.
    drop(guard);

    Ok(RevertResult { outcome, reopened })
}

/// The reopen's own refusals, before any lock: a non-empty comment and a
/// state of the project that is not terminal.
async fn check_reopen(pool: &PgPool, project_id: Uuid, reopen: &Reopen) -> Result<()> {
    if reopen.comment.trim().is_empty() {
        return Err(Error::BadRequest(EMPTY_COMMENT.to_string()));
    }
    let target = resolve_state_in_pool(pool, project_id, &reopen.state).await?;
    require_open(&target)
}

/// A queue or human state, or 400.
fn require_open(target: &TaskState) -> Result<()> {
    if target.kind == TaskStateKind::Terminal {
        return Err(Error::BadRequest(REOPEN_STATE_NOT_OPEN.to_string()));
    }

    Ok(())
}

/// Every task the reverted range names, once each, in the order `reverted`
/// first names them: newest entry first.
fn reverted_tasks(outcome: &RevertOutcome) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = Vec::new();
    for task in outcome.reverted.iter().flat_map(|entry| &entry.tasks) {
        if !ids.contains(&task.id) {
            ids.push(task.id);
        }
    }

    ids
}

/// The system comment a reopened task gets, naming the revert commit.
fn system_line(branch: &str, to: &str, commit: &str) -> String {
    format!(
        "Reopened by the revert of {branch} to {}: commit {commit}.",
        revert::short(to)
    )
}

/// The tracker half: one mutation over every candidate.
async fn reopen_tasks(
    state: &AppState,
    project_id: Uuid,
    user_id: Uuid,
    candidates: &[Uuid],
    reopen: &Reopen,
    line: &str,
) -> Result<Vec<TaskDto>> {
    let mut m =
        TrackerMutation::begin(&state.pool, project_id, TaskActor::User { user_id }).await?;

    let reopened = async {
        // Again under the lock: the state could have been deleted or made
        // terminal since the check before the git lock.
        let target = resolve_state(&mut m, &reopen.state).await?;
        require_open(&target)?;

        let mut reopened = Vec::new();
        for &task_id in candidates {
            if let Some(task) = reopen_one(&mut m, task_id, &target, user_id, reopen, line).await? {
                reopened.push(task);
            }
        }

        Ok::<_, Error>(reopened)
    }
    .await;

    match reopened {
        Ok(reopened) => {
            commit_and_notify(m, state).await?;
            Ok(reopened)
        }
        Err(err) => {
            if let Err(rollback) = m.no_change().await {
                warn!(
                    project_id = %project_id,
                    error = %rollback,
                    "a failed reopen's transaction could not be rolled back",
                );
            }
            Err(err)
        }
    }
}

/// Reopen one task when it is terminal; `None` when it is not, or is gone.
async fn reopen_one(
    m: &mut TrackerMutation<'_>,
    task_id: Uuid,
    target: &TaskState,
    user_id: Uuid,
    reopen: &Reopen,
    line: &str,
) -> Result<Option<TaskDto>> {
    let project_id = m.project_id();
    let repository = TaskRepository::new(m.pool());

    // A task deleted since its hand-off was merged is not in the attribution
    // any more, but one deleted between the read and this lock would be.
    let Some(task) = repository
        .find_task_for_update(m.conn(), project_id, TaskRef::Id(task_id))
        .await?
    else {
        return Ok(None);
    };
    let current = repository
        .find_state_in(m.conn(), project_id, task.state_id)
        .await?
        .ok_or_else(|| Error::Internal("task state is missing".into()))?;
    if current.kind != TaskStateKind::Terminal {
        return Ok(None);
    }

    let moved = change_state(m, &task, target, StateChangeOptions::default()).await?;

    // The user's comment once: the drop writes it when there is a hand-off to
    // drop, and a task without one gets it directly.
    let dto = if task.current_handoff_id.is_some() {
        drop_handoff(m, task.id, user_id, &reopen.comment).await?
    } else {
        add_comment(m, &task, CommentAuthor::User(user_id), &reopen.comment).await?;
        moved.task
    };
    add_comment(m, &task, CommentAuthor::System, line).await?;

    Ok(Some(dto))
}
