//! Arranging a task's lease, attempts, state and current hand-off — through
//! the tracker verbs.
//!
//! `ARCHITECTURE.md`, "Task tracker" → "One mutation at a time per project"
//! and `CLAUDE.md`, "Testing expectations", "Tracker tests": the rules that
//! compose a task's state, lease, `attempts`, closure and hand-off columns
//! live in `tracker/`, and the row helpers underneath are crate-private. A
//! precondition a suite needs — "this session already holds it", "this is the
//! last attempt", "there is code waiting on it", "an open prerequisite blocks
//! it" — is therefore arranged here the way production reaches it, with the
//! verb that composes it, rather than by writing the columns.
//!
//! That is not only a visibility question. `attempts` really is what three
//! claims left behind, a lease really is a claim's, and a current hand-off
//! really is a publication's — so an arrangement that the tracker would refuse
//! fails here rather than producing a row no code path can produce.
//!
//! It may `expect`: an arrangement that cannot be made has nothing to hand
//! back, and a panic naming the failed step is what a test author needs to
//! see. Every identity is obviously fake (rule 3).

use mars_orchestrator::events::TaskActor;
use mars_orchestrator::git::refs;
use mars_orchestrator::models::{
    HandoffCaller, NewTask, Task, TaskDependencyKind, TaskRef, TaskStateKind,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::TaskRepository;
use mars_orchestrator::tracker::handoffs::publish_in_transaction;
use mars_orchestrator::tracker::{
    PreparedHandoff, ReviewCarry, TaskDto, TrackerMutation, UpdateTaskInput, add_dependency,
    claim_for_launch, release_by_user, update_task,
};
use uuid::Uuid;

/// Run `body` against an open mutation on `project_id`, committing it when the
/// verb succeeded and rolling it back when it did not.
///
/// The pairing every transport makes (`tracker::commit_and_notify`), without
/// the email side an arrangement never owes.
pub async fn in_mutation<T, F>(
    pool: &PgPool,
    project_id: Uuid,
    actor: TaskActor,
    body: F,
) -> Result<T>
where
    F: AsyncFnOnce(&mut TrackerMutation<'_>) -> Result<T>,
{
    let mut mutation = TrackerMutation::begin(pool, project_id, actor).await?;
    match body(&mut mutation).await {
        Ok(outcome) => {
            mutation.commit().await?;
            Ok(outcome)
        }
        Err(error) => {
            mutation.no_change().await?;
            Err(error)
        }
    }
}

/// The task row as it stands under this mutation's lock.
///
/// What every verb wants as its `task` argument: the row read through the
/// mutation's own connection, not a stale one from the pool.
pub async fn locked(m: &mut TrackerMutation<'_>, task_id: Uuid) -> Result<Task> {
    let project_id = m.project_id();

    TaskRepository::new(m.pool())
        .find_task_for_update(m.conn(), project_id, TaskRef::Id(task_id))
        .await?
        .ok_or(Error::NotFound)
}

/// Put the lease on a task, the way launching a session for it does.
///
/// [`claim_for_launch`] rather than [`claim_for_profile`](mars_orchestrator::tracker::claim_for_profile):
/// the served states do not apply, so any non-terminal state qualifies and the
/// arrangement needs no profile. `attempts` ends at one, which is what one
/// claim means.
pub async fn hold(pool: &PgPool, project_id: Uuid, task_id: Uuid, session_id: Uuid) -> TaskDto {
    in_mutation(pool, project_id, TaskActor::System, async |m| {
        let task = locked(m, task_id).await?;
        claim_for_launch(m, &task, session_id).await
    })
    .await
    .expect("the claim succeeds")
}

/// Give the lease back the way a user does: the lease clears, the state and
/// `attempts` stay.
pub async fn release(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> TaskDto {
    in_mutation(pool, project_id, TaskActor::System, async |m| {
        let task = locked(m, task_id).await?;
        release_by_user(m, &task).await
    })
    .await
    .expect("the release succeeds")
}

/// `attempts` at `attempts`, with `session_id` still holding the lease.
///
/// A claim is the only thing that raises the counter and only a state change
/// resets it (`ARCHITECTURE.md`, "Task tracker" → "Attempts and escalation"),
/// so the count a suite wants is that many claims: the first `attempts - 1`
/// are given back by a user — a user release never escalates and never
/// touches the counter — and the last one is left standing.
pub async fn hold_with_attempts(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    attempts: i16,
) -> TaskDto {
    assert!(attempts >= 1, "a held task has been claimed at least once");

    for _ in 1..attempts {
        hold(pool, project_id, task_id, session_id).await;
        release(pool, project_id, task_id).await;
    }

    hold(pool, project_id, task_id, session_id).await
}

/// The same counter, with the task free again: `attempts` claims, all of them
/// given back.
pub async fn with_attempts(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    attempts: i16,
) -> TaskDto {
    assert!(attempts >= 1, "a counter is raised by claiming");

    hold_with_attempts(pool, project_id, task_id, session_id, attempts).await;

    release(pool, project_id, task_id).await
}

/// Link a session to a task without leaving it the lease.
///
/// The link is a claim's (`docs/data-model.md`, `task_sessions`), so the
/// arrangement is a claim and a release: `task_sessions` keeps the row, the
/// lease is gone and `attempts` is one.
pub async fn link_session(pool: &PgPool, project_id: Uuid, task_id: Uuid, session_id: Uuid) {
    hold(pool, project_id, task_id, session_id).await;
    release(pool, project_id, task_id).await;
}

/// Move a task into the state named `state`, through the verb every transport
/// moves a task with.
pub async fn move_to(pool: &PgPool, project_id: Uuid, task_id: Uuid, state: &str) -> TaskDto {
    in_mutation(pool, project_id, TaskActor::System, async |m| {
        let task = locked(m, task_id).await?;
        update_task(
            m,
            &task,
            UpdateTaskInput {
                state: Some(state.to_string()),
                ..UpdateTaskInput::default()
            },
        )
        .await
        .map(|outcome| outcome.task)
    })
    .await
    .expect("the state change succeeds")
}

/// Block a task the way an open `blocks` prerequisite does, and answer with
/// the prerequisite's id.
///
/// `blocked` is stored and recomputed, never assigned (`ARCHITECTURE.md`,
/// "Task tracker" → "Blocked is stored"), so the arrangement is the edge that
/// makes it true: a new open task of the same project, and a `blocks` edge to
/// it. Closing that task through [`move_to`] unblocks this one again.
pub async fn block(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> Uuid {
    in_mutation(pool, project_id, TaskActor::System, async |m| {
        let new = NewTask::new(project_id, &format!("prerequisite of {task_id}"))?;
        let prerequisite = TaskRepository::new(m.pool())
            .insert_task(m.conn(), project_id, &new)
            .await?;

        let task = locked(m, task_id).await?;
        add_dependency(m, &task, &prerequisite, TaskDependencyKind::Blocks).await?;

        Ok(prerequisite.id)
    })
    .await
    .expect("the prerequisite blocks the task")
}

/// A hand-off to publish on a task, as the fixture below takes it.
pub struct Handoff<'a> {
    /// The session the code comes from, or `None` for a forward whose session
    /// is gone.
    pub source_session_id: Option<Uuid>,
    /// The source session's branch name, `session/<id>` in production.
    pub source_branch: &'a str,
    /// The full object id the record pins.
    pub commit: &'a str,
    /// The message the hand-off carries to the next worker.
    pub comment: &'a str,
    /// The state the publication moves the task into. A hand-off always moves
    /// the task (`SPEC.md`, "Code hand-offs and review"), so this must name a
    /// state the task is not in.
    pub target_state: &'a str,
    /// Who is publishing: a session must hold the lease, a user need not.
    pub caller: HandoffCaller,
    /// What the record's review fields say. [`ReviewCarry::Fresh`] is a
    /// revision — unreviewed, always — and a decision is a forward's verdict,
    /// attributed to `caller` at the transaction's own timestamp.
    pub review: ReviewCarry,
}

/// Publish a hand-off on a task, through the verb, with the git half already
/// done.
///
/// [`publish_in_transaction`] is the only writer of `tasks.current_handoff_id`
/// and it touches no git at all: its input is the [`PreparedHandoff`] that
/// `tracker::handoffs::prepare` leaves behind once the commit is retained at
/// `refs/handoffs/<id>`. Every field of that value is public, so a suite with
/// no project repository builds it itself and the database half then runs
/// exactly as it does in production — the comment row, the `task_handoffs`
/// row, the pointer, the move through `update_task` and the session links.
/// The one thing that is imaginary is the ref behind `commit`, which only a
/// git test would read.
///
/// Answers with the task after the publication and the new record's id.
pub async fn publish_handoff(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    handoff: Handoff<'_>,
) -> (TaskDto, Uuid) {
    let actor = match handoff.caller {
        HandoffCaller::Session { session_id } => TaskActor::Session { session_id },
        HandoffCaller::User { user_id } => TaskActor::User { user_id },
    };

    in_mutation(pool, project_id, actor, async |m| {
        let task = locked(m, task_id).await?;
        let id = Uuid::new_v4();
        let prepared = PreparedHandoff {
            id,
            task_id,
            state_id: task.state_id,
            lease_holder_session_id: task.lease_holder_session_id,
            source_session_id: handoff.source_session_id,
            source_branch: handoff.source_branch.to_string(),
            commit: handoff.commit.to_string(),
            comment: handoff.comment.to_string(),
            previous_handoff_id: task.current_handoff_id,
            review: handoff.review.clone(),
            ref_name: refs::handoff_ref(id),
        };

        let dto = publish_in_transaction(
            m,
            &task,
            &prepared,
            &handoff.caller,
            UpdateTaskInput {
                state: Some(handoff.target_state.to_string()),
                ..UpdateTaskInput::default()
            },
        )
        .await?;

        Ok((dto, id))
    })
    .await
    .expect("the hand-off publishes")
}

/// Publish a hand-off and leave the task in the state it was already in.
///
/// A hand-off always moves the task (`SPEC.md`, "Code hand-offs and review"),
/// so "this task has code waiting on it" is not a state a single verb can
/// leave behind. It is two: publish into one of the project's other queue
/// columns and move back. The pointer survives the return trip, because a
/// plain state change neither publishes code nor withdraws it
/// (`tracker::state`) — and `attempts`, the lease and `closed_at` end up
/// exactly where the publication itself would have left them.
pub async fn handoff_in_place(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    handoff: Handoff<'_>,
) -> (TaskDto, Uuid) {
    let repository = TaskRepository::new(pool);
    let task = repository
        .find_task(project_id, TaskRef::Id(task_id))
        .await
        .expect("the task reads")
        .expect("the task is there");
    let states = repository
        .list_states(project_id)
        .await
        .expect("the states read");

    let home = states
        .iter()
        .find(|state| state.id == task.state_id)
        .expect("the task is in a state of its project")
        .name
        .clone();
    // A queue column, so the detour neither closes the task nor hands it to a
    // person on the way through.
    let via = states
        .iter()
        .find(|state| state.id != task.state_id && state.kind == TaskStateKind::Queue)
        .expect("the project has a second queue column")
        .name
        .clone();

    let (_, handoff_id) = publish_handoff(
        pool,
        project_id,
        task_id,
        Handoff {
            target_state: &via,
            ..handoff
        },
    )
    .await;

    (move_to(pool, project_id, task_id, &home).await, handoff_id)
}

/// An obviously fake but well-formed object id for [`with_rounds`]'s
/// revisions (rule 3). Nothing reads the ref behind it.
const ROUND_COMMIT: &str = "fedcba9876543210fedcba9876543210fedcba98";

/// `rounds` at `rounds`, the way production reaches it: that many revisions
/// published by `session_id` into `review_state`.
///
/// A revision is the only thing that raises the counter (`ARCHITECTURE.md`,
/// "Task tracker" → "Rounds"; ADR 0046), so the arrangement is that many
/// publications: each round the session claims the task, publishes a revision
/// into `review_state`, and — between rounds — the task goes back to the
/// state it started in by a plain move, which leaves the counter alone as long
/// as that state is not the human one. The task ends in `review_state`,
/// unheld, with the last revision current; the answer is the task and that
/// revision's id.
pub async fn with_rounds(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    session_id: Uuid,
    rounds: i16,
    review_state: &str,
) -> (TaskDto, Uuid) {
    assert!(rounds >= 1, "a round is a published revision");

    let home = TaskRepository::new(pool)
        .load_task_dto(project_id, task_id)
        .await
        .expect("the task reads")
        .expect("the task is there")
        .state;
    let branch = format!("session/{session_id}");

    let mut last = None;
    for round in 1..=rounds {
        if round > 1 {
            move_to(pool, project_id, task_id, &home).await;
        }
        hold(pool, project_id, task_id, session_id).await;
        last = Some(
            publish_handoff(
                pool,
                project_id,
                task_id,
                Handoff {
                    source_session_id: Some(session_id),
                    source_branch: &branch,
                    commit: ROUND_COMMIT,
                    comment: &format!("revision {round}"),
                    target_state: review_state,
                    caller: HandoffCaller::Session { session_id },
                    review: ReviewCarry::Fresh,
                },
            )
            .await,
        );
    }

    let (task, handoff_id) = last.expect("at least one round was published");
    assert_eq!(task.rounds, rounds, "each revision is one round");
    (task, handoff_id)
}
