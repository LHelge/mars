//! The one code path every tracker writer goes through.
//!
//! `ARCHITECTURE.md`, "Task tracker" → "One mutation at a time per project"
//! and `docs/data-model.md`, "Tracker mutation transactions" describe a shape
//! rather than a helper: lock the project row, validate under the lock, apply
//! the change, recompute what it affects, append every resulting event with
//! successive sequences, and commit all of it together. [`TrackerMutation`] is
//! that shape as a type, so the rule is followed by construction instead of
//! being remembered at each call site.
//!
//! It owns three things a writer would otherwise have to carry by hand:
//!
//! - **the locked transaction**, from `TaskRepository::begin_mutation`, handed
//!   to repository helpers as a [`Locked`] token through
//!   [`TrackerMutation::conn`] — and `Locked` is what every tracker write
//!   takes, so a write outside the project lock does not compile;
//! - **the event batch**, collected in emission order by the `emit_*` methods
//!   and appended once on commit, so the sequences are allocated together and
//!   `pg_notify` fires once, inside the transaction (ADR 0028);
//! - **the side effects that must not happen until it commits**: the
//!   `task_sessions` links a change owes (ADR 0030) and the escalation emails
//!   it owes, which are handed back to the caller to send *after* the commit,
//!   because email has no place inside a lock.
//!
//! Nothing is broadcast before commit and rollback exposes neither the change
//! nor its events, so a rejected or no-op operation writes nothing at all —
//! which is what `SPEC.md`, "MCP tool contracts" promises every tool caller.
//! [`TrackerMutation::no_change`] is the explicit way to say so.
//!
//! Later tasks — states, state changes, claims, dependencies, comments,
//! deletion — are written as functions taking `&mut TrackerMutation`, which is
//! how they come to share one transaction without passing one around.

use std::ops::{Deref, DerefMut};

use chrono::Utc;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::events::{TaskActor, TaskEvent, TaskEventKind, TaskEventPayload};
use crate::models::{NewTaskEvent, Project, TaskState};
use crate::prelude::*;
use crate::repositories::{ProjectRepository, TaskRepository};
use crate::tracker::{CommentDto, Escalation, TaskDto};

/// A connection that is known to be inside a project-locked tracker mutation.
///
/// `ARCHITECTURE.md`, "Task tracker" → "One mutation at a time per project"
/// says every tracker writer locks the project row first and shares that one
/// transaction. This type is that sentence as a compile-time fact: the field
/// is private to this module, so the only way to obtain a `Locked` is
/// [`TrackerMutation::conn`] — and every `TaskRepository` helper that writes a
/// tracker table, or has to read one under the lock, takes a `Locked` rather
/// than a bare `&mut PgConnection`. A write outside the lock therefore does
/// not compile, and the module doc of `repositories/tasks` states the rule
/// once instead of every helper repeating a warning nobody can enforce.
///
/// It derefs to the connection because the helpers still run ordinary SQL on
/// it; [`Locked::reborrow`] is how a helper hands the token on to another one
/// without giving it up.
pub struct Locked<'a>(&'a mut PgConnection);

impl<'a> Locked<'a> {
    /// The one documented exception: creating a project.
    ///
    /// `projects::create_project` inserts the project, its default states and
    /// its default profile's served states in one transaction, and there is no
    /// project row to lock yet — the row it would lock is the row it is
    /// inserting, still invisible to every other transaction, so there is
    /// nothing for a lock to serialise against (`docs/data-model.md`, "Tracker
    /// mutation transactions"). That transaction is as exclusive as the lock
    /// would make it, so it may mint a token; nothing else may, which is why
    /// this is `pub(crate)` and named after its single caller.
    pub(crate) fn during_project_creation(conn: &'a mut PgConnection) -> Self {
        Self(conn)
    }

    /// Hand the token to a nested helper for the length of the borrow.
    ///
    /// The same move as sqlx's `&mut *tx`, one level up: the caller keeps the
    /// token and the callee gets one of its own for the call.
    pub fn reborrow(&mut self) -> Locked<'_> {
        Locked(self.0)
    }
}

impl Deref for Locked<'_> {
    type Target = PgConnection;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl DerefMut for Locked<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
    }
}

/// What a committed mutation leaves for its caller.
///
/// The sequences are the ones `TaskRepository::append_task_events` allocated,
/// in emission order — a handler that answers with a cursor, or a test that
/// asserts the stream, reads them here. The escalations are the emails the
/// commit made due; the caller sends them, outside the transaction that is now
/// closed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MutationOutcome {
    /// The `task_events.seq` values written, in emission order; empty when the
    /// mutation emitted nothing.
    pub seqs: Vec<i64>,
    /// The escalation emails the caller now owes.
    pub escalations: Vec<Escalation>,
}

impl MutationOutcome {
    /// The highest sequence written, or `None` when nothing was.
    ///
    /// The value the batch's single notification carried.
    pub fn last_seq(&self) -> Option<i64> {
        self.seqs.last().copied()
    }
}

/// One tracker mutation: the project-locked transaction and everything the
/// change owes when it commits.
///
/// Opened with [`TrackerMutation::begin`], finished with
/// [`TrackerMutation::commit`] or [`TrackerMutation::no_change`]. Dropping it
/// without either rolls the transaction back, which is the correct answer to
/// an error path: no rows, no events, no notification, no email.
pub struct TrackerMutation<'a> {
    /// The transaction holding `SELECT ... FOR UPDATE` on the project row.
    tx: Transaction<'a, Postgres>,
    /// The project every id in this mutation is scoped to.
    project_id: Uuid,
    /// The project row as it was under the lock; `max_attempts` and `name` are
    /// read from here rather than re-queried.
    project: Project,
    /// Who is making the change; every event payload carries it.
    actor: TaskActor,
    /// The batch, in emission order. `append_task_events` allocates successive
    /// sequences, so this order *is* the stream's order.
    events: Vec<NewTaskEvent>,
    /// `(task, session)` pairs to upsert into `task_sessions`, deduplicated.
    touches: Vec<(Uuid, Uuid)>,
    /// Emails the caller sends once this has committed.
    escalations: Vec<Escalation>,
    /// The pool the transaction came from, so the repositories can be
    /// rebuilt on commit without the caller passing one back in.
    pool: &'a PgPool,
}

impl<'a> TrackerMutation<'a> {
    /// Open a mutation: begin the transaction, lock the project row, read it.
    ///
    /// The read is deliberately inside the lock. `max_attempts` decides
    /// whether a release escalates, and a value read before the lock could
    /// already be another mutation's old one (`ARCHITECTURE.md`, "Task
    /// tracker" → "Attempts and escalation").
    ///
    /// An unknown project is [`Error::NotFound`], raised before anything is
    /// written. A project locked by another mutation makes this call *wait*:
    /// there is no timeout of its own, only the pool's acquire timeout, which
    /// applies before the lock is ever reached.
    pub async fn begin(pool: &'a PgPool, project_id: Uuid, actor: TaskActor) -> Result<Self> {
        let mut tx = TaskRepository::new(pool).begin_mutation(project_id).await?;

        let Some(project) = ProjectRepository::new(pool)
            .find_in(&mut tx, project_id)
            .await?
        else {
            // `begin_mutation` already refuses a missing project; this is the
            // row vanishing between the lock and the read, which it cannot.
            return Err(Error::NotFound);
        };

        Ok(Self {
            tx,
            project_id,
            project,
            actor,
            events: Vec::new(),
            touches: Vec::new(),
            escalations: Vec::new(),
            pool,
        })
    }

    /// The locked connection, for the repository helpers this mutation runs.
    ///
    /// The only way to build a [`Locked`], and therefore the only way to reach
    /// a tracker write at all. Everything read through it is authoritative for
    /// this project, and everything written through it commits or rolls back
    /// with the events. **Never call engine, email, git or model code while
    /// holding it**: the project lock is held for exactly as long as this
    /// mutation lives.
    ///
    /// A combined tracker/session operation takes its session connection from
    /// here too, which is what keeps the documented lock order — git, then the
    /// project row, then session rows — true by construction.
    pub fn conn(&mut self) -> Locked<'_> {
        Locked(&mut self.tx)
    }

    /// Who is making this change.
    pub fn actor(&self) -> TaskActor {
        self.actor
    }

    /// The project row, read under the lock.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The project this mutation is scoped to.
    pub fn project_id(&self) -> Uuid {
        self.project_id
    }

    /// Queue one event, timestamped now, with this mutation's actor.
    ///
    /// Order matters: the batch is appended in the order it was emitted and
    /// the sequences follow, so a release with a comment emits `commented`
    /// before `state_changed`, and a state change emits its own event before
    /// its dependants' `unblocked`.
    pub fn emit(
        &mut self,
        kind: TaskEventKind,
        task_id: Option<Uuid>,
        payload: TaskEventPayload,
    ) -> Result<()> {
        // `seq` is the repository's to allocate under the lock, so the event
        // is built with a placeholder and `to_new_event` drops it again.
        let event = TaskEvent::new(0, Utc::now(), task_id, kind, payload);
        self.events.push(event.to_new_event()?);

        Ok(())
    }

    /// An event about a task, carrying the task as it is *after* the change.
    ///
    /// The payload `SPEC.md` gives `created`, `updated`, `claimed`,
    /// `released`, `blocked` and `unblocked`: the actor and the full task.
    /// Load the [`TaskDto`] with
    /// [`TaskRepository::load_task_dto_in`] on [`TrackerMutation::conn`] after
    /// the row change, or the event describes the task as it was.
    pub fn emit_task(&mut self, kind: TaskEventKind, task: &TaskDto) -> Result<()> {
        let mut payload = TaskEventPayload::new(self.actor);
        payload.task = Some(task.clone());

        self.emit(kind, Some(task.id), payload)
    }

    /// A `state_changed` or `escalated` event: the task, the states it moved
    /// between and why.
    ///
    /// `reason` is `None` for an ordinary hand-off and `Some` for an
    /// escalation or a release that carries one (`SPEC.md`, "TaskEvent").
    pub fn emit_state(
        &mut self,
        kind: TaskEventKind,
        task: &TaskDto,
        from: &str,
        to: &str,
        reason: Option<&str>,
    ) -> Result<()> {
        let mut payload = TaskEventPayload::new(self.actor);
        payload.task = Some(task.clone());
        payload.from = Some(from.to_string());
        payload.to = Some(to.to_string());
        payload.reason = reason.map(str::to_string);

        self.emit(kind, Some(task.id), payload)
    }

    /// A `commented` event: the task and the comment that was written.
    pub fn emit_comment(&mut self, task: &TaskDto, comment: &CommentDto) -> Result<()> {
        let mut payload = TaskEventPayload::new(self.actor);
        payload.task = Some(task.clone());
        payload.comment = Some(comment.clone());

        self.emit(TaskEventKind::Commented, Some(task.id), payload)
    }

    /// A `deleted` event: the original task id and nothing but the actor.
    ///
    /// The task row is gone by the time this commits, so there is no `task` to
    /// carry; the id is retained so history is never rewritten (ADR 0022).
    pub fn emit_deleted(&mut self, task_id: Uuid) -> Result<()> {
        self.emit(
            TaskEventKind::Deleted,
            Some(task_id),
            TaskEventPayload::new(self.actor),
        )
    }

    /// A `states_changed` event: the project's full state list, no task.
    ///
    /// The board's columns changed, which is a project-wide fact, so `task_id`
    /// is `NULL` (`SPEC.md`, "TaskEvent").
    pub fn emit_states_changed(&mut self, states: &[TaskState]) -> Result<()> {
        let mut payload = TaskEventPayload::new(self.actor);
        payload.states = Some(states.to_vec());

        self.emit(TaskEventKind::StatesChanged, None, payload)
    }

    /// Record that this session changed this task.
    ///
    /// The link is written on commit, never before: `task_sessions` is
    /// history, so a rejected or no-op operation must leave no trace of having
    /// been attempted (`docs/data-model.md`, `task_sessions`; ADR 0030).
    /// Recording the same pair twice in one mutation upserts once.
    pub fn touch(&mut self, task_id: Uuid, session_id: Uuid) {
        let pair = (task_id, session_id);
        if !self.touches.contains(&pair) {
            self.touches.push(pair);
        }
    }

    /// The same, for whoever is making this change — when that is a session.
    ///
    /// A user's change and the orchestrator's own are not session work, so
    /// they create no link: `task_sessions` answers "which sessions worked on
    /// this task", and a reaper release is not a session having worked on it.
    pub fn touch_actor(&mut self, task_id: Uuid) {
        if let TaskActor::Session { session_id } = self.actor {
            self.touch(task_id, session_id);
        }
    }

    /// Queue the escalation email this change made due.
    ///
    /// Returned by [`TrackerMutation::commit`] for the caller to send once the
    /// transaction is closed; a rollback sends nothing.
    pub fn record_escalation(&mut self, escalation: Escalation) {
        self.escalations.push(escalation);
    }

    /// Append the events, write the session links and commit everything.
    ///
    /// The whole batch goes through `TaskRepository::append_task_events` in
    /// one call, so the sequences are successive and exactly one
    /// `pg_notify('task_events', '<project_id>:<seq>')` is issued, inside this
    /// transaction and therefore delivered only on commit (ADR 0028). A second
    /// notify is never added here.
    ///
    /// A mutation that emitted nothing still commits — a read taken under the
    /// lock, or a row change that owes the stream nothing, is legal — but it
    /// notifies nothing and writes no links, because a link records a *change*
    /// and there was none to announce.
    ///
    /// A sequence collision surfaces as [`Error::Internal`] and takes the row
    /// changes down with it: it means a writer allocated outside the lock, not
    /// something to retry.
    pub async fn commit(self) -> Result<MutationOutcome> {
        let Self {
            mut tx,
            project_id,
            events,
            touches,
            escalations,
            pool,
            ..
        } = self;

        let repository = TaskRepository::new(pool);

        let seqs = if events.is_empty() {
            Vec::new()
        } else {
            let seqs = repository
                .append_task_events(Locked(&mut tx), project_id, &events)
                .await?;

            for (task_id, session_id) in &touches {
                repository
                    .touch_task_session(Locked(&mut tx), *task_id, *session_id)
                    .await?;
            }

            seqs
        };

        tx.commit().await?;

        debug!(
            project_id = %project_id,
            events = seqs.len(),
            touches = touches.len(),
            "tracker mutation committed",
        );

        Ok(MutationOutcome { seqs, escalations })
    }

    /// Nothing changed: roll back and say so.
    ///
    /// The explicit form of dropping the mutation, for the no-op paths
    /// `SPEC.md`, "MCP tool contracts" reserves — an update that sets a field
    /// to the value it already has, a release of a lease nobody holds. The
    /// intent is then visible at the call site rather than implied by a
    /// missing `commit`.
    pub async fn no_change(self) -> Result<()> {
        self.tx.rollback().await?;

        debug!(project_id = %self.project_id, "tracker mutation changed nothing");

        Ok(())
    }
}
