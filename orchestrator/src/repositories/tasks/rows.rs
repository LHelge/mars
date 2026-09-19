//! The `tasks` row itself: insertion with its number and default state,
//! lookup by either of the two references a caller may use, the field update,
//! the generic state write and the board reads.
//!
//! `docs/data-model.md`, `tasks` and `SPEC.md`, "Tasks". Three of its rules
//! have no constraint behind them and therefore live here, under the project
//! lock:
//!
//! - the per-project `number` comes from `projects.next_task_number`, in the
//!   same transaction as the insert, and is never reused;
//! - a task with no state given lands in the project's default queue state;
//! - nesting is one level deep (`ARCHITECTURE.md`, "Task tracker",
//!   "Parents"): the parent is a different task of the same project with no
//!   parent of its own, and the child has no children.
//!
//! What is deliberately *not* here: the events. A row change owes the board a
//! `TaskEvent`, and none of these helpers writes one — the caller composes the
//! batch and passes it to
//! [`TaskRepository::append_task_events`](super::TaskRepository::append_task_events)
//! in the same transaction (ADR 0028). [`TaskRepository::update_task`]'s
//! `Option` return is what lets the caller obey the other half of that rule:
//! an update with no effective change writes no event (ADR 0030).
//!
//! `blocked`, the lease and `closed_at` are not fields anybody sets one at a
//! time either. They move together as a state change composes them, so they
//! share one statement, [`TaskRepository::set_task_state_fields`], which is
//! `pub(crate)` and which `tracker/` builds releases, hand-offs and
//! escalations out of.
//!
//! The claim is the exception that proves that rule. It is not a composition
//! of fields but the one atomic statement `docs/data-model.md` prints in full,
//! and it is a statement precisely because its conditions and its write have to
//! be indivisible: [`TaskRepository::claim`] is that statement, and
//! [`TaskRepository::list_claimable`] is the read of the same predicate that
//! the `ready` tool offers before anyone races for one.

use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::models::{NewTask, Task, TaskError, TaskRef, TaskStateKind, TaskUpdate};
use crate::prelude::*;
use crate::repositories::ProjectRepository;
use crate::repositories::tasks::TaskRepository;
use crate::tracker::Locked;

/// The parent named by the caller is not a task of this project, or is not
/// there at all.
const PARENT_NOT_TOP_LEVEL: &str = "parent must be a top-level task of the same project";
/// The task being re-parented is itself an epic.
const CHILD_HAS_CHILDREN: &str = "a task with children cannot get a parent";
/// The proposed parent is itself a child.
const PARENT_IS_NESTED: &str = "a task with a parent cannot receive children";

/// Which of the project's tasks a board read wants (`SPEC.md`, "Tasks":
/// `?state=&label=&priority=&parent=&held=`).
///
/// Every field is a filter that is only applied when it is `Some`, so the
/// default value is "the whole project". The route translates its query string
/// into this — `state` by name through
/// [`TaskRepository::find_state_by_name`](super::TaskRepository::find_state_by_name),
/// since the rows reference states by id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFilter {
    /// Only tasks in this state.
    pub state_id: Option<Uuid>,
    /// Only tasks carrying this label. An unknown label matches nothing
    /// rather than failing: a filter is a question, not a validated field.
    pub label: Option<String>,
    /// Only tasks at this priority, `0` critical to `3` low.
    pub priority: Option<i16>,
    /// Only the children of this task.
    pub parent_id: Option<Uuid>,
    /// `Some(true)`: only tasks somebody holds. `Some(false)`: only free ones.
    pub held: Option<bool>,
}

/// The columns a state change, a claim, a release or an escalation moves.
///
/// `pub(crate)` and referenced only from `tracker/`: composing these — which
/// combination each operation writes — is the tracker's job, so nothing
/// outside it can write a half-composed state.
///
/// One statement rather than one per field, because they never move alone:
/// entering a terminal state sets `closed_at` and clears the lease, a claim
/// sets the lease and raises `attempts`, an escalation moves the state and
/// writes `needs_human_reason` (`docs/data-model.md`, `tasks`). Composing them
/// — which combination each operation writes — is the tracker epic's; this is
/// the one write underneath.
///
/// `None` leaves a column alone. The nested `Option`s distinguish that from
/// "set it to NULL", which is what a release, a reopen and a resolved
/// escalation all need to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateFields {
    /// The state the task moves to; it must belong to the same project.
    pub state_id: Option<Uuid>,
    /// The holder and the time it claimed. `Some(None)` releases.
    ///
    /// One field for two columns on purpose: `CHECK
    /// ((lease_holder_session_id IS NULL) = (lease_since IS NULL))` makes a
    /// half-set lease unrepresentable, so it is unrepresentable here too.
    pub lease: Option<Option<(Uuid, DateTime<Utc>)>>,
    /// Claims since the task last changed state; a state change resets it.
    pub attempts: Option<i16>,
    /// Set on entering a terminal state, cleared on leaving one.
    pub closed_at: Option<Option<DateTime<Utc>>>,
    /// Why the task was escalated; cleared when it leaves the human state.
    pub needs_human_reason: Option<Option<String>>,
    /// The current hand-off, which must belong to this task.
    pub current_handoff_id: Option<Option<Uuid>>,
    /// The recomputed blocked flag.
    pub blocked: Option<bool>,
}

/// One row of the claimable list, before the tracker turns it into a
/// `TaskSummary` (`SPEC.md`, "MCP tool contracts" → `ready`).
///
/// The columns the summary is made of, and nothing else: no lease, no
/// timestamps, no dependency list — `ready` is a menu, not a detail read. The
/// `description` is the whole stored text; shortening it to
/// `description_excerpt` is the tracker's rule, applied once in
/// `tracker::leases`, because it is a contract about what agents see rather
/// than about what the table holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSummaryRow {
    pub id: Uuid,
    pub number: i32,
    pub title: String,
    pub description: String,
    /// The state's name, from the join — the board's columns are named states.
    pub state: String,
    pub priority: i16,
    pub labels: Vec<String>,
    pub attempts: i16,
    /// Outgoing `task_dependencies` edges of every kind.
    pub depends_on_count: i64,
}

impl TaskRepository<'_> {
    /// Insert a task, allocating its number and resolving its state.
    ///
    /// Everything the insert needs is a fact about the project as it is right
    /// now: the next number, the default state and whether the parent is a
    /// top-level task of this project. Without the lock two inserts could take
    /// the same number and lose one to `UNIQUE (project_id, number)`, and a
    /// parent could gain a parent of its own between the check and the insert.
    ///
    /// `project_id` is the project whose row the caller locked; a `task` built
    /// for a different one is [`Error::BadRequest`], because the lock held is
    /// then not the lock that serialises the insert.
    ///
    /// A `state_id` the caller supplies must be a state of this project
    /// ([`Error::BadRequest`]); `None` resolves to
    /// [`TaskRepository::default_state`], the queue state with the lowest
    /// position. A project with no queue state at all cannot take a new task
    /// and is [`Error::Conflict`] with `project has no queue state` — a
    /// defensive answer, since
    /// [`TaskRepository::delete_state`](super::TaskRepository::delete_state)
    /// refuses to remove the last one.
    ///
    /// The parent, when there is one, must be a task of this project with no
    /// parent of its own (`ARCHITECTURE.md`, "Task tracker", "Parents"). The
    /// other half of that rule — the child has no children — is true by
    /// construction here: the row does not exist yet, so nothing can point at
    /// it. [`TaskRepository::update_task`] is where it has to be checked.
    pub async fn insert_task(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        task: &NewTask,
    ) -> Result<Task> {
        task.validate()?;

        if task.project_id != project_id {
            return Err(Error::BadRequest(
                "a task is inserted into the project whose lock is held".into(),
            ));
        }

        let state_id = match task.state_id {
            Some(state_id) => {
                if !state_in_project(&mut tx, project_id, state_id).await? {
                    return Err(Error::BadRequest(
                        "state must belong to this project".into(),
                    ));
                }
                state_id
            }
            None => {
                self.default_state(tx.reborrow(), project_id)
                    .await
                    // The only `NotFound` this read can produce is "no queue
                    // state"; the project itself was proved to exist by the
                    // lock this runs under.
                    .map_err(|err| match err {
                        Error::NotFound => Error::Conflict("project has no queue state".into()),
                        other => other,
                    })?
                    .id
            }
        };

        if let Some(parent_id) = task.parent_id
            && parent_of(&mut tx, project_id, parent_id).await?.is_some()
        {
            return Err(Error::BadRequest(PARENT_NOT_TOP_LEVEL.into()));
        }

        let number = ProjectRepository::new(self.pool)
            .allocate_task_number(&mut tx, project_id)
            .await?;

        let labels = task.label_strings();

        let inserted = sqlx::query_as!(
            Task,
            r#"
            INSERT INTO tasks (
                id, project_id, number, title, description, state_id, priority, labels,
                parent_id, assignee_user_id, created_by_user_id, created_by_session_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING id, project_id, number, title, description, state_id, priority, blocked,
                      labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since,
                      attempts, needs_human_reason, current_handoff_id, created_by_user_id,
                      created_by_session_id, created_at, updated_at, closed_at
            "#,
            task.id,
            project_id,
            number,
            task.title.as_str(),
            task.description,
            state_id,
            task.priority.get(),
            &labels[..],
            task.parent_id,
            task.assignee_user_id,
            task.created_by_user_id,
            task.created_by_session_id,
        )
        .fetch_one(&mut *tx)
        .await?;

        // The number and the ids, never the title: a title is user content and
        // belongs in the event payload, not in the log.
        debug!(project_id = %project_id, task_id = %inserted.id, number, "task inserted");

        Ok(inserted)
    }

    /// The task this reference names in this project, or `None`.
    ///
    /// A [`TaskRef::Number`] that no task carries is `None`, exactly like an
    /// unknown UUID: the two forms are interchangeable everywhere `SPEC.md`
    /// accepts them, so they answer the same way.
    ///
    /// Read on the pool. A caller that is about to change the task wants
    /// [`TaskRepository::find_task_for_update`] instead.
    pub async fn find_task(&self, project_id: Uuid, task_ref: TaskRef) -> Result<Option<Task>> {
        let task = match task_ref {
            TaskRef::Id(id) => {
                sqlx::query_as!(
                    Task,
                    r#"
                    SELECT id, project_id, number, title, description, state_id, priority, blocked,
                           labels, parent_id, assignee_user_id, lease_holder_session_id,
                           lease_since, attempts, needs_human_reason, current_handoff_id,
                           created_by_user_id, created_by_session_id, created_at, updated_at,
                           closed_at
                    FROM tasks
                    WHERE id = $1 AND project_id = $2
                    "#,
                    id,
                    project_id,
                )
                .fetch_optional(self.pool)
                .await?
            }
            TaskRef::Number(number) => {
                sqlx::query_as!(
                    Task,
                    r#"
                    SELECT id, project_id, number, title, description, state_id, priority, blocked,
                           labels, parent_id, assignee_user_id, lease_holder_session_id,
                           lease_since, attempts, needs_human_reason, current_handoff_id,
                           created_by_user_id, created_by_session_id, created_at, updated_at,
                           closed_at
                    FROM tasks
                    WHERE project_id = $1 AND number = $2
                    "#,
                    project_id,
                    number,
                )
                .fetch_optional(self.pool)
                .await?
            }
        };

        Ok(task)
    }

    /// [`TaskRepository::find_task`] with the task row locked as well.
    ///
    /// The project lock already serialises the project's writers, so the row
    /// lock adds nothing against them; what it adds is a stable row for the
    /// reads that follow within this transaction and a wait for any writer
    /// that reached this row without the project lock. Every mutation that
    /// compares "what is stored" against "what was asked for" — which is what
    /// [`TaskRepository::update_task`] does to decide whether anything
    /// changed — starts here.
    pub async fn find_task_for_update(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        task_ref: TaskRef,
    ) -> Result<Option<Task>> {
        let task = match task_ref {
            TaskRef::Id(id) => {
                sqlx::query_as!(
                    Task,
                    r#"
                    SELECT id, project_id, number, title, description, state_id, priority, blocked,
                           labels, parent_id, assignee_user_id, lease_holder_session_id,
                           lease_since, attempts, needs_human_reason, current_handoff_id,
                           created_by_user_id, created_by_session_id, created_at, updated_at,
                           closed_at
                    FROM tasks
                    WHERE id = $1 AND project_id = $2
                    FOR UPDATE
                    "#,
                    id,
                    project_id,
                )
                .fetch_optional(&mut *tx)
                .await?
            }
            TaskRef::Number(number) => {
                sqlx::query_as!(
                    Task,
                    r#"
                    SELECT id, project_id, number, title, description, state_id, priority, blocked,
                           labels, parent_id, assignee_user_id, lease_holder_session_id,
                           lease_since, attempts, needs_human_reason, current_handoff_id,
                           created_by_user_id, created_by_session_id, created_at, updated_at,
                           closed_at
                    FROM tasks
                    WHERE project_id = $1 AND number = $2
                    FOR UPDATE
                    "#,
                    project_id,
                    number,
                )
                .fetch_optional(&mut *tx)
                .await?
            }
        };

        Ok(task)
    }

    /// The task with this UUID, whatever project it belongs to.
    ///
    /// The one deliberately unscoped task read. `POST
    /// /projects/{pid}/tasks/{id}/dependencies` answers a `depends_on` UUID
    /// that names a task of *another* project with 400 `dependency must
    /// reference tasks of the same project`, and one that names no task at all
    /// with 404 (`SPEC.md`, "Tasks") — a difference only a lookup outside the
    /// project scope can make. Nothing else uses it: every other read puts the
    /// project in its `WHERE` clause, as the conventions require.
    ///
    /// Read under the caller's token but deliberately **without** `FOR
    /// UPDATE`. The row may belong to another project, and locking it from
    /// inside this project's lock is how two projects deadlock against each
    /// other; the caller only needs to know that the row exists, and is about
    /// to refuse the request because of it.
    pub async fn find_task_in_any_project_in(
        &self,
        mut tx: Locked<'_>,
        id: Uuid,
    ) -> Result<Option<Task>> {
        let task = sqlx::query_as!(
            Task,
            r#"
            SELECT id, project_id, number, title, description, state_id, priority, blocked,
                   labels, parent_id, assignee_user_id, lease_holder_session_id,
                   lease_since, attempts, needs_human_reason, current_handoff_id,
                   created_by_user_id, created_by_session_id, created_at, updated_at,
                   closed_at
            FROM tasks
            WHERE id = $1
            "#,
            id,
        )
        .fetch_optional(&mut *tx)
        .await?;

        Ok(task)
    }

    /// Apply the fields a `PUT` supplied, and say whether anything moved.
    ///
    /// The return distinguishes the three outcomes ADR 0030 cares about. An
    /// unknown task is [`Error::NotFound`]. A task that exists but whose
    /// stored values already equal every supplied value is `Ok(None)`: nothing
    /// is written, `updated_at` does not move, and the caller writes no
    /// `updated` event and creates no session link. Anything else is
    /// `Ok(Some(row))` with the stored row as it now is.
    ///
    /// "Nothing changed" is decided against the row read under `FOR UPDATE`
    /// here, not against the number of rows the statement touched: an `UPDATE`
    /// that sets a column to the value it already has still reports one row,
    /// and would bump `updated_at` and produce an event for a request that
    /// changed nothing.
    ///
    /// Re-parenting re-runs the parent rules, and re-runs them only when the
    /// parent actually changes — asking for the parent a task already has is a
    /// no-op, not a rule to enforce. Giving a task a parent requires that it
    /// has no children of its own ([`Error::BadRequest`], `a task with
    /// children cannot get a parent`) and that the proposed parent has no
    /// parent ([`Error::BadRequest`], `a task with a parent cannot receive
    /// children`); a parent that is not a task of this project, or is the task
    /// itself, is rejected as it is on insert.
    pub async fn update_task(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        id: Uuid,
        update: &TaskUpdate,
    ) -> Result<Option<Task>> {
        let current = self
            .find_task_for_update(tx.reborrow(), project_id, TaskRef::Id(id))
            .await?
            .ok_or(Error::NotFound)?;

        let title = update
            .title
            .as_ref()
            .map(|title| title.as_str())
            .filter(|title| *title != current.title);
        let description = update
            .description
            .as_deref()
            .filter(|description| *description != current.description);
        let priority = update
            .priority
            .map(|priority| priority.get())
            .filter(|priority| *priority != current.priority);
        let labels = update
            .label_strings()
            .filter(|labels| *labels != current.labels);
        let assignee_user_id = update
            .assignee_user_id
            .filter(|assignee| *assignee != current.assignee_user_id);
        let parent_id = update
            .parent_id
            .filter(|parent_id| *parent_id != current.parent_id);

        if title.is_none()
            && description.is_none()
            && priority.is_none()
            && labels.is_none()
            && assignee_user_id.is_none()
            && parent_id.is_none()
        {
            return Ok(None);
        }

        if let Some(Some(new_parent_id)) = parent_id {
            if new_parent_id == id {
                return Err(TaskError::SelfParent.into());
            }
            if parent_of(&mut tx, project_id, new_parent_id)
                .await?
                .is_some()
            {
                return Err(Error::BadRequest(PARENT_IS_NESTED.into()));
            }
            if has_children(&mut tx, id).await? {
                return Err(Error::BadRequest(CHILD_HAS_CHILDREN.into()));
            }
        }

        let updated = sqlx::query_as!(
            Task,
            r#"
            UPDATE tasks
            SET title = COALESCE($3, title),
                description = COALESCE($4, description),
                priority = COALESCE($5, priority),
                labels = COALESCE($6, labels),
                assignee_user_id = CASE WHEN $7 THEN $8 ELSE assignee_user_id END,
                parent_id = CASE WHEN $9 THEN $10 ELSE parent_id END,
                updated_at = NOW()
            WHERE id = $1 AND project_id = $2
            RETURNING id, project_id, number, title, description, state_id, priority, blocked,
                      labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since,
                      attempts, needs_human_reason, current_handoff_id, created_by_user_id,
                      created_by_session_id, created_at, updated_at, closed_at
            "#,
            id,
            project_id,
            title as Option<&str>,
            description as Option<&str>,
            priority as Option<i16>,
            labels.as_deref() as Option<&[String]>,
            assignee_user_id.is_some(),
            assignee_user_id.flatten(),
            parent_id.is_some(),
            parent_id.flatten(),
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;

        debug!(project_id = %project_id, task_id = %id, "task updated");

        Ok(Some(updated))
    }

    /// Write the state, lease, attempt, closure, escalation, hand-off and
    /// blocked columns in one statement.
    ///
    /// Composed in `tracker/` and nowhere else, which is why this and
    /// [`StateFields`] are `pub(crate)`: the coupling between the state's
    /// kind, the lease, `attempts` and `closed_at` is decided once, in the
    /// tracker's `change_state`, together with the events the change owes the
    /// board and, where the actor is a session, its `touch_task_session` link.
    ///
    /// This helper decides nothing. Whether a release escalates, whether a
    /// state move resets `attempts`, whether a terminal state closes the task
    /// — those are the tracker epic's compositions, and each of them ends up
    /// here with a different [`StateFields`]. What this does enforce is scope:
    /// the state must belong to this project and the hand-off to this task,
    /// both [`Error::BadRequest`], because neither is a constraint the schema
    /// can carry (`docs/data-model.md`, `tasks`).
    ///
    /// An unknown task is [`Error::NotFound`]. Unlike
    /// [`TaskRepository::update_task`] there is no "nothing changed" answer:
    /// the caller composing a state move already knows what it is changing,
    /// and a claim that writes the same lease twice is a bug in the caller,
    /// not a no-op to absorb.
    pub(crate) async fn set_task_state_fields(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        id: Uuid,
        fields: &StateFields,
    ) -> Result<Task> {
        if let Some(state_id) = fields.state_id
            && !state_in_project(&mut tx, project_id, state_id).await?
        {
            return Err(Error::BadRequest(
                "state must belong to this project".into(),
            ));
        }

        if let Some(Some(handoff_id)) = fields.current_handoff_id
            && !handoff_on_task(&mut tx, id, handoff_id).await?
        {
            return Err(Error::BadRequest(
                "hand-off must belong to this task".into(),
            ));
        }

        let (lease_holder, lease_since) = match fields.lease {
            Some(Some((holder, since))) => (Some(holder), Some(since)),
            _ => (None, None),
        };

        let updated = sqlx::query_as!(
            Task,
            r#"
            UPDATE tasks
            SET state_id = COALESCE($3, state_id),
                lease_holder_session_id = CASE WHEN $4 THEN $5 ELSE lease_holder_session_id END,
                lease_since = CASE WHEN $4 THEN $6 ELSE lease_since END,
                attempts = COALESCE($7, attempts),
                closed_at = CASE WHEN $8 THEN $9 ELSE closed_at END,
                needs_human_reason = CASE WHEN $10 THEN $11 ELSE needs_human_reason END,
                current_handoff_id = CASE WHEN $12 THEN $13 ELSE current_handoff_id END,
                blocked = COALESCE($14, blocked),
                updated_at = NOW()
            WHERE id = $1 AND project_id = $2
            RETURNING id, project_id, number, title, description, state_id, priority, blocked,
                      labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since,
                      attempts, needs_human_reason, current_handoff_id, created_by_user_id,
                      created_by_session_id, created_at, updated_at, closed_at
            "#,
            id,
            project_id,
            fields.state_id,
            fields.lease.is_some(),
            lease_holder,
            lease_since,
            fields.attempts,
            fields.closed_at.is_some(),
            fields.closed_at.flatten(),
            fields.needs_human_reason.is_some(),
            fields.needs_human_reason.clone().flatten(),
            fields.current_handoff_id.is_some(),
            fields.current_handoff_id.flatten(),
            fields.blocked,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;

        debug!(project_id = %project_id, task_id = %id, "task state fields written");

        Ok(updated)
    }

    /// Take the lease on a task, if it is still there to take.
    ///
    /// The statement is `docs/data-model.md`, `tasks`, word for word: the four
    /// columns it writes and the five conditions it writes them under. Every
    /// rule a claim has is in that `WHERE` clause — the task is this project's,
    /// it is in one of the states the caller is allowed to claim from, it is
    /// not blocked and nobody holds it — so there is no check in Rust to get
    /// out of step with it, and `Ok(None)` (zero rows) is the single answer to
    /// all of them: the claim lost, and the caller owes a conflict.
    ///
    /// `state_ids` carries the policy rather than the kind of the state: the
    /// profile's served states for an agent's `claim`, every non-terminal state
    /// for a launch from the UI (`ARCHITECTURE.md`, "Task tracker" → "The lease
    /// is the worker" and "Launching a session for a task"). An empty slice
    /// matches nothing, which is exactly right for a profile that serves
    /// nothing.
    ///
    /// `attempts` is incremented here and nowhere else; a state change resets
    /// it ("Attempts and escalation"). The project lock the [`Locked`] token
    /// proves serialises this project's claims, and the `lease_holder_session_id
    /// IS NULL` condition keeps the statement correct even for a caller that
    /// reached it without one.
    pub async fn claim(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        task_id: Uuid,
        session_id: Uuid,
        state_ids: &[Uuid],
    ) -> Result<Option<Task>> {
        let claimed = sqlx::query_as!(
            Task,
            r#"
            UPDATE tasks
            SET lease_holder_session_id = $2,
                lease_since = NOW(),
                attempts = attempts + 1,
                updated_at = NOW()
            WHERE id = $1
              AND project_id = $3
              AND state_id = ANY($4)
              AND NOT blocked
              AND lease_holder_session_id IS NULL
            RETURNING id, project_id, number, title, description, state_id, priority, blocked,
                      labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since,
                      attempts, needs_human_reason, current_handoff_id, created_by_user_id,
                      created_by_session_id, created_at, updated_at, closed_at
            "#,
            task_id,
            session_id,
            project_id,
            state_ids,
        )
        .fetch_optional(&mut *tx)
        .await?;

        match &claimed {
            Some(task) => debug!(
                project_id = %project_id,
                task_id = %task_id,
                session_id = %session_id,
                attempts = task.attempts,
                "task claimed",
            ),
            None => debug!(
                project_id = %project_id,
                task_id = %task_id,
                session_id = %session_id,
                "claim matched no row",
            ),
        }

        Ok(claimed)
    }

    /// The tasks a caller serving `state_ids` could claim right now.
    ///
    /// The read behind the MCP `ready` tool (`SPEC.md`, "MCP tool contracts"):
    /// "tasks in the calling profile's served states that are not blocked and
    /// have no lease holder, ordered by `priority` then `number`". The
    /// predicate is `tasks_claimable_idx`'s own — `WHERE NOT blocked AND
    /// lease_holder_session_id IS NULL`, keyed `(project_id, state_id,
    /// priority, number)` — so the list walks the partial index rather than the
    /// table (`docs/data-model.md`, `tasks`).
    ///
    /// One query, not one per task: the state's name comes from the join and
    /// `depends_on_count` from a lateral aggregate over `task_dependencies`,
    /// counting outgoing edges **of every kind**, which is what `TaskSummary`
    /// documents. An empty `state_ids` matches nothing, so a profile that
    /// serves no states gets an empty list without a special case.
    ///
    /// Read on the pool and it writes nothing: `ready` takes no lock, emits no
    /// event and creates no session link (ADR 0021, ADR 0030). The caller
    /// validates `limit`; the statement applies it as given.
    pub async fn list_claimable(
        &self,
        project_id: Uuid,
        state_ids: &[Uuid],
        limit: i64,
    ) -> Result<Vec<TaskSummaryRow>> {
        let rows = sqlx::query_as!(
            TaskSummaryRow,
            r#"
            SELECT t.id, t.number, t.title, t.description, s.name AS state, t.priority, t.labels,
                   t.attempts, d.depends_on_count AS "depends_on_count!"
            FROM tasks AS t
            JOIN task_states AS s ON s.id = t.state_id
            CROSS JOIN LATERAL (
                SELECT COUNT(*) AS depends_on_count
                FROM task_dependencies AS e
                WHERE e.task_id = t.id
            ) AS d
            WHERE t.project_id = $1
              AND t.state_id = ANY($2)
              AND NOT t.blocked
              AND t.lease_holder_session_id IS NULL
            ORDER BY t.priority, t.number
            LIMIT $3
            "#,
            project_id,
            state_ids,
            limit,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(rows)
    }

    /// The project's tasks matching `filter`, ordered by priority then number.
    ///
    /// `GET /projects/{pid}/tasks` (`SPEC.md`, "Tasks"): "ordered by priority,
    /// then number", which is also the order `tasks_project_state_idx`
    /// carries, so a state-filtered board read walks the index.
    ///
    /// Read on the pool: a board read takes no part in anyone's mutation and
    /// wants the committed state, not a lock.
    pub async fn list_tasks(&self, project_id: Uuid, filter: &TaskFilter) -> Result<Vec<Task>> {
        let tasks = sqlx::query_as!(
            Task,
            r#"
            SELECT id, project_id, number, title, description, state_id, priority, blocked,
                   labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since,
                   attempts, needs_human_reason, current_handoff_id, created_by_user_id,
                   created_by_session_id, created_at, updated_at, closed_at
            FROM tasks
            WHERE project_id = $1
              AND ($2::uuid IS NULL OR state_id = $2)
              AND ($3::text IS NULL OR $3 = ANY (labels))
              AND ($4::smallint IS NULL OR priority = $4)
              AND ($5::uuid IS NULL OR parent_id = $5)
              AND ($6::boolean IS NULL OR (lease_holder_session_id IS NOT NULL) = $6)
            ORDER BY priority, number
            "#,
            project_id,
            filter.state_id,
            filter.label.as_deref(),
            filter.priority,
            filter.parent_id,
            filter.held,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(tasks)
    }

    /// Every task in a state of this kind, across every project.
    ///
    /// The dashboard's `GET /tasks?state_kind=human` (`SPEC.md`, "Tasks"): the
    /// one tracker read that is not project-scoped, because the question it
    /// answers — what is waiting for a person — spans the projects a user
    /// works. Ordered by priority first for that reason, with the project and
    /// number breaking ties into a stable order.
    pub async fn list_tasks_by_state_kind(&self, kind: TaskStateKind) -> Result<Vec<Task>> {
        let tasks = sqlx::query_as!(
            Task,
            r#"
            SELECT t.id, t.project_id, t.number, t.title, t.description, t.state_id, t.priority,
                   t.blocked, t.labels, t.parent_id, t.assignee_user_id,
                   t.lease_holder_session_id, t.lease_since, t.attempts, t.needs_human_reason,
                   t.current_handoff_id, t.created_by_user_id, t.created_by_session_id,
                   t.created_at, t.updated_at, t.closed_at
            FROM tasks AS t
            JOIN task_states AS s ON s.id = t.state_id
            WHERE s.kind = $1
            ORDER BY t.priority, t.project_id, t.number
            "#,
            kind as TaskStateKind,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(tasks)
    }

    /// An epic's children, in the same order as the board.
    ///
    /// `TaskDetail.children` (`SPEC.md`, "Tasks"), and the list a parent's
    /// `blocked` recomputation and its automatic closure read.
    pub async fn list_children(&self, project_id: Uuid, parent_id: Uuid) -> Result<Vec<Task>> {
        let children = sqlx::query_as!(
            Task,
            r#"
            SELECT id, project_id, number, title, description, state_id, priority, blocked,
                   labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since,
                   attempts, needs_human_reason, current_handoff_id, created_by_user_id,
                   created_by_session_id, created_at, updated_at, closed_at
            FROM tasks
            WHERE project_id = $1 AND parent_id = $2
            ORDER BY priority, number
            "#,
            project_id,
            parent_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(children)
    }

    /// Every task this session currently holds.
    ///
    /// The stuck-task reaper walks held tasks by their holder, and a session
    /// ending releases what it still holds (`ARCHITECTURE.md`, "Task
    /// tracker"); `tasks_lease_holder_idx` is the partial index that serves
    /// both. Not project-scoped, because a session belongs to exactly one
    /// project and the holder already names it.
    pub async fn list_by_lease_holder(&self, session_id: Uuid) -> Result<Vec<Task>> {
        let tasks = sqlx::query_as!(
            Task,
            r#"
            SELECT id, project_id, number, title, description, state_id, priority, blocked,
                   labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since,
                   attempts, needs_human_reason, current_handoff_id, created_by_user_id,
                   created_by_session_id, created_at, updated_at, closed_at
            FROM tasks
            WHERE lease_holder_session_id = $1
            ORDER BY priority, number
            "#,
            session_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(tasks)
    }

    /// The same list, read inside a tracker mutation and scoped to its
    /// project.
    ///
    /// Discovery provenance is decided from what the calling session holds
    /// *at the moment of the creation*, under the project lock, before any row
    /// is inserted (`ARCHITECTURE.md`, "Task tracker" → "Discovery
    /// provenance"), so it cannot read a list taken on the pool: a release
    /// committed between that read and the lock would make the inference name
    /// a task the session no longer holds. The `project_id` filter is
    /// redundant for a well-formed session — a session belongs to exactly one
    /// project — and is in the `WHERE` clause anyway, so a holder from
    /// elsewhere can never widen a mutation's scope.
    pub async fn list_by_lease_holder_in(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        session_id: Uuid,
    ) -> Result<Vec<Task>> {
        let tasks = sqlx::query_as!(
            Task,
            r#"
            SELECT id, project_id, number, title, description, state_id, priority, blocked,
                   labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since,
                   attempts, needs_human_reason, current_handoff_id, created_by_user_id,
                   created_by_session_id, created_at, updated_at, closed_at
            FROM tasks
            WHERE lease_holder_session_id = $1 AND project_id = $2
            ORDER BY priority, number
            "#,
            session_id,
            project_id,
        )
        .fetch_all(&mut *tx)
        .await?;

        Ok(tasks)
    }

    /// Delete a task; `false` when this project has no such task.
    ///
    /// **Read what the deletion will destroy before calling.** The cascades
    /// take the task's dependency edges, comments, hand-offs and session links
    /// with it, and set its children's `parent_id` to NULL. Afterwards there
    /// is no way to find out who was waiting on it, so the caller captures the
    /// surviving dependants and children *first*, deletes, then recomputes
    /// their `blocked` flags from the remaining edges and children and emits
    /// `dependency_removed`, `blocked`/`unblocked` and the `deleted` event in
    /// this same transaction (`docs/data-model.md`, `tasks`).
    ///
    /// The `deleted` event keeps the original UUID; history is never
    /// rewritten, and `task_events` has no foreign key to `tasks` precisely so
    /// that it survives this (ADR 0022).
    pub async fn delete_task(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        id: Uuid,
    ) -> Result<bool> {
        let deleted = sqlx::query!(
            "DELETE FROM tasks WHERE id = $1 AND project_id = $2",
            id,
            project_id,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;

        if deleted {
            debug!(project_id = %project_id, task_id = %id, "task deleted");
        }

        Ok(deleted)
    }
}

/// Does this state belong to this project?
///
/// `tasks.state_id`'s foreign key proves the state exists; only this proves it
/// is one of *this* project's columns. Without it a caller holding one
/// project's lock could park a task in another project's state, and the board
/// would never show it again.
async fn state_in_project(tx: &mut PgConnection, project_id: Uuid, state_id: Uuid) -> Result<bool> {
    let exists = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM task_states WHERE id = $1 AND project_id = $2
        ) AS "exists!"
        "#,
        state_id,
        project_id,
    )
    .fetch_one(&mut *tx)
    .await?;

    Ok(exists)
}

/// Is this hand-off a record of this task?
///
/// `tasks.current_handoff_id` "must belong to this task (repository check)"
/// (`docs/data-model.md`, `tasks`); the foreign key only says the row exists.
async fn handoff_on_task(tx: &mut PgConnection, task_id: Uuid, handoff_id: Uuid) -> Result<bool> {
    let exists = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM task_handoffs WHERE id = $1 AND task_id = $2
        ) AS "exists!"
        "#,
        handoff_id,
        task_id,
    )
    .fetch_one(&mut *tx)
    .await?;

    Ok(exists)
}

/// The proposed parent's own parent, proving it is a task of this project on
/// the way.
///
/// `Err(`[`Error::BadRequest`]`)` when there is no such task in this project —
/// the answer both creation and re-parenting give a parent they cannot use.
/// `Ok(Some(_))` means the candidate is itself a child, which is the one-level
/// rule's other half and which the two callers word differently.
async fn parent_of(
    tx: &mut PgConnection,
    project_id: Uuid,
    parent_id: Uuid,
) -> Result<Option<Uuid>> {
    let parent = sqlx::query!(
        "SELECT parent_id FROM tasks WHERE id = $1 AND project_id = $2",
        parent_id,
        project_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| Error::BadRequest(PARENT_NOT_TOP_LEVEL.into()))?;

    Ok(parent.parent_id)
}

/// Is anything already nested under this task?
///
/// Asked before giving a task a parent: nesting is one level deep, including
/// terminal children (`ARCHITECTURE.md`, "Task tracker", "Parents"), so an
/// epic can never become a child. `tasks_parent_idx` serves the read.
async fn has_children(tx: &mut PgConnection, task_id: Uuid) -> Result<bool> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM tasks WHERE parent_id = $1) AS "exists!""#,
        task_id,
    )
    .fetch_one(&mut *tx)
    .await?;

    Ok(exists)
}
