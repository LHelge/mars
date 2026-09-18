//! All SQL against `sessions` and `events`, including the one append path
//! every event writer in the orchestrator goes through.
//!
//! `docs/data-model.md`, "Sessions and events" is the contract, and its
//! `events` paragraph is the reason this file exists in the shape it does:
//!
//! > `seq` is monotonic per session and is derived from the table at insert
//! > time, never from an in-memory counter. Every writer begins a transaction
//! > and locks the existing session row before reading the sequence or
//! > transcript offset (ADR 0021).
//!
//! [`SessionRepository::append_events`] is that writer. The session owner, the
//! launcher, recovery, the reaper and the git handlers all call it rather than
//! writing their own insert, so there is exactly one implementation of the
//! locking contract to get right. A primary-key collision therefore is not a
//! race to retry but an invariant failure: it means some writer bypassed the
//! lock, and it surfaces as [`Error::Internal`].
//!
//! Notifications are issued with `pg_notify` inside the same transaction as
//! the rows they announce; PostgreSQL delivers them only after commit and
//! discards them on rollback, so there is never a separate post-commit write
//! (ADR 0028; `ARCHITECTURE.md`, "Event delivery"). Payloads are bound
//! parameters, never formatted into the statement.

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::{
    EventRow, NewEvent, NewSession, ProfileKind, Session, SessionError, SessionState, SessionTitle,
    StateChange,
};
use crate::prelude::*;
use crate::repositories::unique_violation;

/// The largest page `GET /sessions/{id}/events` will return (`SPEC.md`,
/// "Sessions": `?before=<seq>&limit=<n≤500>`).
pub const MAX_EVENT_PAGE: u32 = 500;

/// All SQL against `sessions` and `events` (`ARCHITECTURE.md`, "Orchestrator
/// internals").
///
/// Reads that need no transaction go straight to the pool; everything that
/// mutates, locks or has to be read under someone else's lock takes the
/// caller's `&mut PgConnection`, so one transaction can hold a whole
/// composition — a state change, its `state_change` event and the tracker
/// rows it releases — together.
pub struct SessionRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> SessionRepository<'a> {
    /// Borrow `pool` for the lifetime of this repository.
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new session and return the stored row.
    ///
    /// Validates the [`NewSession`] first, so a hand-built struct cannot store
    /// a branch that is not the session's own. The caller's transaction is
    /// where the rest of the creation lives: `SPEC.md`, "Sessions" claims
    /// `task_id` in this same transaction.
    pub async fn insert(&self, tx: &mut PgConnection, session: &NewSession) -> Result<Session> {
        session.validate()?;

        let inserted = sqlx::query_as!(
            Session,
            r#"
            INSERT INTO sessions (id, project_id, profile_id, kind, created_by, title,
                                  base_ref, branch, mcp_token_hash, task_id, handoff_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING id, project_id, profile_id, kind AS "kind: ProfileKind", created_by,
                      title, task_id, handoff_id, state AS "state: SessionState", base_ref,
                      branch, container_id, cli_session_id, mcp_token_hash, last_seq,
                      last_activity_at, cost_usd, input_tokens, output_tokens, error,
                      created_at, parked_at, ended_at
            "#,
            session.id,
            session.project_id,
            session.profile_id,
            session.kind as ProfileKind,
            session.created_by,
            session.title.as_ref().map(SessionTitle::as_str),
            session.base_ref,
            session.branch,
            session.mcp_token_hash,
            session.task_id,
            session.handoff_id,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| map_token_collision(err, session.id))?;

        debug!(
            session_id = %inserted.id,
            project_id = %inserted.project_id,
            kind = ?inserted.kind,
            "session inserted",
        );

        Ok(inserted)
    }

    /// The session with this id, or `None`.
    pub async fn find(&self, id: Uuid) -> Result<Option<Session>> {
        let session = sqlx::query_as!(
            Session,
            r#"
            SELECT id, project_id, profile_id, kind AS "kind: ProfileKind", created_by,
                   title, task_id, handoff_id, state AS "state: SessionState", base_ref,
                   branch, container_id, cli_session_id, mcp_token_hash, last_seq,
                   last_activity_at, cost_usd, input_tokens, output_tokens, error,
                   created_at, parked_at, ended_at
            FROM sessions
            WHERE id = $1
            "#,
            id,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(session)
    }

    /// The session with this id *within* this project, or `None`.
    ///
    /// The scope is in the `WHERE` clause rather than checked afterwards
    /// (`CLAUDE.md`, "Backend conventions"), so a project-scoped route cannot
    /// leak the existence of another project's session.
    pub async fn find_in_project(&self, project_id: Uuid, id: Uuid) -> Result<Option<Session>> {
        let session = sqlx::query_as!(
            Session,
            r#"
            SELECT id, project_id, profile_id, kind AS "kind: ProfileKind", created_by,
                   title, task_id, handoff_id, state AS "state: SessionState", base_ref,
                   branch, container_id, cli_session_id, mcp_token_hash, last_seq,
                   last_activity_at, cost_usd, input_tokens, output_tokens, error,
                   created_at, parked_at, ended_at
            FROM sessions
            WHERE id = $1 AND project_id = $2
            "#,
            id,
            project_id,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(session)
    }

    /// Every session of one project, newest first, optionally filtered by
    /// state (`GET /projects/{pid}/sessions?state=`).
    ///
    /// `id` breaks ties so two sessions created in the same transaction — and
    /// therefore sharing `NOW()` — still come back in a stable order.
    pub async fn list_by_project(
        &self,
        project_id: Uuid,
        state: Option<SessionState>,
    ) -> Result<Vec<Session>> {
        let sessions = sqlx::query_as!(
            Session,
            r#"
            SELECT id, project_id, profile_id, kind AS "kind: ProfileKind", created_by,
                   title, task_id, handoff_id, state AS "state: SessionState", base_ref,
                   branch, container_id, cli_session_id, mcp_token_hash, last_seq,
                   last_activity_at, cost_usd, input_tokens, output_tokens, error,
                   created_at, parked_at, ended_at
            FROM sessions
            WHERE project_id = $1
              AND ($2::session_state IS NULL OR state = $2)
            ORDER BY created_at DESC, id
            "#,
            project_id,
            state as Option<SessionState>,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(sessions)
    }

    /// Every session across all projects, newest first, optionally filtered by
    /// state (`GET /sessions?state=`, the dashboard).
    pub async fn list_all(&self, state: Option<SessionState>) -> Result<Vec<Session>> {
        let sessions = sqlx::query_as!(
            Session,
            r#"
            SELECT id, project_id, profile_id, kind AS "kind: ProfileKind", created_by,
                   title, task_id, handoff_id, state AS "state: SessionState", base_ref,
                   branch, container_id, cli_session_id, mcp_token_hash, last_seq,
                   last_activity_at, cost_usd, input_tokens, output_tokens, error,
                   created_at, parked_at, ended_at
            FROM sessions
            WHERE $1::session_state IS NULL OR state = $1
            ORDER BY created_at DESC, id
            "#,
            state as Option<SessionState>,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(sessions)
    }

    /// How many sessions of this project are `running` or `creating`.
    ///
    /// The one question behind every running-session refusal: deleting a
    /// project, deleting or clearing a shared directory and the other
    /// destructive project operations are 409 while a session is live
    /// (`SPEC.md`, "Projects", "Shared directories"). It takes the caller's
    /// connection because the count is only authoritative inside the
    /// transaction that holds the project lock and then performs the deletion;
    /// counting on the pool first and deleting afterwards is exactly the race
    /// the lock exists to close (`ARCHITECTURE.md`, "Task tracker"; ADR 0021).
    ///
    /// `parked` does not count. The documented set is exactly `running` and
    /// `creating` — the two states that have, or are about to have, a
    /// container holding the project's files open.
    pub async fn count_live_for_project(
        &self,
        tx: &mut PgConnection,
        project_id: Uuid,
    ) -> Result<i64> {
        let count = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) AS "count!"
            FROM sessions
            WHERE project_id = $1
              AND state IN ('running'::session_state, 'creating'::session_state)
            "#,
            project_id,
        )
        .fetch_one(&mut *tx)
        .await?;

        Ok(count)
    }

    /// The ids of every session of this project, whatever its state.
    ///
    /// Project deletion's list of directories to remove
    /// ([`crate::projects::delete_project`]): the rows go with the project by
    /// cascade, but `DATA_DIR/sessions/<id>/` does not, so the ids are read
    /// while they still exist. It takes the caller's connection because they
    /// have to be the ids the *deleting* transaction removes — read on the
    /// pool beforehand, a session created in between would leave its directory
    /// behind.
    ///
    /// Ordered by id, so a log line or a test reads the same list twice.
    pub async fn ids_for_project(
        &self,
        tx: &mut PgConnection,
        project_id: Uuid,
    ) -> Result<Vec<Uuid>> {
        let ids = sqlx::query_scalar!(
            "SELECT id FROM sessions WHERE project_id = $1 ORDER BY id",
            project_id,
        )
        .fetch_all(&mut *tx)
        .await?;

        debug!(project_id = %project_id, count = ids.len(), "project sessions listed");

        Ok(ids)
    }

    /// Retitle a session and return the stored row, or `None` when no session
    /// has this id (`PUT /sessions/{id}`).
    ///
    /// `None` for `title` clears it back to untitled.
    pub async fn update_title(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        title: Option<&SessionTitle>,
    ) -> Result<Option<Session>> {
        let updated = sqlx::query_as!(
            Session,
            r#"
            UPDATE sessions
            SET title = $2
            WHERE id = $1
            RETURNING id, project_id, profile_id, kind AS "kind: ProfileKind", created_by,
                      title, task_id, handoff_id, state AS "state: SessionState", base_ref,
                      branch, container_id, cli_session_id, mcp_token_hash, last_seq,
                      last_activity_at, cost_usd, input_tokens, output_tokens, error,
                      created_at, parked_at, ended_at
            "#,
            id,
            title.map(SessionTitle::as_str),
        )
        .fetch_optional(&mut *tx)
        .await?;

        debug!(session_id = %id, updated = updated.is_some(), "session retitled");

        Ok(updated)
    }

    /// Record the engine container id of the current container, or clear it
    /// with `None` once the container is removed.
    ///
    /// `Ok(false)` when no session has this id.
    pub async fn set_container_id(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        container_id: Option<&str>,
    ) -> Result<bool> {
        let result = sqlx::query!(
            "UPDATE sessions SET container_id = $2 WHERE id = $1",
            id,
            container_id,
        )
        .execute(&mut *tx)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Record the CLI's own session id, taken from its `init` event and needed
    /// for `--resume`.
    ///
    /// `Ok(false)` when no session has this id.
    pub async fn set_cli_session_id(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        cli_session_id: &str,
    ) -> Result<bool> {
        let result = sqlx::query!(
            "UPDATE sessions SET cli_session_id = $2 WHERE id = $1",
            id,
            cli_session_id,
        )
        .execute(&mut *tx)
        .await?;

        debug!(session_id = %id, "cli session id recorded");

        Ok(result.rows_affected() > 0)
    }

    /// Store the hash of the MCP bearer token the *next* process launch will
    /// use (`docs/data-model.md`, `sessions.mcp_token_hash`; ADR 0029).
    ///
    /// Each launch, including resume and retry, generates a fresh token; the
    /// replacement hash has to commit before the new process starts. The raw
    /// token never reaches this repository — only its SHA-256 — and is never
    /// logged (`CLAUDE.md`, rule 3). `Ok(false)` when no session has this id.
    pub async fn set_mcp_token_hash(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        hash: &str,
    ) -> Result<bool> {
        let result = sqlx::query!(
            "UPDATE sessions SET mcp_token_hash = $2 WHERE id = $1",
            id,
            hash,
        )
        .execute(&mut *tx)
        .await
        .map_err(|err| map_token_collision(err, id))?;

        debug!(session_id = %id, "mcp token hash replaced");

        Ok(result.rows_affected() > 0)
    }

    /// Delete a session, reporting whether a row matched.
    ///
    /// `Ok(false)` rather than an error when nothing matched: the route turns
    /// that into 404. `events` cascade with the row; whether the session is in
    /// a state that may be deleted, and removing its directory, are the
    /// route's (`SPEC.md`, "Sessions").
    pub async fn delete(&self, tx: &mut PgConnection, id: Uuid) -> Result<bool> {
        let result = sqlx::query!("DELETE FROM sessions WHERE id = $1", id)
            .execute(&mut *tx)
            .await?;

        let deleted = result.rows_affected() > 0;
        debug!(session_id = %id, deleted, "session deleted");

        Ok(deleted)
    }

    /// Lock the session row `FOR UPDATE` and return it as read under the lock.
    ///
    /// Every event writer and every state change serialises here (ADR 0021):
    /// the lock is taken before the sequence or the transcript offset is read,
    /// and held until the caller's transaction commits or rolls back. An
    /// unlocked [`SessionRepository::find`] may locate the session first; the
    /// value it returned is not authoritative and must be replaced by this one.
    ///
    /// **Lock order.** A transaction that also mutates tracker rows locks the
    /// project row *before* this one, and any git lock comes before either
    /// (ADR 0021; `CLAUDE.md`, "Backend conventions").
    ///
    /// [`Error::NotFound`] when the session is gone, which is the same answer
    /// the route would give.
    pub async fn lock_session(&self, tx: &mut PgConnection, id: Uuid) -> Result<Session> {
        let session = sqlx::query_as!(
            Session,
            r#"
            SELECT id, project_id, profile_id, kind AS "kind: ProfileKind", created_by,
                   title, task_id, handoff_id, state AS "state: SessionState", base_ref,
                   branch, container_id, cli_session_id, mcp_token_hash, last_seq,
                   last_activity_at, cost_usd, input_tokens, output_tokens, error,
                   created_at, parked_at, ended_at
            FROM sessions
            WHERE id = $1
            FOR UPDATE
            "#,
            id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;

        debug!(session_id = %id, state = session.state.as_str(), "session row locked");

        Ok(session)
    }

    /// Move a session to `to`, validating the transition under the session row
    /// lock and announcing it on `session_state`.
    ///
    /// The current state is read under the lock rather than trusted from the
    /// caller, so two concurrent stops cannot both pass the check. Entering
    /// `parked` sets `parked_at`, entering `done` or `failed` sets `ended_at`,
    /// and `change.error` is stored only when entering `failed`
    /// (`docs/data-model.md`, `sessions`).
    ///
    /// The `state_change` event is deliberately *not* inserted here: the
    /// caller appends it with [`SessionRepository::append_events`] in the same
    /// transaction, because only the caller knows the reason and the signal
    /// that ended the run (`SPEC.md`, "AgentEvent"; `ARCHITECTURE.md`, "Stop
    /// semantics").
    ///
    /// [`SessionError::InvalidTransition`] — a 409 — when the lifecycle
    /// diagram has no such edge, including a transition to the state the
    /// session is already in.
    pub async fn set_state(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        to: SessionState,
        change: &StateChange,
    ) -> Result<Session> {
        let current = self.lock_session(&mut *tx, id).await?;
        let from = current.state;

        if !from.can_transition_to(to) {
            return Err(SessionError::InvalidTransition { from, to }.into());
        }

        // Which timestamps the transition sets follows from the target state
        // alone, so it is decided here and bound as plain booleans rather than
        // re-derived from an enum comparison in SQL.
        let entering_parked = to == SessionState::Parked;
        let entering_ended = matches!(to, SessionState::Done | SessionState::Failed);
        let entering_failed = to == SessionState::Failed;

        let updated = sqlx::query_as!(
            Session,
            r#"
            UPDATE sessions
            SET state = $2,
                parked_at = CASE WHEN $3 THEN NOW() ELSE parked_at END,
                ended_at = CASE WHEN $4 THEN NOW() ELSE ended_at END,
                error = CASE WHEN $5 THEN $6 ELSE error END
            WHERE id = $1
            RETURNING id, project_id, profile_id, kind AS "kind: ProfileKind", created_by,
                      title, task_id, handoff_id, state AS "state: SessionState", base_ref,
                      branch, container_id, cli_session_id, mcp_token_hash, last_seq,
                      last_activity_at, cost_usd, input_tokens, output_tokens, error,
                      created_at, parked_at, ended_at
            "#,
            id,
            to as SessionState,
            entering_parked,
            entering_ended,
            entering_failed,
            change.error.as_deref(),
        )
        .fetch_one(&mut *tx)
        .await?;

        // In this transaction, so the notification is delivered if and only if
        // the state change commits (ADR 0028).
        notify_session_state(&mut *tx, &format!("{id}:{to}")).await?;

        debug!(
            session_id = %id,
            from = from.as_str(),
            to = to.as_str(),
            "session state changed",
        );

        Ok(updated)
    }

    /// Append a batch of events to a session and announce the batch on
    /// `session_events`.
    ///
    /// This is the only event-insert path in the orchestrator
    /// (`docs/data-model.md`, `events`). It locks the session row, then
    /// derives each `seq` in the insert statement itself with
    /// `COALESCE(MAX(seq), 0) + 1`, then caches the highest one in
    /// `sessions.last_seq`, advances `last_activity_at` and issues one
    /// notification carrying `<session_id>:<highest seq>` — all in the
    /// caller's transaction, so a rollback publishes neither the rows nor the
    /// notification (ADR 0028).
    ///
    /// **Locking contract.** The caller's transaction must not already hold a
    /// session row lock for another session and, if it also mutates tracker
    /// rows, must have locked the project row first (ADR 0021).
    ///
    /// An empty slice is a no-op returning `Ok(vec![])`: no lock, no
    /// notification, no `last_activity_at` bump. Otherwise the returned
    /// sequences are in the order the events were given, consecutive, and
    /// start just after the session's previous highest.
    ///
    /// A primary-key collision means a writer bypassed this path and allocated
    /// a sequence outside the lock. That is an invariant failure, not a race
    /// to retry: the batch is never partially retried, the caller's
    /// transaction is expected to roll back and the error is
    /// [`Error::Internal`].
    pub async fn append_events(
        &self,
        tx: &mut PgConnection,
        session_id: Uuid,
        events: &[NewEvent],
    ) -> Result<Vec<i64>> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        // The lock, not the insert, is what makes `MAX(seq) + 1` safe: a
        // waiting writer reads the previous writer's committed events only
        // after it acquires this (`docs/data-model.md`, `events`).
        let locked = sqlx::query_scalar!(
            "SELECT id FROM sessions WHERE id = $1 FOR UPDATE",
            session_id
        )
        .fetch_optional(&mut *tx)
        .await?;
        if locked.is_none() {
            return Err(Error::NotFound);
        }

        let mut sequences = Vec::with_capacity(events.len());
        for event in events {
            let seq = sqlx::query_scalar!(
                r#"
                INSERT INTO events (session_id, seq, ts, kind, payload)
                SELECT $1, COALESCE(MAX(seq), 0) + 1, $2, $3, $4
                FROM events
                WHERE session_id = $1
                RETURNING seq AS "seq!"
                "#,
                session_id,
                event.ts,
                event.kind,
                event.payload,
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| map_sequence_collision(err, session_id))?;

            sequences.push(seq);
        }

        // Safe to unwrap: the slice was checked non-empty above.
        let last_seq = *sequences.last().expect("the batch has at least one event");

        sqlx::query!(
            "UPDATE sessions SET last_seq = $2, last_activity_at = NOW() WHERE id = $1",
            session_id,
            last_seq,
        )
        .execute(&mut *tx)
        .await?;

        // One notification per batch, carrying the highest sequence it wrote;
        // consumers read the rows themselves (ADR 0028).
        notify_session_events(&mut *tx, &format!("{session_id}:{last_seq}")).await?;

        // The kinds, never the payloads: a payload may hold agent output, and
        // that is never logged at `info` or above (`CLAUDE.md`, rule 3).
        debug!(
            session_id = %session_id,
            count = events.len(),
            last_seq,
            "events appended",
        );

        Ok(sequences)
    }

    /// Add one `result` event's cost and tokens to the session's counters
    /// (`ARCHITECTURE.md`, "Cost accounting").
    ///
    /// Takes the caller's connection because the owner runs it in the same
    /// transaction that appends the `result` event, under the same session row
    /// lock. Increments only: [`SessionError::InvalidUsage`] rejects a
    /// negative or non-finite value rather than letting a bad `result` payload
    /// walk a counter backwards.
    pub async fn add_usage(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        cost_usd: f64,
        input_tokens: i64,
        output_tokens: i64,
    ) -> Result<()> {
        if !cost_usd.is_finite() || cost_usd < 0.0 || input_tokens < 0 || output_tokens < 0 {
            return Err(SessionError::InvalidUsage.into());
        }

        sqlx::query!(
            r#"
            UPDATE sessions
            SET cost_usd = cost_usd + $2,
                input_tokens = input_tokens + $3,
                output_tokens = output_tokens + $4
            WHERE id = $1
            "#,
            id,
            cost_usd,
            input_tokens,
            output_tokens,
        )
        .execute(&mut *tx)
        .await?;

        Ok(())
    }

    /// One page of a session's events, oldest first, ending just before
    /// `before` (`GET /sessions/{id}/events?before=&limit=`).
    ///
    /// `before` is the exclusive upper `seq` bound and `None` means "the
    /// newest page". `limit` is clamped to `1..=`[`MAX_EVENT_PAGE`], so a
    /// caller cannot ask for the whole stream in one request. The second
    /// element of the pair is `has_more`: whether an older page exists, which
    /// is answered by reading one row further than the page and dropping it.
    pub async fn list_events(
        &self,
        session_id: Uuid,
        before: Option<i64>,
        limit: u32,
    ) -> Result<(Vec<EventRow>, bool)> {
        let limit = limit.clamp(1, MAX_EVENT_PAGE);

        // One extra row answers `has_more` without a second count query.
        let mut rows = sqlx::query_as!(
            EventRow,
            r#"
            SELECT session_id, seq, ts, kind, payload
            FROM events
            WHERE session_id = $1
              AND ($2::bigint IS NULL OR seq < $2)
            ORDER BY seq DESC
            LIMIT $3
            "#,
            session_id,
            before,
            i64::from(limit) + 1,
        )
        .fetch_all(self.pool)
        .await?;

        let has_more = rows.len() > limit as usize;
        rows.truncate(limit as usize);
        // Read newest-first for the `before` cursor, returned newest-last as
        // the endpoint promises.
        rows.reverse();

        Ok((rows, has_more))
    }

    /// Every event after `after`, oldest first.
    ///
    /// The replay half of `GET /ws/sessions/{id}?after=`: the handler
    /// subscribes to the notification fan-out, then replays from its cursor,
    /// then follows notifications (`ARCHITECTURE.md`, "Event delivery").
    /// Unbounded on purpose — a client's cursor names how far behind it is,
    /// and the stream has to catch up completely.
    pub async fn list_events_after(&self, session_id: Uuid, after: i64) -> Result<Vec<EventRow>> {
        let rows = sqlx::query_as!(
            EventRow,
            r#"
            SELECT session_id, seq, ts, kind, payload
            FROM events
            WHERE session_id = $1 AND seq > $2
            ORDER BY seq
            "#,
            session_id,
            after,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(rows)
    }

    /// The highest committed sequence of a session, or `0` when it has no
    /// events yet.
    ///
    /// Read from `events`, which is the truth; `sessions.last_seq` is only a
    /// cache of it (`docs/data-model.md`, `sessions`).
    pub async fn max_seq(&self, session_id: Uuid) -> Result<i64> {
        let max = sqlx::query_scalar!(
            r#"SELECT COALESCE(MAX(seq), 0) AS "max!" FROM events WHERE session_id = $1"#,
            session_id,
        )
        .fetch_one(self.pool)
        .await?;

        Ok(max)
    }

    /// The highest transcript byte offset recorded in this session's events,
    /// or `None` when no event carries one.
    ///
    /// Recovery resumes tailing `log/stream.jsonl` from here
    /// (`ARCHITECTURE.md`, "Durability and recovery"); the field is
    /// [`crate::models::OFFSET_FIELD`], internal and stripped before an event
    /// leaves the orchestrator.
    ///
    /// Read on the pool, so the answer is a snapshot: a writer that rechecks
    /// the offset *under* the session row lock before appending reads it
    /// inside its own transaction, which the session-owner epic adds when it
    /// needs it.
    pub async fn max_offset(&self, session_id: Uuid) -> Result<Option<i64>> {
        let max = sqlx::query_scalar!(
            r#"
            SELECT MAX((payload->>'_offset')::bigint)
            FROM events
            WHERE session_id = $1
            "#,
            session_id,
        )
        .fetch_one(self.pool)
        .await?;

        Ok(max)
    }
}

/// Announce a committed event batch on `session_events`, payload
/// `<session_id>:<seq>` (`docs/data-model.md`, "Notifications").
///
/// The payload is a bound parameter, never formatted into the statement, and
/// the call is made on the caller's transaction, so PostgreSQL delivers it
/// only on commit and discards it on rollback (ADR 0028).
async fn notify_session_events(tx: &mut PgConnection, payload: &str) -> Result<()> {
    sqlx::query!("SELECT pg_notify('session_events', $1)", payload)
        .execute(&mut *tx)
        .await?;

    Ok(())
}

/// Announce a committed state change on `session_state`, payload
/// `<session_id>:<state>`. Bound and in-transaction for the same reasons as
/// [`notify_session_events`].
async fn notify_session_state(tx: &mut PgConnection, payload: &str) -> Result<()> {
    sqlx::query!("SELECT pg_notify('session_state', $1)", payload)
        .execute(&mut *tx)
        .await?;

    Ok(())
}

/// Map a duplicate `mcp_token_hash` to an internal error.
///
/// The hash is the SHA-256 of a freshly generated random token, so a
/// collision is either astronomically unlikely or a bug that reused a token —
/// never something the caller can fix, and never something to tell them about.
fn map_token_collision(err: sqlx::Error, id: Uuid) -> Error {
    match unique_violation(&err) {
        Some("sessions_mcp_token_hash_key") => {
            error!(session_id = %id, "mcp token hash collision");
            Error::Internal("mcp token hash collision".into())
        }
        _ => Error::from(err),
    }
}

/// Map a duplicate `(session_id, seq)` to an internal error.
///
/// `docs/data-model.md`, `events`: "A primary-key collision indicates a writer
/// bypassed the locking contract or another implementation defect; roll back
/// and surface an error, without retrying only part of the batch."
fn map_sequence_collision(err: sqlx::Error, session_id: Uuid) -> Error {
    match unique_violation(&err) {
        Some("events_pkey") => {
            error!(session_id = %session_id, "event sequence collision");
            Error::Internal("event sequence collision".into())
        }
        _ => Error::from(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_limit_is_the_documented_maximum() {
        // `SPEC.md`, "Sessions": `?before=<seq>&limit=<n≤500>`.
        assert_eq!(MAX_EVENT_PAGE, 500);
    }
}
