//! `task_events`: the project-scoped `TaskEvent` stream and its notification.
//!
//! `docs/data-model.md`, `task_events` is the contract, and one sentence of it
//! is the reason this file exists in the shape it does:
//!
//! > Append-only, with `seq` allocated as `MAX(seq)+1` for this project while
//! > holding the project row lock in the tracker mutation transaction. Writers
//! > never allocate outside that lock or publish an event in a separate
//! > transaction from its change.
//!
//! [`TaskRepository::append_task_events`] is that writer — the sibling of
//! `SessionRepository::append_events`, with the project row in place of the
//! session row. Every tracker writer goes through it rather than inserting its
//! own rows, so there is one implementation of the locking contract to get
//! right, and a primary-key collision is therefore not a race to retry but an
//! invariant failure: it means a writer bypassed the lock.
//!
//! The `task_sessions` upsert sits beside it, because it is the other write
//! `TrackerMutation::commit` makes and it commits with those same events (ADR
//! 0030): the two together are what a committed tracker change leaves behind
//! besides the rows it changed.
//!
//! The notification is issued with `pg_notify` on the caller's transaction,
//! with bound parameters. PostgreSQL delivers it only after that transaction
//! commits and discards it on rollback, so there is never a separate
//! post-commit write (ADR 0028; `ARCHITECTURE.md`, "Event delivery").

use sqlx::PgConnection;
use uuid::Uuid;

use crate::models::{NewTaskEvent, TaskEventRow, task_event_kind};
use crate::prelude::*;
use crate::repositories::tasks::{TaskRepository, task_in_project};
use crate::repositories::unique_violation;
use crate::tracker::{Locked, TaskSessionLinkDto};

/// The largest replay page `GET /projects/{pid}/tasks/stream?after=` will read
/// in one query.
///
/// The same bound as the session event page (`SPEC.md`, "Sessions"): a client
/// that has been away for a long time catches up in several reads rather than
/// one unbounded one, and its cursor already says where to resume.
pub const MAX_TASK_EVENT_PAGE: u32 = 500;

impl TaskRepository<'_> {
    /// Append a batch of events to the project's stream and announce it.
    ///
    /// Called from
    /// [`TrackerMutation::commit`](crate::tracker::TrackerMutation::commit)
    /// alone, with the change the events describe. The project lock, not the
    /// insert, is what makes `MAX(seq) + 1` safe: a waiting writer reads
    /// the previous writer's committed events only after it acquires the lock
    /// (`docs/data-model.md`, `task_events`). Appending outside the lock, or
    /// in a transaction of its own, breaks both the sequence and the rule that
    /// rollback exposes neither the change nor its events.
    ///
    /// Each event with a `task_id` is checked against the project first, under
    /// that lock — except a `deleted` event, whose task row is already gone by
    /// design: the event keeps the original UUID so history is never rewritten
    /// (ADR 0022). A task from another project is [`Error::NotFound`], the
    /// same answer every out-of-scope id gets in this crate. A `None` task_id
    /// is a project-wide event such as `states_changed` and is not checked.
    ///
    /// An empty batch is a no-op: no rows, and no notification promising rows
    /// that are not there.
    ///
    /// Returns the allocated sequences, in the order the events were given.
    pub(crate) async fn append_task_events(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        events: &[NewTaskEvent],
    ) -> Result<Vec<i64>> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        for event in events {
            // A `deleted` event is the one kind whose task is expected to be
            // missing; checking it would refuse exactly the event that records
            // the deletion.
            if let Some(task_id) = event.task_id
                && event.kind != task_event_kind::DELETED
                && !task_in_project(tx.reborrow(), project_id, task_id).await?
            {
                return Err(Error::NotFound);
            }
        }

        let mut sequences = Vec::with_capacity(events.len());
        for event in events {
            let seq = sqlx::query_scalar!(
                r#"
                INSERT INTO task_events (project_id, seq, ts, task_id, kind, payload)
                SELECT $1, COALESCE(MAX(seq), 0) + 1, $2, $3, $4, $5
                FROM task_events
                WHERE project_id = $1
                RETURNING seq AS "seq!"
                "#,
                project_id,
                event.ts,
                event.task_id,
                event.kind,
                event.payload,
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| map_sequence_collision(err, project_id))?;

            sequences.push(seq);
        }

        // Safe to unwrap: the slice was checked non-empty above.
        let last_seq = *sequences.last().expect("the batch has at least one event");

        // One notification per batch, carrying the highest sequence it wrote;
        // consumers read the rows themselves (ADR 0028).
        notify_task_events(&mut tx, &format!("{project_id}:{last_seq}")).await?;

        // The count and the sequence, never the payloads: a payload may carry
        // a task title or a comment body, and event payloads are never logged
        // at `info` or above (`CLAUDE.md`, rule 3).
        debug!(
            project_id = %project_id,
            count = events.len(),
            last_seq,
            "task events appended",
        );

        Ok(sequences)
    }

    /// Record that this session just changed this task.
    ///
    /// Called from
    /// [`TrackerMutation::commit`](crate::tracker::TrackerMutation::commit)
    /// alone, in the same transaction as the change and as the events above:
    /// "the link for the directly changed task commits in the same
    /// transaction as the change and its events" (`docs/data-model.md`,
    /// `task_sessions`; ADR 0030). The link is history, not configuration — it
    /// records that a session *changed* something, so reads, rejected
    /// operations and updates with no effective change create no link and
    /// advance no timestamp, which is why the caller decides when to call this
    /// and why [`TaskRepository::update_task`] answers `Ok(None)` for a no-op
    /// instead of quietly writing a link anyway.
    ///
    /// The first touch inserts; every later one updates `last_touched_at` and
    /// leaves `first_touched_at` exactly as it was. `ON CONFLICT` rather than
    /// a read-then-write because the two are one statement and cannot
    /// interleave; the project lock already keeps this project's writers
    /// apart, and the upsert keeps the statement honest for the session-scoped
    /// writes that do not hold it.
    ///
    /// Both timestamps come from `NOW()`, which is the transaction's start
    /// time, so calling this twice in one transaction is idempotent down to
    /// the timestamp — a hand-off that comments, moves and links in one go
    /// records one touch, not three.
    ///
    /// The link comes back as the DTO the detail sends
    /// ([`TaskSessionLinkDto`]); the `task_id` the caller passed in is the one
    /// column it leaves out.
    pub(crate) async fn touch_task_session(
        &self,
        mut tx: Locked<'_>,
        task_id: Uuid,
        session_id: Uuid,
    ) -> Result<TaskSessionLinkDto> {
        let link = sqlx::query_as!(
            TaskSessionLinkDto,
            r#"
            INSERT INTO task_sessions (task_id, session_id)
            VALUES ($1, $2)
            ON CONFLICT (task_id, session_id) DO UPDATE SET last_touched_at = NOW()
            RETURNING session_id, first_touched_at, last_touched_at
            "#,
            task_id,
            session_id,
        )
        .fetch_one(&mut *tx)
        .await?;

        debug!(task_id = %task_id, session_id = %session_id, "task session link touched");

        Ok(link)
    }

    /// Every event after `after`, oldest first, at most `limit` of them.
    ///
    /// The replay half of `GET /projects/{pid}/tasks/stream?after=` and of a
    /// `Last-Event-ID` reconnect: the handler subscribes to the notification
    /// fan-out, replays from its cursor, then follows notifications
    /// (`ARCHITECTURE.md`, "Event delivery"). `limit` is clamped to
    /// `1..=`[`MAX_TASK_EVENT_PAGE`]; a client that is further behind than one
    /// page reads again from the last `seq` it received.
    ///
    /// Read on the pool: replay is a catch-up read of committed rows and takes
    /// no part in anyone's mutation.
    pub async fn list_task_events_after(
        &self,
        project_id: Uuid,
        after: i64,
        limit: u32,
    ) -> Result<Vec<TaskEventRow>> {
        let limit = limit.clamp(1, MAX_TASK_EVENT_PAGE);

        let rows = sqlx::query_as!(
            TaskEventRow,
            r#"
            SELECT project_id, seq, ts, task_id, kind, payload
            FROM task_events
            WHERE project_id = $1 AND seq > $2
            ORDER BY seq
            LIMIT $3
            "#,
            project_id,
            after,
            i64::from(limit),
        )
        .fetch_all(self.pool)
        .await?;

        Ok(rows)
    }

    /// The highest committed sequence of a project's stream, or `0` when it
    /// has no events yet.
    ///
    /// The cursor a stream starts from when the client asks for `?after=latest`
    /// — a board with no cursor of its own (`SPEC.md`, "SSE: task stream"), for
    /// which the whole history is a replay nobody reads. Read on the
    /// pool, so the answer is a snapshot: a writer needs no such read, because
    /// the insert derives the next sequence from the table itself under the
    /// project lock.
    pub async fn max_task_event_seq(&self, project_id: Uuid) -> Result<i64> {
        let max = sqlx::query_scalar!(
            r#"SELECT COALESCE(MAX(seq), 0) AS "max!" FROM task_events WHERE project_id = $1"#,
            project_id,
        )
        .fetch_one(self.pool)
        .await?;

        Ok(max)
    }
}

/// Announce a committed event batch on `task_events`, payload
/// `<project_id>:<seq>` (`docs/data-model.md`, "Notifications").
///
/// The payload is a bound parameter, never formatted into the statement, and
/// the call is made on the caller's transaction, so PostgreSQL delivers it
/// only on commit and discards it on rollback (ADR 0028).
async fn notify_task_events(tx: &mut PgConnection, payload: &str) -> Result<()> {
    sqlx::query!("SELECT pg_notify('task_events', $1)", payload)
        .execute(&mut *tx)
        .await?;

    Ok(())
}

/// Turn a `task_events` primary-key collision into an internal error.
///
/// `(project_id, seq)` can only collide if two writers allocated the same
/// sequence, which the project row lock exists to prevent. That is a broken
/// invariant in the orchestrator, not a client mistake and not something to
/// retry, so it is logged with the project and answered 500 with a generic
/// message (`CLAUDE.md`, "Backend conventions").
fn map_sequence_collision(err: sqlx::Error, project_id: Uuid) -> Error {
    match unique_violation(&err) {
        Some("task_events_pkey") => {
            error!(project_id = %project_id, "task event sequence collision");
            Error::Internal("task event sequence collision".into())
        }
        _ => Error::from(err),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::events::TaskActor;
    use crate::models::task_event_kind;
    use crate::repositories::TaskRepository;
    use crate::repositories::tasks::testfix::Fixture;
    use crate::tracker::TrackerMutation;

    #[test]
    fn the_replay_page_is_the_session_stream_bound() {
        // The same bound as `SessionRepository::MAX_EVENT_PAGE`; a client
        // further behind than one page reads again from its last `seq`.
        assert_eq!(MAX_TASK_EVENT_PAGE, 500);
    }

    /// A batch naming another project's task is refused whole.
    ///
    /// The one rule of [`TaskRepository::append_task_events`] that no verb can
    /// express: `TrackerMutation`'s typed `emit_*` methods only ever carry a
    /// task the mutation just changed, so a batch that is out of scope cannot
    /// be built above the repository — and the check exists precisely for the
    /// writer that gets it wrong. Asserted here because the statement is
    /// `pub(crate)`, `TrackerMutation::commit` being its only caller.
    ///
    /// The `deleted` exception — the one kind whose task row is expected to be
    /// gone (ADR 0022) — is covered through `delete_task` in
    /// `tests/tasks_api.rs`.
    #[tokio::test]
    async fn an_event_about_another_project_s_task_is_refused_and_writes_nothing() {
        let fixture = Fixture::create().await;
        let repository = TaskRepository::new(&fixture.pool);

        let mine = fixture.task(fixture.project_id, "mine").await;
        let theirs = fixture.task(fixture.other_project_id, "theirs").await;

        let batch = [
            NewTaskEvent::about(mine.id, task_event_kind::UPDATED, json!({})),
            NewTaskEvent::about(theirs.id, task_event_kind::UPDATED, json!({})),
        ];

        let mut mutation =
            TrackerMutation::begin(&fixture.pool, fixture.project_id, TaskActor::System)
                .await
                .expect("the mutation opens");
        let refused = repository
            .append_task_events(mutation.conn(), fixture.project_id, &batch)
            .await;
        mutation.no_change().await.expect("the mutation rolls back");

        assert!(
            matches!(refused, Err(Error::NotFound)),
            "a foreign task's event was accepted: {refused:?}",
        );

        // Whole batch or none: the first event is in scope and is still not
        // written, because the check runs over every event before any row.
        assert_eq!(
            repository
                .max_task_event_seq(fixture.project_id)
                .await
                .expect("the cursor reads"),
            0,
        );
    }
}
