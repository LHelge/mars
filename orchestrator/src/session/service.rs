//! [`SessionService`], the lifecycle rules for everything a *user* asks of a
//! session: input, stop, end, retry, sync and delete.
//!
//! The launcher starts sessions and the owner runs them; this is the other
//! half, the one place the REST routes, the WebSocket handler and the cron jobs
//! reach for so that all three apply the same rules (`ARCHITECTURE.md`,
//! "Session lifecycle", "Stop semantics", "Storage"; `SPEC.md`, "Sessions").
//! Nothing here decides anything a document does not fix:
//!
//! - **What may be asked at all** is the session model's:
//!   [`Session::accepts_input`], [`Session::can_retry`] and
//!   [`Session::can_delete`] are the state table of "Session lifecycle" as
//!   code, and this module only widens their refusals into HTTP answers.
//! - **Where an input goes** is the registry's ([`super::SessionRegistry::submit`]),
//!   and a `parked` session's relaunch is the launcher's
//!   ([`super::Launcher::launch`] with [`LaunchMode::Resume`]).
//! - **The fetch-back** is the git layer's ([`GitService::sync_session`]),
//!   which takes the project git lock and writes the `git { op: "sync" }`
//!   event itself; this module only sequences it and reads its outcome
//!   (`ARCHITECTURE.md`, "Git model", Fetch-back; ADR 0021).
//! - **The state changes** are [`SessionRepository::transition`]'s, under the
//!   session row lock, so a service action that raced the idle reaper loses the
//!   race instead of overwriting it (ADR 0021).
//!
//! **Ending is not forcing `done`.** `end` ends the run and then moves the
//! session to `done` from `creating`, `running` or `parked`, which are the
//! three edges the lifecycle diagram has into it. A run that ended `failed`
//! while the stop
//! was in flight — which is every ephemeral session, because an ephemeral
//! session is never parked (ADR 0003) — stays `failed`: the diagram has no
//! `failed → done` edge, and inventing one in the service would make the state
//! machine two different things in two places. The session is still fetched
//! back, its container still removed, its registry entry still dropped and
//! [`AppState::session_ended`] still invoked, and the row comes back as it is.
//!
//! **Sessions a restart forgot.** The registry is in memory (ADR 0020), so
//! after a restart a `parked` session has no entry and
//! [`super::SessionRegistry::submit`] would answer "session has no owner" for a
//! session that is perfectly resumable. `send_input` therefore submits through
//! [`super::SessionRegistry::submit_parked`] when the row it read says `parked`, which
//! registers the entry, queues the input and decides on the relaunch under one
//! acquisition of the registry's mutex.

use std::time::Duration;

use chrono::Utc;
use uuid::Uuid;

use super::launcher::LaunchMode;
use super::owner::StopReason;
use super::registry::{QueuedInput, StopOutcome, SubmitResult};
use crate::engine::{ContainerId, EngineError, Signal};
use crate::events::SessionInput;
use crate::git::{GitActor, GitService};
use crate::models::{Session, SessionState, SyncOutcome};
use crate::prelude::*;
use crate::projects::layout::{remove_dir_all, remove_file, session_dir};
use crate::repositories::{SessionRepository, Transition};

/// What `end` records in the `state_change` it writes (`SPEC.md`,
/// "AgentEvent").
const ENDED_REASON: &str = "ended by user";

/// What the `failed → parked` retry records.
const RETRIED_REASON: &str = "retried by user";

/// How often `end` looks at the row while it waits for the run to finish.
///
/// The owner writes the transition, so the wait is a read of the row rather
/// than a channel: the owner may be a task of this process or, after a restart,
/// one that adopted the container. 100 ms is well inside what a stop takes and
/// costs a handful of queries.
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long `end` waits for a launch it cancelled to let go of the session.
///
/// The launch checks the cancellation between its steps, so the wait is one
/// step of the launch sequence and not the whole of it: the longest of them is
/// an image pull or a clone of a large repository. Past this bound the `end`
/// stops waiting and closes the session anyway, removing whatever container the
/// row names — the launch then finds its own `creating → running` transition
/// refused, and its failure path removes anything it created after that
/// (`ARCHITECTURE.md`, "Session lifecycle", "A session ended while it is
/// creating").
const LAUNCH_CANCEL_TIMEOUT: Duration = Duration::from_secs(30);

/// How much longer than the stop's own grace period `end` waits before it kills
/// the container itself.
///
/// The owner escalates to `SIGTERM` at `STOP_GRACE_SECS` (`ARCHITECTURE.md`,
/// "Stop semantics"); this is the margin after that in which it should have
/// committed the transition. Past it the owner is not answering, and `end` stops
/// waiting for it and removes the container.
const EXIT_GRACE_MARGIN: Duration = Duration::from_secs(10);

/// The user-initiated half of the session lifecycle.
///
/// Cheap to build — one [`AppState`] clone, which is a handful of `Arc`s — so a
/// handler builds one per request with [`SessionService::new`] rather than the
/// state carrying a field that would point back at it.
#[derive(Clone)]
pub struct SessionService {
    state: AppState,
}

impl SessionService {
    /// The service over the state the caller already holds.
    pub fn new(state: &AppState) -> Self {
        Self {
            state: state.clone(),
        }
    }

    /// Accept one input for a session (`SPEC.md`, "Sessions", `POST
    /// /sessions/{id}/input`; "WebSocket: session stream").
    ///
    /// The model decides whether the session may be sent anything at all: an
    /// ephemeral session never can, and a `done` or `failed` one no longer can
    /// (409 either way). What is accepted is forwarded to a live owner, queued
    /// for one that is `creating` or being resumed, or queued with a relaunch
    /// started here — which is the documented "a message to a parked session
    /// relaunches it" rule. `Ok(())` is acceptance by the orchestrator and not
    /// delivery to the CLI (ADR 0020).
    pub async fn send_input(
        &self,
        session_id: Uuid,
        input: SessionInput,
        user_id: Option<Uuid>,
        client_id: Option<String>,
    ) -> Result<()> {
        let session = SessionRepository::new(&self.state.pool)
            .get(session_id)
            .await?;
        session.accepts_input()?;

        let queued = QueuedInput {
            input,
            user_id,
            client_id,
            accepted_at: Utc::now(),
        };
        let registry = &self.state.session_registry;
        let submitted = if session.state == SessionState::Parked {
            // A restart leaves a `parked` row with no entry; registering it
            // parked is what makes the queue-and-relaunch below possible.
            registry.submit_parked(session_id, session.kind, queued)
        } else {
            registry.submit(session_id, queued)
        };

        match submitted {
            SubmitResult::Forwarded | SubmitResult::Queued => Ok(()),
            SubmitResult::ParkedNeedsRelaunch => {
                // The input is already queued; the resume drains it once stdin
                // is attached. `None` means another launch got there first,
                // which will drain it just the same.
                self.state.launcher().launch(session_id, LaunchMode::Resume);
                info!(session_id = %session_id, "a message to a parked session is resuming it");
                Ok(())
            }
            SubmitResult::Rejected(reason) => Err(Error::Conflict(reason)),
        }
    }

    /// Ask a running session to stop and return at once (`SPEC.md`, "Sessions",
    /// `POST /sessions/{id}/stop` → 202).
    ///
    /// `SIGINT` now and `SIGTERM` after `STOP_GRACE_SECS` are the owner's, as
    /// is the `parked` (or, for an ephemeral session, `failed`) the exit leads
    /// to (`ARCHITECTURE.md`, "Stop semantics"). Only a `running` session has a
    /// run to stop; anything else is a conflict.
    pub async fn stop(&self, session_id: Uuid) -> Result<()> {
        let session = SessionRepository::new(&self.state.pool)
            .get(session_id)
            .await?;
        require_state(&session, &[SessionState::Running])?;

        match self
            .state
            .session_registry
            .stop(session_id, StopReason::User)
        {
            // A stop already under way is the stop this caller asked for: the
            // session is going down either way, which is what 202 promises.
            StopOutcome::Sent | StopOutcome::AlreadyStopping { .. } => Ok(()),
            StopOutcome::NoOwner => Err(Error::Conflict("session is not running".to_string())),
        }
    }

    /// End a session: stop it, fetch its branch back, remove its container and
    /// close it (`SPEC.md`, "Sessions", `POST /sessions/{id}/end`;
    /// `ARCHITECTURE.md`, "Stop semantics").
    ///
    /// Allowed from `creating`, `running` and `parked`. From `creating` the
    /// launch is cancelled first: the flag goes into the registry, the launch
    /// stops at its next checkpoint and removes whatever container it had got as
    /// far as creating, and this waits [`LAUNCH_CANCEL_TIMEOUT`] for it to let
    /// go of the session before re-reading the row — which is what makes the
    /// `done` this answers a session with nothing left running behind it
    /// (`ARCHITECTURE.md`, "Session lifecycle", "A session ended while it is
    /// creating"). A launch that reached `running` first is then ended the
    /// ordinary way, because the row read after the wait is the one that
    /// decides.
    ///
    /// From `running` it is the stop
    /// followed by a bounded wait for the owner's own transition, and a
    /// `SIGKILL` and forced removal if that wait runs out. Then, whatever state
    /// the run ended in, the session branch is fetched into the mirror under the
    /// project git lock and the outcome recorded as `git { op: "sync" }` — a
    /// failure there is recorded, never fatal, because the session did run and
    /// what is left to say is that its branch did not arrive. A session ended
    /// during `creating` goes through the same fetch-back: it has a work clone
    /// if its launch got that far, and if it did not, [`GitService::sync_session`]
    /// refuses a session with no work tree without an event, which is the
    /// documented no-op rather than a failure.
    ///
    /// The session then moves to `done`, and the module documentation says why a
    /// run that ended `failed` is left `failed` instead: the lifecycle diagram
    /// has no `failed → done` edge. Either way the container is gone,
    /// `container_id` is null, the registry entry is dropped and
    /// [`AppState::session_ended`] has run, which is what releases the tasks the
    /// session held (`ARCHITECTURE.md`, "Task tracker", Liveness). The session
    /// directory is kept until it is deleted.
    pub async fn end(&self, session_id: Uuid) -> Result<Session> {
        let repository = SessionRepository::new(&self.state.pool);
        let session = repository.get(session_id).await?;
        require_state(
            &session,
            &[
                SessionState::Creating,
                SessionState::Running,
                SessionState::Parked,
            ],
        )?;

        // The row after the cancelled launch has let go of the session; a
        // launch that reached `running` first is ended from there.
        let session = if session.state == SessionState::Creating {
            self.cancel_launch(&session).await?
        } else {
            session
        };

        if session.state == SessionState::Running {
            self.stop_and_wait(&session).await;
        }

        // Before the state change, so the `git` event is in the transcript
        // ahead of the `state_change` that closes the session, and outside any
        // transaction: the git lock always comes first (ADR 0021).
        self.fetch_back(&session).await;

        let ended = self.close(session_id).await?;
        let ended = self.discard_container(ended).await?;
        self.state.session_registry.remove(session_id);
        self.state.session_ended(session_id).await;

        info!(
            session_id = %session_id,
            state = ended.state.as_str(),
            "the session was ended by a user",
        );
        Ok(ended)
    }

    /// Retry a failed conversational session (`SPEC.md`, "Sessions", `POST
    /// /sessions/{id}/retry`).
    ///
    /// `failed → parked` clears `sessions.error`, which is the only edge out of
    /// `failed` there is; an ephemeral session is not retried and a new one is
    /// launched instead (ADR 0003). With a `message` the session is relaunched
    /// at once, through the same path a message to any parked session takes —
    /// the message queues and the resume delivers it — so a retry of a session
    /// that failed during `creating` gets the fresh sequence the launcher falls
    /// back to when there is no checkout to resume.
    pub async fn retry(
        &self,
        session_id: Uuid,
        message: Option<String>,
        user_id: Option<Uuid>,
    ) -> Result<Session> {
        let repository = SessionRepository::new(&self.state.pool);
        let session = repository.get(session_id).await?;
        session.can_retry()?;

        let mut tx = self.state.pool.begin().await?;
        let parked = repository
            .transition(
                &mut tx,
                session_id,
                &Transition::new(SessionState::Failed, SessionState::Parked, RETRIED_REASON),
            )
            .await?;
        tx.commit().await?;

        if let Some(text) = message {
            self.send_input(session_id, SessionInput::Message { text }, user_id, None)
                .await?;
        }

        info!(session_id = %session_id, "the session was retried by a user");
        Ok(parked)
    }

    /// Fetch the session branch into the project repository on request
    /// (`SPEC.md`, "Sessions", `POST /sessions/{id}/sync` → `{ref, commit}`).
    ///
    /// Allowed in every state but `creating`, which has no work tree yet;
    /// `running` included, where the fetch reads whatever is committed in the
    /// work tree at that moment. The lock, the fetch and the `git { op: "sync" }`
    /// event are [`GitService::sync_session`]'s, which is the same call the
    /// owner's end-of-run fetch-back and every other git operation's silent
    /// pre-sync go through (`ARCHITECTURE.md`, "Git model", Fetch-back).
    pub async fn sync(&self, session_id: Uuid) -> Result<SyncOutcome> {
        let session = SessionRepository::new(&self.state.pool)
            .get(session_id)
            .await?;
        if session.state == SessionState::Creating {
            return Err(Error::Conflict(format!("session is {}", session.state)));
        }

        GitService::from_state(&self.state)
            // The orchestrator does the fetch, with no credential and no
            // remote: `System` is what the audit and the event targets should
            // say, and a user id would change neither (`ARCHITECTURE.md`,
            // "Git model", Fetch-back).
            .sync_session(session.project_id, session_id, &GitActor::System)
            .await
    }

    /// Delete a session and everything outside Postgres that belongs to it
    /// (`SPEC.md`, "Sessions", `DELETE /sessions/{id}` → 204;
    /// `ARCHITECTURE.md`, "Storage").
    ///
    /// Only a `done` or `failed` session: a live one has to end first. The
    /// directory and the CLI transcript go before the row, so a failure leaves
    /// the row behind and the next delete retries rather than orphaning files
    /// nothing points at any more. `events` and `secret_uses` cascade with the
    /// row.
    ///
    /// The row itself goes through [`SessionRepository::delete`], which takes
    /// the project row lock first because the deletion's referential actions
    /// reach into the tracker's tables — the documented lock order, and the
    /// reason this passes the project id (`ARCHITECTURE.md`, "Task tracker" →
    /// "Lock order").
    pub async fn delete(&self, session_id: Uuid) -> Result<()> {
        let repository = SessionRepository::new(&self.state.pool);
        let session = repository.get(session_id).await?;
        session.can_delete()?;

        // A `done` or `failed` session should have none; one that is recorded
        // is a container a failed clean-up left behind.
        if let Some(container_id) = &session.container_id {
            self.remove_container(session_id, &ContainerId(container_id.clone()))
                .await?;
        }

        remove_dir_all(&session_dir(&self.state.config.data_dir, session_id)).await?;
        self.remove_cli_transcript(&session).await?;

        self.state.session_registry.remove(session_id);
        let mut tx = self.state.pool.begin().await?;
        let deleted = repository
            .delete(&mut tx, session.project_id, session_id)
            .await?;
        tx.commit().await?;

        if !deleted {
            // Somebody else deleted it while its files were being removed; the
            // caller asked for it to be gone and it is.
            debug!(session_id = %session_id, "the session row was already gone");
        }

        info!(session_id = %session_id, "the session was deleted");
        Ok(())
    }

    /// Cancel the launch of a session that is still `creating` and return the
    /// row as it reads once that launch has let go of it.
    ///
    /// The cancellation is a flag in the registry; the launch reads it at its
    /// checkpoints, stops where it is and removes the container if it created
    /// one. Waiting for it to release its claim is what this adds: until it
    /// does, it may still be about to create a container, and closing the
    /// session before then would be the leak this exists to prevent
    /// (`ARCHITECTURE.md`, "Session lifecycle", "A session ended while it is
    /// creating").
    ///
    /// A launch that is not answering within [`LAUNCH_CANCEL_TIMEOUT`] is
    /// logged and left: the end carries on, removes whatever container the row
    /// names, and the launch's own `creating → running` transition is then
    /// refused as a conflict, whose failure path removes what it created. No
    /// launch in progress at all — a `creating` row a crashed launch left
    /// behind — waits for nothing.
    async fn cancel_launch(&self, session: &Session) -> Result<Session> {
        let session_id = session.id;
        let repository = SessionRepository::new(&self.state.pool);

        if !self.state.session_registry.cancel_launch(session_id) {
            debug!(
                session_id = %session_id,
                "no launch is in progress for the session being ended",
            );
            return repository.get(session_id).await;
        }

        let deadline = tokio::time::Instant::now() + LAUNCH_CANCEL_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(EXIT_POLL_INTERVAL).await;

            if !self.state.session_registry.is_launching(session_id) {
                info!(
                    session_id = %session_id,
                    "the launch of the session being ended was cancelled",
                );
                return repository.get(session_id).await;
            }
        }

        warn!(
            session_id = %session_id,
            "the cancelled launch did not let go of the session in time; ending it anyway",
        );
        repository.get(session_id).await
    }

    /// Ask the owner to stop and wait until the session leaves `running`.
    ///
    /// Bounded by the stop's own grace period plus [`EXIT_GRACE_MARGIN`],
    /// because the owner escalates to `SIGTERM` at the end of that period and
    /// needs a moment after it to commit the transition. When the bound runs out
    /// the owner is not answering: the container is killed outright and removed,
    /// and `end` carries on with the state change, which is then the
    /// `running → done` edge.
    ///
    /// A registry that has no owner to ask is not an error here: a session whose
    /// row still says `running` after a restart that could not adopt it has
    /// nothing to stop, and `end` proceeds to the same clean-up.
    async fn stop_and_wait(&self, session: &Session) {
        let session_id = session.id;

        if self
            .state
            .session_registry
            .stop(session_id, StopReason::User)
            == StopOutcome::NoOwner
        {
            debug!(
                session_id = %session_id,
                "no live owner to stop; ending the session without one",
            );
            return;
        }

        let deadline = tokio::time::Instant::now()
            + Duration::from_secs(self.state.config.stop_grace_secs)
            + EXIT_GRACE_MARGIN;
        let repository = SessionRepository::new(&self.state.pool);

        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(EXIT_POLL_INTERVAL).await;

            match repository.find(session_id).await {
                Ok(Some(current)) if current.state != SessionState::Running => return,
                // Deleted under the end: nothing left to wait for.
                Ok(None) => return,
                Ok(Some(_)) => {}
                Err(err) => {
                    error!(
                        session_id = %session_id,
                        error = %err,
                        "could not read the session while waiting for its run to end",
                    );
                    return;
                }
            }
        }

        warn!(
            session_id = %session_id,
            "the session did not leave running within the stop grace period; killing it",
        );
        if let Some(container_id) = &session.container_id {
            let container_id = ContainerId(container_id.clone());
            if let Err(err) = self.state.engine.kill(&container_id, Signal::Sigkill).await {
                // A container that has already exited or gone answers Conflict
                // or NotFound; either way the process is what the kill was for.
                debug!(
                    session_id = %session_id,
                    error = %err,
                    "the session container could not be killed",
                );
            }
            if let Err(err) = self.remove_container(session_id, &container_id).await {
                warn!(
                    session_id = %session_id,
                    error = %err,
                    "the session container could not be removed after the kill",
                );
            }
        }
    }

    /// Fetch the session branch into the mirror and record the outcome.
    ///
    /// The whole of it is [`GitService::sync_session`]: the project git lock,
    /// the fetch and the `git { op: "sync" }` event, success or failure. A
    /// failure is logged and swallowed — the event is the user-visible half and
    /// `end` must still end the session (`ARCHITECTURE.md`, "Git model",
    /// Fetch-back).
    ///
    /// What that call refuses before it does anything at all — a project that is
    /// not `ready`, a session with no work tree and no ref of its own — it
    /// refuses without an event, because nothing happened to describe; those are
    /// the only failures an ending session sees no `git { ok: false }` for, and
    /// the log line is where an operator reads them.
    async fn fetch_back(&self, session: &Session) {
        let outcome = GitService::from_state(&self.state)
            .sync_session(session.project_id, session.id, &GitActor::System)
            .await;

        match outcome {
            Ok(synced) => debug!(
                session_id = %session.id,
                commit = %synced.commit,
                "the session branch was fetched back before the session ended",
            ),
            Err(err) => warn!(
                session_id = %session.id,
                error = %err,
                "the fetch-back before the end of the session failed",
            ),
        }
    }

    /// Move the session to `done`, or return it as it is when the run left it
    /// somewhere `done` cannot be reached from.
    ///
    /// Two attempts: the state read before the stop may be stale by now — the
    /// idle reaper parking the session is the documented race — so a conflict
    /// re-reads the row once and transitions from the state it actually has.
    /// `failed` and `done` are returned untouched; see the module
    /// documentation. `creating` is the session whose launch this `end` just
    /// cancelled, and takes the `creating → done` edge.
    async fn close(&self, session_id: Uuid) -> Result<Session> {
        let repository = SessionRepository::new(&self.state.pool);

        for attempt in 0..2 {
            let session = repository.get(session_id).await?;
            match session.state {
                SessionState::Creating | SessionState::Running | SessionState::Parked => {}
                // `done` is already there and `failed` has no edge into it.
                _ => return Ok(session),
            }

            let change = Transition::new(session.state, SessionState::Done, ENDED_REASON);
            let mut tx = self.state.pool.begin().await?;
            match repository.transition(&mut tx, session_id, &change).await {
                Ok(ended) => {
                    tx.commit().await?;
                    return Ok(ended);
                }
                // Somebody moved the session between the read and the lock; the
                // rolled-back transaction leaves nothing behind.
                Err(Error::Conflict(reason)) if attempt == 0 => debug!(
                    session_id = %session_id,
                    reason,
                    "the session changed state under the end; reading it again",
                ),
                Err(err) => return Err(err),
            }
        }

        repository.get(session_id).await
    }

    /// Leave no container behind an ended session, and no `container_id` on its
    /// row (`docs/data-model.md`, `sessions.container_id`).
    ///
    /// The owner's own clean-up does this when it ends a run; this is for the
    /// session that had no owner to do it — one ended from `parked` after a
    /// restart, or one whose owner the stop's grace period ran out on.
    async fn discard_container(&self, session: Session) -> Result<Session> {
        let Some(container_id) = session.container_id.clone() else {
            return Ok(session);
        };

        self.remove_container(session.id, &ContainerId(container_id))
            .await?;

        let repository = SessionRepository::new(&self.state.pool);
        let mut tx = self.state.pool.begin().await?;
        repository
            .set_container_id(&mut tx, session.id, None)
            .await?;
        tx.commit().await?;

        Ok(Session {
            container_id: None,
            ..session
        })
    }

    /// Remove one container, forced, with a container that is already gone
    /// counting as removed.
    async fn remove_container(&self, session_id: Uuid, container_id: &ContainerId) -> Result<()> {
        match self.state.engine.remove(container_id, true).await {
            Ok(()) | Err(EngineError::NotFound(_)) => {
                debug!(
                    session_id = %session_id,
                    container_id = %container_id,
                    "the session container is removed",
                );
                Ok(())
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Remove a deleted session's CLI transcript: `<cli_session_id>.jsonl` and
    /// any directory of the same name (`ARCHITECTURE.md`, "Storage").
    ///
    /// A session with no `cli_session_id` has no transcript, which is ordinary:
    /// a conversational session that was never sent a message never produced an
    /// `init` event (ADR 0032). The state directory itself is the project's and
    /// is never touched here.
    async fn remove_cli_transcript(&self, session: &Session) -> Result<()> {
        let Some(cli_session_id) = &session.cli_session_id else {
            return Ok(());
        };

        let layout = self.state.config.project_layout(session.project_id);
        let Some((transcript, directory)) = layout.cli_transcript_paths(cli_session_id) else {
            warn!(
                session_id = %session.id,
                "the recorded CLI session id is not a path component; leaving its transcript",
            );
            return Ok(());
        };

        remove_file(&transcript).await?;
        remove_dir_all(&directory).await?;

        Ok(())
    }
}

/// Refuse a session that is not in one of `allowed` with the documented
/// conflict.
///
/// `session is <state>` is what every lifecycle refusal says, in the session
/// model's own words ([`crate::models::SessionError::NotAcceptingInput`]), so
/// the message reads the same whichever check produced it.
fn require_state(session: &Session, allowed: &[SessionState]) -> Result<()> {
    if allowed.contains(&session.state) {
        return Ok(());
    }

    Err(Error::Conflict(format!("session is {}", session.state)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ProfileKind, SessionState};

    fn session(state: SessionState) -> Session {
        Session {
            id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            profile_id: Uuid::new_v4(),
            kind: ProfileKind::Conversational,
            created_by: None,
            title: None,
            task_id: None,
            handoff_id: None,
            state,
            base_ref: "refs/heads/main".to_string(),
            branch: "session/x".to_string(),
            container_id: None,
            cli_session_id: None,
            mcp_token_hash: "not-a-real-hash".to_string(),
            last_seq: 0,
            last_activity_at: Utc::now(),
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            error: None,
            created_at: Utc::now(),
            parked_at: None,
            ended_at: None,
        }
    }

    #[test]
    fn a_refused_state_answers_the_state_it_is_in() {
        let error = require_state(&session(SessionState::Parked), &[SessionState::Running])
            .expect_err("a parked session is not running");

        assert!(matches!(error, Error::Conflict(message) if message == "session is parked"));
    }

    #[test]
    fn an_allowed_state_passes() {
        require_state(
            &session(SessionState::Parked),
            &[SessionState::Running, SessionState::Parked],
        )
        .expect("parked is allowed");
    }
}
