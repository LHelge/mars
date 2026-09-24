//! The auto-merge job: approved hand-offs merged by the orchestrator.
//!
//! `ARCHITECTURE.md`, "Task tracker" → "Automatic merges" and "Background
//! jobs"; ADR 0045. A task in a state with `auto_merge` set whose current
//! hand-off is approved needs no judgement to merge: the tracker knows the
//! hand-off and its review, and git knows whether it conflicts. Each run walks
//! the `ready`, unpaused projects and, within each, the unheld and unblocked
//! tasks of their `auto_merge` states in `ready` order — priority, then
//! number — and handles one task at a time:
//!
//! 1. take the project git lock, and re-read the project, the task and its
//!    current hand-off: the candidate list was a lock-free read, and a task
//!    that no longer qualifies is skipped;
//! 2. escalate a task with no approved current hand-off to the human state —
//!    it can only have been moved there by hand;
//! 3. skip the merge when the default branch already contains the hand-off's
//!    commit, which is the run after a crash between a merge and its tracker
//!    commit;
//! 4. otherwise run the task merge of "Git model" as [`GitActor::System`];
//! 5. still under the git lock, one tracker mutation that re-checks the state
//!    and the current hand-off and moves the task to the first terminal state,
//!    or on a conflict sends it back to the state's `conflict_state` through
//!    the round-limit rule ([`send_back`]).
//!
//! **Lock order.** The git lock first and held through the tracker commit,
//! then the project row inside the mutation (ADR 0021): the order hand-off
//! publication already follows, so a new revision or a forward cannot change
//! the hand-off under the job. A person's plain move takes no git lock, which
//! is the one race left, and step 5 answers it with a comment instead of a
//! move.
//!
//! **Failures are the instance's.** A merge that fails for any reason but a
//! conflict leaves the task where it is, is logged at `error` and counted, and
//! the next run retries it; so is a tracker mutation that fails after a
//! successful merge, which the next run finds at step 3.
//!
//! **Woken like the dispatcher.** The same waker on the shared listener's
//! fan-out ([`super::dispatcher::spawn_waker`]) signals this job's own loop, so
//! an approved task is normally merged within a second of arriving and a run
//! still never overlaps another. Every move this job makes writes task events
//! and so wakes it once more; that run finds nothing and writes nothing.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{CronService, JobReport};
use crate::events::TaskActor;
use crate::git::{GitActor, GitService};
use crate::models::{ProjectStatus, ReviewStatus, Task, TaskHandoff, TaskRef, TaskState};
use crate::prelude::*;
use crate::repositories::{ProjectRepository, TaskRepository};
use crate::tracker::comments::{CommentAuthor, add_comment};
use crate::tracker::leases::{READY_MAX_LIMIT, human_state};
use crate::tracker::state::{
    StateChangeOptions, StateEventKind, change_state, lowest_terminal_state, resolve_state,
};
use crate::tracker::{Escalation, TrackerMutation, commit_and_notify, send_back};

/// Why a project or a task was passed over, as the `reason` log field.
const SKIP_PAUSED: &str = "automation_paused";
/// The task no longer qualifies under the git lock: moved, claimed, blocked,
/// deleted, or its project is no longer ready.
const SKIP_STALE: &str = "no_longer_candidate";

/// What handling one candidate came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handled {
    /// Merged, or found merged, and moved to the first terminal state.
    Merged,
    /// The merge conflicted and the task was sent back — or, at the round
    /// limit, escalated.
    SentBack,
    /// No approved current hand-off: escalated to the human state.
    Escalated,
    /// The code is merged but a person moved the task out of the state
    /// first; the comment says so and the task stays where it is.
    Left,
    /// No longer a candidate by the time the lock was held.
    Skipped,
}

/// The outcome of steps 3 and 4.
#[derive(Debug)]
enum Merge {
    /// The default branch contains the hand-off commit, now or already.
    Merged,
    /// The merge stopped on these paths and the default branch is untouched.
    Conflict(Vec<String>),
}

/// `Merged <commit> into <branch>.`
fn merged_comment(commit: &str, branch: &str) -> String {
    format!("Merged {commit} into {branch}.")
}

/// `Merged <commit> into <branch>, but the task had left <state>; its state is
/// unchanged.`
fn left_comment(commit: &str, branch: &str, state: &str) -> String {
    format!("Merged {commit} into {branch}, but the task had left {state}; its state is unchanged.")
}

/// The conflict send-back's comment: the fixed sentence and one path per line.
fn conflict_comment(branch: &str, paths: &[String]) -> String {
    let mut comment = format!(
        "Merge into {branch} conflicted; bring the branch up to date and hand off a new \
         revision. Conflicting paths:"
    );
    for path in paths {
        comment.push('\n');
        comment.push_str(path);
    }
    comment
}

/// The escalation reason for a task in an auto-merge state that has nothing
/// approved to merge.
fn unapproved_reason(state: &str) -> String {
    format!("{state} is an auto-merge state and the task has no approved hand-off")
}

impl CronService {
    /// Merge the approved hand-offs of every `ready`, unpaused project's
    /// auto-merge states, one task at a time.
    ///
    /// `now` is unused: nothing here is measured against a clock.
    ///
    /// The counters: `items` is a task merged, sent back, escalated or
    /// commented on; `skipped` is a paused project or a candidate that no
    /// longer qualified under the lock; `failures` is a task whose merge or
    /// tracker commit failed for a reason that is not a conflict, logged and
    /// left for the next run.
    ///
    /// # Errors
    ///
    /// Only from listing the projects. Everything after that is counted.
    pub async fn auto_merge(&self, now: DateTime<Utc>) -> Result<JobReport> {
        let _ = now;

        let projects = ProjectRepository::new(&self.state.pool);
        let tasks = TaskRepository::new(&self.state.pool);
        let project_ids = projects.list_ids_by_status(ProjectStatus::Ready).await?;

        let mut report = JobReport::default();

        for project_id in project_ids {
            let Some(project) = projects.find(project_id).await? else {
                continue;
            };
            if project.status != ProjectStatus::Ready {
                continue;
            }
            if project.automation_paused {
                debug!(project_id = %project_id, reason = SKIP_PAUSED, "auto-merge skipped");
                report.skipped += 1;
                continue;
            }

            // Lock-free: the re-read under the git lock decides.
            let candidates = match self.candidates(&tasks, project_id).await {
                Ok(candidates) => candidates,
                Err(error) => {
                    error!(
                        project_id = %project_id,
                        error = %error,
                        "the auto-merge job could not list a project's candidates",
                    );
                    report.failures += 1;
                    continue;
                }
            };

            for (state, task_id) in candidates {
                match self.merge_task(project_id, &state, task_id).await {
                    Ok(Handled::Skipped) => {
                        debug!(
                            project_id = %project_id,
                            task_id = %task_id,
                            reason = SKIP_STALE,
                            "auto-merge skipped",
                        );
                        report.skipped += 1;
                    }
                    Ok(_) => report.items += 1,
                    Err(error) => {
                        error!(
                            project_id = %project_id,
                            task_id = %task_id,
                            error = %error,
                            "auto-merge failed; retried at the next run",
                        );
                        report.failures += 1;
                    }
                }
            }
        }

        Ok(report)
    }

    /// The unheld, unblocked tasks of the project's auto-merge states, with
    /// the state each is in, in `ready` order.
    ///
    /// The `ready` tool's own query over the auto-merge states rather than
    /// those a profile serves. One page per run: a task handled leaves the
    /// state and its events wake the job again, so a longer queue drains over
    /// successive runs.
    async fn candidates(
        &self,
        tasks: &TaskRepository<'_>,
        project_id: Uuid,
    ) -> Result<Vec<(TaskState, Uuid)>> {
        let states: Vec<TaskState> = tasks
            .list_states(project_id)
            .await?
            .into_iter()
            .filter(|state| state.auto_merge)
            .collect();
        if states.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<Uuid> = states.iter().map(|state| state.id).collect();
        let rows = tasks
            .list_claimable(project_id, &ids, READY_MAX_LIMIT)
            .await?;

        Ok(rows
            .into_iter()
            .filter_map(|row| {
                states
                    .iter()
                    .find(|state| state.name == row.state)
                    .map(|state| (state.clone(), row.id))
            })
            .collect())
    }

    /// Steps 1 to 5 for one task, under the project git lock throughout.
    async fn merge_task(
        &self,
        project_id: Uuid,
        state: &TaskState,
        task_id: Uuid,
    ) -> Result<Handled> {
        let service = GitService::from_state(&self.state);
        let tasks = TaskRepository::new(&self.state.pool);

        // Held until this function returns, which is after the tracker
        // mutation below has committed.
        let guard = self.state.git_locks.lock(project_id).await;

        // Step 1: plain reads. Under the git lock the hand-off cannot change;
        // the state and the lease can, which the mutation re-checks.
        let Some(project) = ProjectRepository::new(&self.state.pool)
            .find(project_id)
            .await?
        else {
            return Ok(Handled::Skipped);
        };
        if project.status != ProjectStatus::Ready || project.automation_paused {
            return Ok(Handled::Skipped);
        }
        let Some(branch) = project.default_branch else {
            return Err(Error::Internal(
                "a ready project has no default branch".into(),
            ));
        };

        let Some(task) = tasks.find_task(project_id, TaskRef::Id(task_id)).await? else {
            return Ok(Handled::Skipped);
        };
        if !still_candidate(&task, state) {
            return Ok(Handled::Skipped);
        }

        let approved = match task.current_handoff_id {
            Some(handoff_id) => tasks
                .find_handoff(project_id, handoff_id)
                .await?
                .filter(|handoff| handoff.review_status == ReviewStatus::Approved),
            None => None,
        };

        // Step 2.
        let Some(handoff) = approved else {
            return self.escalate_unapproved(project_id, state, &task).await;
        };

        // Steps 3 and 4.
        let merge = if service
            .is_merged_into(&guard, &handoff.commit, &branch)
            .await?
        {
            debug!(
                project_id = %project_id,
                task_id = %task.id,
                "the hand-off is already on the default branch",
            );
            Merge::Merged
        } else {
            match service
                .merge_handoff_locked(
                    &guard,
                    project_id,
                    task.id,
                    handoff.id,
                    &branch,
                    None,
                    &GitActor::System,
                )
                .await
            {
                Ok(_) => Merge::Merged,
                Err(error) => match error.conflicts() {
                    Some(paths) => Merge::Conflict(paths),
                    None => return Err(error),
                },
            }
        };

        // Step 5, still holding `guard`.
        let handled = self
            .settle(project_id, state, &task, &handoff, &branch, merge)
            .await?;
        drop(guard);

        Ok(handled)
    }

    /// Step 5: the one tracker mutation after the merge.
    async fn settle(
        &self,
        project_id: Uuid,
        state: &TaskState,
        task: &Task,
        handoff: &TaskHandoff,
        branch: &str,
        merge: Merge,
    ) -> Result<Handled> {
        let mut m = TrackerMutation::begin(&self.state.pool, project_id, TaskActor::System).await?;
        let tasks = TaskRepository::new(m.pool());

        let Some(current) = tasks
            .find_task_for_update(m.conn(), project_id, TaskRef::Id(task.id))
            .await?
        else {
            // Deleted since the lock-free read; there is nothing to move and
            // nowhere to comment.
            m.no_change().await?;
            return Ok(Handled::Skipped);
        };
        let unchanged =
            current.state_id == state.id && current.current_handoff_id == Some(handoff.id);

        let handled = match merge {
            Merge::Merged if unchanged => {
                add_comment(
                    &mut m,
                    &current,
                    CommentAuthor::System,
                    &merged_comment(&handoff.commit, branch),
                )
                .await?;
                let terminal = lowest_terminal_state(&mut m).await?;
                change_state(&mut m, &current, &terminal, StateChangeOptions::default()).await?;
                Handled::Merged
            }
            Merge::Merged => {
                add_comment(
                    &mut m,
                    &current,
                    CommentAuthor::System,
                    &left_comment(&handoff.commit, branch, &state.name),
                )
                .await?;
                Handled::Left
            }
            Merge::Conflict(ref paths) if unchanged => {
                // The state as it stands under the lock: its conflict state
                // may have been reconfigured, or the flag turned off, since
                // the listing.
                let conflict_state = tasks
                    .find_state_in(m.conn(), project_id, state.id)
                    .await?
                    .and_then(|state| state.conflict_state);
                let Some(conflict_state) = conflict_state else {
                    m.no_change().await?;
                    return Ok(Handled::Skipped);
                };
                let target = resolve_state(&mut m, &conflict_state).await?;

                let comment = conflict_comment(branch, paths);
                add_comment(&mut m, &current, CommentAuthor::System, &comment).await?;
                send_back(&mut m, &current, &target, &comment).await?;
                Handled::SentBack
            }
            Merge::Conflict(_) => {
                // Nothing was merged and the task is somebody else's now.
                m.no_change().await?;
                return Ok(Handled::Skipped);
            }
        };

        commit_and_notify(m, &self.state).await?;

        info!(
            project_id = %project_id,
            task_id = %task.id,
            handoff_id = %handoff.id,
            outcome = ?handled,
            "auto-merge",
        );

        Ok(handled)
    }

    /// Step 2: a task in an auto-merge state with nothing approved to merge.
    ///
    /// Escalated with the reason as a system comment, in `needs_human_reason`
    /// and on the `escalated` event, and the escalation email the move owes —
    /// the shape every escalation by the orchestrator has.
    async fn escalate_unapproved(
        &self,
        project_id: Uuid,
        state: &TaskState,
        task: &Task,
    ) -> Result<Handled> {
        let mut m = TrackerMutation::begin(&self.state.pool, project_id, TaskActor::System).await?;

        let Some(current) = TaskRepository::new(m.pool())
            .find_task_for_update(m.conn(), project_id, TaskRef::Id(task.id))
            .await?
        else {
            m.no_change().await?;
            return Ok(Handled::Skipped);
        };
        if !still_candidate(&current, state)
            || current.current_handoff_id != task.current_handoff_id
        {
            m.no_change().await?;
            return Ok(Handled::Skipped);
        }

        let reason = unapproved_reason(&state.name);
        add_comment(&mut m, &current, CommentAuthor::System, &reason).await?;

        let human = human_state(&mut m).await?;
        change_state(
            &mut m,
            &current,
            &human,
            StateChangeOptions {
                event: StateEventKind::Escalated {
                    reason: reason.clone(),
                },
                needs_human_reason: Some(reason.clone()),
            },
        )
        .await?;
        m.record_escalation(Escalation {
            project_id,
            project_name: m.project().name.clone(),
            task_id: current.id,
            task_number: current.number,
            task_title: current.title.clone(),
            assignee_user_id: current.assignee_user_id,
            reason,
        });

        commit_and_notify(m, &self.state).await?;

        info!(
            project_id = %project_id,
            task_id = %task.id,
            "auto-merge escalated a task with no approved hand-off",
        );

        Ok(Handled::Escalated)
    }
}

/// Is this row still what the candidate listing selected: in the state,
/// unheld and unblocked?
fn still_candidate(task: &Task, state: &TaskState) -> bool {
    task.state_id == state.id && task.lease_holder_session_id.is_none() && !task.blocked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_comments_have_the_documented_wording() {
        assert_eq!(merged_comment("abc", "main"), "Merged abc into main.",);
        assert_eq!(
            left_comment("abc", "main", "merge"),
            "Merged abc into main, but the task had left merge; its state is unchanged.",
        );
        assert_eq!(
            conflict_comment("main", &["a.txt".to_string(), "b/c.rs".to_string()]),
            "Merge into main conflicted; bring the branch up to date and hand off a new \
             revision. Conflicting paths:\na.txt\nb/c.rs",
        );
        assert_eq!(
            unapproved_reason("merge"),
            "merge is an auto-merge state and the task has no approved hand-off",
        );
    }
}
