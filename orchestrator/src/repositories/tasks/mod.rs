//! All SQL against the tracker tables, and the transaction primitive every
//! tracker mutation is built on.
//!
//! `docs/data-model.md`, "Tracker mutation transactions" and `ARCHITECTURE.md`,
//! "Task tracker" put one rule above everything else in this module: **one
//! mutation at a time per project**. Every writer begins a `READ COMMITTED`
//! transaction, locks the project row, and only then reads authoritative state
//! and validates. [`TaskRepository::begin_mutation`] is that opening move, and
//! [`TrackerMutation`](crate::tracker::TrackerMutation) — its only caller — is
//! what the rest of the crate opens instead.
//!
//! **The rule, stated once: every helper here that writes a tracker table, and
//! every read that has to be authoritative, takes a
//! [`Locked`](crate::tracker::Locked) rather than a `&mut PgConnection`, and
//! only [`TrackerMutation::conn`](crate::tracker::TrackerMutation::conn) can
//! produce one.** So a tracker write outside the project lock does not
//! compile, and no helper below repeats a warning the type already carries. A
//! helper that still takes a bare connection is a lock-free read — the board
//! lists, the detail loaders, `ready` — and says so by its signature (ADR
//! 0021).
//!
//! **Lock order.** Any git lock is acquired before any database lock, then the
//! project row ([`TaskRepository::begin_mutation`]), then session rows, then
//! task rows; never the other way round. A combined tracker/session operation
//! takes its session connection from the same token, so the order holds by
//! construction.
//!
//! The isolation level is left at the Postgres default, `READ COMMITTED`: the
//! project row lock, not a stricter snapshot, is what serialises the tracker,
//! and raising the level would only add serialisation failures for callers to
//! retry.
//!
//! **Events are the caller's to compose.** None of these helpers appends the
//! `states_changed` event a state change owes the board, or the `updated`
//! events a task change owes it. A helper changes rows; the caller assembles
//! the [`NewTaskEvent`](crate::models::NewTaskEvent) batch that describes the
//! change and passes it to [`TaskRepository::append_task_events`] **in the same
//! transaction**, so the rows, their events and the `pg_notify` that announces
//! them commit or roll back together (ADR 0028; `SPEC.md`, "TaskEvent"). The
//! payload assembly itself — actor, `task`, `from`/`to`, `states` — belongs to
//! `events/` and the tracker epic, not here.
//!
//! The module is split by table: `rows` holds `tasks`, `dependencies` holds
//! `task_dependencies`, `comments` holds `task_comments`, `handoffs` holds
//! `task_handoffs`, `links` holds `task_sessions`, `states` holds
//! `task_states` and `profile_states`, and `events` holds `task_events`. They
//! are one `impl TaskRepository` between them, so a caller sees one repository
//! and the files stay the size of the table they are about.
//!
//! Two files are not about a table. `graph` holds the SQL the dependency graph
//! is made of — what `blocked` evaluates to, the write that stores it, the
//! `blocks`-only reachability the cycle check asks about, and the reads a
//! deletion takes before the cascades erase them — because each of those spans
//! `tasks`, `task_dependencies` and `task_states` at once; the compositions on
//! top of them are `tracker::graph`'s.
//!
//! The other is `dto`: the read-only loaders that
//! assemble `SPEC.md`'s `Task` and `TaskDetail` out of several of them at
//! once, without an N+1 per task.

mod comments;
mod dependencies;
mod dto;
mod events;
mod graph;
mod handoffs;
mod links;
mod rows;
mod states;
pub mod test_support;

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub use events::MAX_TASK_EVENT_PAGE;
pub use graph::BlockedState;
/// The state-column write `tracker/` composes its state moves out of. Crate-
/// private like the helper that takes it: `rows` is a private module, so this
/// re-export is how the tracker names the type at all.
pub(crate) use rows::StateFields;
pub use rows::{TaskFilter, TaskSummaryRow};

use crate::prelude::*;
use crate::repositories::ProjectRepository;
use crate::tracker::Locked;

/// All SQL against the tracker tables (`ARCHITECTURE.md`, "Orchestrator
/// internals").
///
/// Reads that need no transaction go straight to the pool; everything that
/// mutates, and everything that has to be read under the project lock to be
/// authoritative, takes the mutation's [`Locked`](crate::tracker::Locked)
/// connection, so one transaction can hold a whole tracker mutation — the
/// lock, the change, its dependency effects and its events — together, and so
/// that there is no way to spell a write that is not inside one.
pub struct TaskRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> TaskRepository<'a> {
    /// Borrow `pool` for the lifetime of this repository.
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    /// Open a tracker mutation: one transaction with the project row locked.
    ///
    /// This is the first statement of every tracker write — task, state,
    /// dependency, comment, claim, release, hand-off, profile served states,
    /// background job and deletion alike (`docs/data-model.md`, "Tracker
    /// mutation transactions"). The lock is held until the returned
    /// transaction commits or rolls back, so everything read through it
    /// afterwards is authoritative for this project, and nothing read *before*
    /// it is.
    ///
    /// An unknown project is [`Error::NotFound`], raised by
    /// [`ProjectRepository::lock_project`] before the caller can write
    /// anything: a mutation must not proceed unserialised against a project
    /// that is not there. The transaction is dropped — and therefore rolled
    /// back — on that path.
    ///
    /// Concurrent calls for the same project queue at the lock; calls for
    /// different projects are independent. An operation spanning several
    /// projects opens them in UUID order.
    ///
    /// `pub(crate)`, and called from
    /// [`TrackerMutation::begin`](crate::tracker::TrackerMutation::begin)
    /// alone: a raw transaction is not a token, and everything that needs one
    /// needs the event batch, the session links and the escalations that come
    /// with it.
    pub(crate) async fn begin_mutation(
        &self,
        project_id: Uuid,
    ) -> Result<Transaction<'a, Postgres>> {
        let mut tx = self.pool.begin().await?;
        ProjectRepository::new(self.pool)
            .lock_project(&mut tx, project_id)
            .await?;

        Ok(tx)
    }
}

/// Is there a task with this id in this project?
///
/// The scope check shared by the helpers that accept a task id alongside the
/// project they are mutating. It takes the token because without the project
/// lock the answer can change before the caller acts on it.
async fn task_in_project(mut tx: Locked<'_>, project_id: Uuid, task_id: Uuid) -> Result<bool> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM tasks WHERE id = $1 AND project_id = $2) AS "exists!""#,
        task_id,
        project_id,
    )
    .fetch_one(&mut *tx)
    .await?;

    Ok(exists)
}
