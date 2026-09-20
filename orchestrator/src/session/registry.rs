//! The in-memory map from session id to the running owner's handle
//! (`ARCHITECTURE.md`, "Orchestrator internals", "Session owner task",
//! "Session lifecycle"; ADR 0020).
//!
//! Every path that wants to reach a session's process — the REST input
//! endpoint, the WebSocket handler, the launcher, the idle reaper and the
//! shutdown hook — goes through [`SessionRegistry`]. That is what makes the
//! CLI's stdin single-writer: the registry hands out no file descriptor, only
//! a bounded channel to the one task that owns the attached stdin
//! (`ARCHITECTURE.md`, "Session owner task").
//!
//! The registry holds four things per session: the channel to its owner, the
//! queue of inputs accepted while no owner is attached, the coarse phase that
//! decides between forwarding and queueing, and the session's
//! [`SessionKind`], so the WebSocket handler can answer an ephemeral session's
//! input without a database round-trip. It holds no pending-prompt cursor: the
//! pinned CLI never asks the host a question under the permission flags Mars
//! launches it with, so there is one input kind and nothing to answer
//! (ADR 0033; `SPEC.md`, "WebSocket: session stream").
//!
//! Nothing here is durable. A restart loses every queue, which v1 accepts
//! (`ARCHITECTURE.md`, "Input delivery across restarts"; ADR 0020).
//!
//! The registry decides deliverability only. Whether a session may take input
//! at all is the session model's rule — [`crate::models::Session::accepts_input`],
//! which is what refuses an ephemeral session and a `done` or `failed` one —
//! and the service applies it before calling [`SessionRegistry::submit`], or
//! [`SessionRegistry::submit_parked`] for a row that says `parked`, which a
//! restart may have left with no entry at all.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use tokio::time::Instant;
use uuid::Uuid;

use crate::events::SessionInput;
use crate::models::SessionKind;
use crate::prelude::*;
use crate::session::owner::StopReason;

/// How many commands the owner's channel buffers.
///
/// The owner reads its channel in the same loop that tails the transcript, so
/// this only has to cover a burst while it is busy translating a line. A
/// deeper buffer would not help: an owner that is not reading at all is an
/// owner whose session wants relaunching, not one whose inputs want hoarding.
const CHANNEL_CAPACITY: usize = 64;

/// How many inputs may wait for an owner that is not attached yet.
///
/// Waiting inputs are in memory and are lost on a restart (ADR 0020), so the
/// queue is a burst buffer for the seconds a launch takes, not a mailbox.
/// Beyond this the submission is refused rather than silently dropped.
const MAX_QUEUED: usize = 256;

/// The receiving half of one session owner's command channel.
///
/// [`SessionRegistry::register`] returns it and the spawned owner task takes
/// ownership of it; the registry keeps only the sender.
pub type OwnerRx = mpsc::Receiver<OwnerCommand>;

/// One accepted input, with who sent it and when it was accepted.
///
/// `user_id` is `None` for an input the orchestrator itself submits — a
/// session's launch prompt — and `client_id` is the client-generated string
/// echoed back in `input_accepted` so optimistic UI can reconcile (`SPEC.md`,
/// "WebSocket: session stream"). `accepted_at` is when the registry took it,
/// which is what the owner records on the `user_message` event.
#[derive(Debug, Clone)]
pub struct QueuedInput {
    /// What the user sent.
    pub input: SessionInput,
    /// The user who sent it, or `None` when the orchestrator did.
    pub user_id: Option<Uuid>,
    /// The client's own id for this input, echoed back on acknowledgement.
    pub client_id: Option<String>,
    /// When the registry accepted it.
    pub accepted_at: DateTime<Utc>,
}

/// What can be said to a session owner.
#[derive(Debug)]
pub enum OwnerCommand {
    /// Write this input to the CLI's stdin, recording a `user_message` event
    /// first (`ARCHITECTURE.md`, "Session owner task").
    Input(QueuedInput),
    /// Stop the session: SIGINT now, SIGTERM after the grace period
    /// (`ARCHITECTURE.md`, "Session lifecycle"). The reason is what the
    /// `state_change` the exit writes records.
    Stop {
        /// Who asked and what the exit should be recorded as.
        reason: StopReason,
    },
    /// Leave the loop without touching the container.
    ///
    /// Orchestrator shutdown and the tests use it: the container keeps
    /// running and is adopted again on the next start (`ARCHITECTURE.md`,
    /// "Restart procedure").
    Shutdown,
}

/// How far along a registered session is, as far as input delivery cares.
///
/// Coarser than [`crate::models::SessionState`] on purpose: `done` and
/// `failed` sessions have no entry at all, and the registry never consults the
/// database, so these three are the only distinctions it can make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Registered, no stdin yet: inputs queue (`ARCHITECTURE.md`, "Session
    /// lifecycle").
    Creating,
    /// An owner is attached to stdin: inputs are forwarded.
    Running,
    /// No process: inputs queue and the caller relaunches.
    Parked,
}

/// What became of a submitted input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitResult {
    /// Handed to the live owner.
    Forwarded,
    /// Waiting for the owner that is being launched or resumed.
    Queued,
    /// Queued, and the caller must call the launcher's resume: the session is
    /// parked and nothing is launching it (`ARCHITECTURE.md`, "Session
    /// lifecycle", `parked → running`).
    ParkedNeedsRelaunch,
    /// Refused, with the reason the caller puts in `input_rejected` or the
    /// HTTP error (`SPEC.md`, "WebSocket: session stream").
    Rejected(String),
}

/// What became of a stop request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// The owner took it and the signal sequence has begun.
    Sent,
    /// A stop was already accepted for this session and is still running its
    /// course; `since` is when the first one was taken, which is what lets the
    /// idle reaper decide a `SIGKILL` escalation is due. The first stop's
    /// reason stands.
    AlreadyStopping {
        /// When the first accepted stop was taken.
        since: Instant,
    },
    /// No live owner to ask: the session is not running, which is the 409 the
    /// REST stop endpoint answers.
    NoOwner,
}

/// One registered session.
#[derive(Debug)]
struct Entry {
    /// The session's kind, so the WebSocket handler can read it without a
    /// database round-trip. The registry itself never looks at it: the
    /// ephemeral rule belongs to the session model.
    kind: SessionKind,
    /// Which of the three input regimes applies.
    phase: Phase,
    /// The live owner's channel, `None` once the session is parked.
    tx: Option<mpsc::Sender<OwnerCommand>>,
    /// Inputs accepted while no owner was attached, oldest first.
    queue: VecDeque<QueuedInput>,
    /// When a stop was accepted for this session, cleared as soon as the owner
    /// leaves `running`. `Some` is what makes a second stop
    /// [`StopOutcome::AlreadyStopping`].
    stopping_since: Option<Instant>,
}

/// Everything behind the one mutex.
#[derive(Debug, Default)]
struct State {
    /// One entry per session that has been registered and not removed.
    sessions: HashMap<Uuid, Entry>,
    /// The sessions a launch or resume is in progress for.
    ///
    /// Separate from `sessions` because a launch guard is taken before the
    /// session is registered — that is the point of it — and a resume is taken
    /// for a parked session that a restart left with no entry at all.
    launching: HashSet<Uuid>,
}

/// The handles to the running session owner tasks (`ARCHITECTURE.md`,
/// "Orchestrator internals").
///
/// Cloning shares the map: the copy in [`AppState`] and the copy a spawned
/// launcher holds are the same registry. The map is behind a
/// `std::sync::Mutex` held only for a lookup and a `try_send`, never across an
/// `.await`.
#[derive(Debug, Clone, Default)]
pub struct SessionRegistry {
    /// The map and the launch flags.
    state: Arc<Mutex<State>>,
}

impl SessionRegistry {
    /// An empty registry; entries appear as sessions are launched.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a session about to be launched and return its owner's
    /// receiver.
    ///
    /// The channel is created here rather than by the owner so that an input
    /// arriving between `register` and the owner's first poll is buffered
    /// instead of refused. Re-registering a session that is already known —
    /// the resume of a parked session — keeps its queue, so inputs accepted
    /// while it was parked are still delivered when it reaches `Running`, and
    /// replaces its channel.
    pub fn register(&self, session_id: Uuid, kind: SessionKind, phase: Phase) -> OwnerRx {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let mut state = self.state();

        match state.sessions.get_mut(&session_id) {
            Some(entry) => {
                entry.kind = kind;
                entry.phase = phase;
                entry.tx = Some(tx);
                // A resumed session is a new run: whatever stop the previous
                // one was under is finished with it.
                entry.stopping_since = None;
            }
            None => {
                state.sessions.insert(
                    session_id,
                    Entry {
                        kind,
                        phase,
                        tx: Some(tx),
                        queue: VecDeque::new(),
                        stopping_since: None,
                    },
                );
            }
        }

        debug!(session_id = %session_id, phase = ?phase, "registered a session owner");
        rx
    }

    /// Switch a session to `Running` and take everything that queued while it
    /// was not.
    ///
    /// Called when stdin is attached, which is when the session becomes
    /// `running`: the pinned CLI writes nothing at all — `init` included —
    /// until it has read its first stdin line, so waiting for an event before
    /// flushing the queue would deadlock (ADR 0032; `ARCHITECTURE.md`, "Launch
    /// sequence"). The caller sends the returned inputs to the owner in the
    /// order they came back. An unknown id is a no-op returning nothing, which
    /// is what an owner adopted after a restart sees before it registers
    /// itself.
    pub fn mark_running(&self, session_id: Uuid) -> Vec<QueuedInput> {
        let mut state = self.state();
        let Some(entry) = state.sessions.get_mut(&session_id) else {
            return Vec::new();
        };

        entry.phase = Phase::Running;
        // A stop taken while the session was still `creating` is one the owner
        // ignored; left set, it would answer the first real stop with
        // `AlreadyStopping`.
        entry.stopping_since = None;
        let drained: Vec<QueuedInput> = entry.queue.drain(..).collect();

        if !drained.is_empty() {
            debug!(
                session_id = %session_id,
                queued = drained.len(),
                "delivering the inputs that queued before stdin was attached"
            );
        }
        drained
    }

    /// Mark a session parked: its owner is gone, its queue stays.
    ///
    /// Dropping the sender closes the channel, so a caller that raced this
    /// call and forwarded an input sees the send fail and requeues it. An
    /// unknown id is a no-op.
    pub fn mark_parked(&self, session_id: Uuid) {
        let mut state = self.state();
        if let Some(entry) = state.sessions.get_mut(&session_id) {
            entry.phase = Phase::Parked;
            entry.tx = None;
            entry.stopping_since = None;
            debug!(session_id = %session_id, "session owner detached; session parked");
        }
    }

    /// Forget a session entirely: it ended, failed or was deleted.
    ///
    /// Anything still queued is dropped. The count is logged, never the text
    /// (rule 3).
    pub fn remove(&self, session_id: Uuid) {
        let Some(entry) = self.state().sessions.remove(&session_id) else {
            return;
        };

        if !entry.queue.is_empty() {
            warn!(
                session_id = %session_id,
                dropped = entry.queue.len(),
                "dropping queued inputs for a session that ended"
            );
        }
    }

    /// Forget every session, the way a restart does.
    ///
    /// A new process starts with an empty registry, which is why the queues are
    /// not durable and why the inputs that were waiting in them are gone
    /// (ADR 0020; `ARCHITECTURE.md`, "Input delivery across restarts"). Nothing
    /// in a serving orchestrator calls this: it is what lets a test simulate a
    /// restart against the same database and data directory before it calls
    /// [`crate::session::recover`] again.
    pub fn clear(&self) {
        let forgotten = std::mem::take(&mut self.state().sessions);

        if !forgotten.is_empty() {
            debug!(
                sessions = forgotten.len(),
                "the session registry was cleared",
            );
        }
    }

    /// Offer one input to a session.
    ///
    /// The caller has already checked that the session may take input at all
    /// ([`crate::models::Session::accepts_input`]); this decides only where the
    /// input goes. See [`SubmitResult`] for the four answers; on
    /// [`SubmitResult::ParkedNeedsRelaunch`] the input is already queued and
    /// the caller resumes the session, which drains it.
    pub fn submit(&self, session_id: Uuid, input: QueuedInput) -> SubmitResult {
        submit_to(&mut self.state(), session_id, input)
    }

    /// Offer one input to a session whose row says `parked`, registering it
    /// parked first if the registry has forgotten it.
    ///
    /// Nothing is durable here, so a restart leaves every `parked` row without
    /// an entry (ADR 0020) and [`SessionRegistry::submit`] would answer
    /// `Rejected("session has no owner")` for a session that is perfectly
    /// resumable. The service calls this instead when the row it read says
    /// `parked`: registering, queueing and deciding whether a relaunch is
    /// needed happen under one acquisition of the mutex, so two messages
    /// arriving at once cannot both be told to relaunch. The entry is created
    /// with no channel and [`Phase::Parked`], which is exactly the state
    /// [`SessionRegistry::mark_parked`] would have left it in; the resume's
    /// [`SessionRegistry::register`] then keeps the queue this call filled.
    ///
    /// A session that *is* registered keeps its phase: a row that says `parked`
    /// while an owner is attached is a stale read, and the live entry is the
    /// truth about where the input goes.
    pub fn submit_parked(
        &self,
        session_id: Uuid,
        kind: SessionKind,
        input: QueuedInput,
    ) -> SubmitResult {
        let mut state = self.state();
        state.sessions.entry(session_id).or_insert_with(|| {
            debug!(session_id = %session_id, "registering a parked session the registry had lost");
            Entry {
                kind,
                phase: Phase::Parked,
                tx: None,
                queue: VecDeque::new(),
                stopping_since: None,
            }
        });

        submit_to(&mut state, session_id, input)
    }

    /// Ask a session's owner to stop: SIGINT now, SIGTERM after the grace
    /// period, and the reason recorded on the `state_change` the exit writes
    /// (`ARCHITECTURE.md`, "Stop semantics").
    ///
    /// The user's stop, the session service's `end` and the idle reaper all
    /// come through here, so the signals are sent by the one task that owns the
    /// container and no caller transitions a session behind its owner's back.
    /// See [`StopOutcome`] for the three answers; a second stop while one is
    /// still running its course is not forwarded, and the first stop's reason
    /// stands.
    pub fn stop(&self, session_id: Uuid, reason: StopReason) -> StopOutcome {
        let mut state = self.state();
        let Some(entry) = state.sessions.get_mut(&session_id) else {
            return StopOutcome::NoOwner;
        };

        if let Some(since) = entry.stopping_since {
            debug!(session_id = %session_id, "a stop of this session is already under way");
            return StopOutcome::AlreadyStopping { since };
        }

        let sent = entry
            .tx
            .as_ref()
            .is_some_and(|tx| tx.try_send(OwnerCommand::Stop { reason }).is_ok());

        if !sent {
            // A closed channel means the owner is already gone; park the entry
            // so nothing else tries to forward to it.
            if entry.tx.as_ref().is_some_and(mpsc::Sender::is_closed) {
                entry.phase = Phase::Parked;
                entry.tx = None;
            }
            return StopOutcome::NoOwner;
        }

        entry.stopping_since = Some(Instant::now());
        debug!(session_id = %session_id, ?reason, "asked the session owner to stop");
        StopOutcome::Sent
    }

    /// Whether a live owner is reachable for this session right now.
    pub fn is_live(&self, session_id: Uuid) -> bool {
        self.state()
            .sessions
            .get(&session_id)
            .and_then(|entry| entry.tx.as_ref())
            .is_some_and(|tx| !tx.is_closed())
    }

    /// The registered kind of a session, without a database round-trip.
    ///
    /// `None` when the session is not registered; the caller then reads the
    /// row, which it has to do anyway to know the session exists.
    pub fn kind(&self, session_id: Uuid) -> Option<SessionKind> {
        self.state()
            .sessions
            .get(&session_id)
            .map(|entry| entry.kind)
    }

    /// Claim the right to launch or resume one session.
    ///
    /// `None` while another launch of the same session holds the guard, which
    /// is what keeps a user message to a parked session and an explicit resume
    /// from starting two containers. The guard releases on drop, including
    /// when the launching task is cancelled or panics.
    pub fn try_begin_launch(&self, session_id: Uuid) -> Option<LaunchGuard> {
        if !self.state().launching.insert(session_id) {
            debug!(session_id = %session_id, "a launch of this session is already in progress");
            return None;
        }

        Some(LaunchGuard {
            session_id,
            state: Arc::clone(&self.state),
        })
    }

    /// Whether a launch or resume of this session is in progress. Test and
    /// diagnostic use only.
    pub fn is_launching(&self, session_id: Uuid) -> bool {
        self.state().launching.contains(&session_id)
    }

    /// The phase of a registered session. Test and diagnostic use only.
    pub fn phase(&self, session_id: Uuid) -> Option<Phase> {
        self.state()
            .sessions
            .get(&session_id)
            .map(|entry| entry.phase)
    }

    /// How many inputs are waiting for this session. Test and diagnostic use
    /// only.
    pub fn queued(&self, session_id: Uuid) -> usize {
        self.state()
            .sessions
            .get(&session_id)
            .map_or(0, |entry| entry.queue.len())
    }

    /// How many sessions have an entry. Test and diagnostic use only.
    pub fn tracked_sessions(&self) -> usize {
        self.state().sessions.len()
    }

    /// The state, recovering from a poisoned lock rather than propagating a
    /// panic. Nothing under this lock can panic — the critical sections are
    /// hash lookups, a `VecDeque` push and a non-blocking send — so a poisoned
    /// mutex means an unrelated failure, and refusing every input because of
    /// it would be worse than carrying on.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Where one input goes, with the registry's state already locked.
///
/// The body of [`SessionRegistry::submit`], factored out so that
/// [`SessionRegistry::submit_parked`] can register an entry and submit into it
/// without releasing the mutex in between.
fn submit_to(state: &mut State, session_id: Uuid, input: QueuedInput) -> SubmitResult {
    let relaunching = state.launching.contains(&session_id);
    let Some(entry) = state.sessions.get_mut(&session_id) else {
        return SubmitResult::Rejected("session has no owner".to_string());
    };

    match entry.phase {
        Phase::Running => {
            let Some(tx) = entry.tx.as_ref() else {
                // Running without a channel cannot happen — `register` is the
                // only way into this phase — but treat it like a closed one
                // rather than trusting the invariant.
                return park_and_queue(session_id, entry, input, relaunching);
            };

            match tx.try_send(OwnerCommand::Input(input)) {
                Ok(()) => {
                    debug!(session_id = %session_id, "input forwarded to the session owner");
                    SubmitResult::Forwarded
                }
                // The owner exited between the phase check and the send. Park
                // the entry so the next input queues too, and tell the caller
                // to relaunch.
                Err(mpsc::error::TrySendError::Closed(OwnerCommand::Input(input))) => {
                    park_and_queue(session_id, entry, input, relaunching)
                }
                // `try_send` hands back exactly what it was given, so this arm
                // is never taken; it answers rather than panics under the
                // registry's lock.
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    SubmitResult::Rejected("session has no owner".to_string())
                }
                // The owner is alive but behind. Queueing here would reorder
                // this input behind the next one that fits, so refuse it
                // instead and let the user resend.
                Err(mpsc::error::TrySendError::Full(_)) => {
                    warn!(
                        session_id = %session_id,
                        "the session owner's channel is full; input refused"
                    );
                    SubmitResult::Rejected("input queue full".to_string())
                }
            }
        }
        Phase::Creating => push(session_id, entry, input, SubmitResult::Queued),
        // A relaunch already under way will drain the queue; asking the caller
        // for a second one would race it.
        Phase::Parked => push(session_id, entry, input, queued_answer(relaunching)),
    }
}

/// Park an entry whose channel turned out to be closed and queue the input
/// that could not be sent, so the caller's relaunch delivers it.
fn park_and_queue(
    session_id: Uuid,
    entry: &mut Entry,
    input: QueuedInput,
    relaunching: bool,
) -> SubmitResult {
    debug!(
        session_id = %session_id,
        "the session owner is gone; queueing the input and parking the session"
    );
    entry.phase = Phase::Parked;
    entry.tx = None;
    entry.stopping_since = None;

    push(session_id, entry, input, queued_answer(relaunching))
}

/// What a parked session answers a queued input: the caller relaunches unless
/// somebody already is.
fn queued_answer(relaunching: bool) -> SubmitResult {
    if relaunching {
        SubmitResult::Queued
    } else {
        SubmitResult::ParkedNeedsRelaunch
    }
}

/// Append an input to an entry's queue and say what to answer.
///
/// `Queued` when it fits and `queued_answer` is what the caller wants said
/// instead, `Rejected("input queue full")` when the queue is at its bound.
/// Never logs the text (rule 3).
fn push(
    session_id: Uuid,
    entry: &mut Entry,
    input: QueuedInput,
    queued_answer: SubmitResult,
) -> SubmitResult {
    if entry.queue.len() >= MAX_QUEUED {
        warn!(
            session_id = %session_id,
            queued = entry.queue.len(),
            "the input queue is full; input refused"
        );
        return SubmitResult::Rejected("input queue full".to_string());
    }

    entry.queue.push_back(input);
    debug!(
        session_id = %session_id,
        queued = entry.queue.len(),
        "input queued until the session is running"
    );
    queued_answer
}

/// Proof that the holder is the one launching or resuming a session.
///
/// Dropping it lets the next launch in. It deliberately carries no permission
/// beyond that: the launcher still registers the session and marks it running
/// through the registry.
#[derive(Debug)]
pub struct LaunchGuard {
    /// The session being launched.
    session_id: Uuid,
    /// The registry's state, so the flag can be cleared on drop.
    state: Arc<Mutex<State>>,
}

impl LaunchGuard {
    /// The session this guard covers.
    ///
    /// Helpers that take the guard as proof of the claim read the id from it
    /// rather than from a second parameter, so the id and the claim cannot
    /// disagree.
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }
}

impl Drop for LaunchGuard {
    fn drop(&mut self) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .launching
            .remove(&self.session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> QueuedInput {
        QueuedInput {
            input: SessionInput::Message {
                text: text.to_string(),
            },
            user_id: Some(Uuid::new_v4()),
            client_id: Some(format!("client-{text}")),
            accepted_at: Utc::now(),
        }
    }

    /// The text of the input the owner received, whatever kind it is.
    fn received_text(command: OwnerCommand) -> String {
        match command {
            OwnerCommand::Input(queued) => queued.input.text().to_string(),
            other => panic!("expected an input command, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn inputs_queue_while_creating_and_drain_in_order_when_running() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();
        let _rx = registry.register(session, SessionKind::Conversational, Phase::Creating);

        assert_eq!(
            registry.submit(session, message("one")),
            SubmitResult::Queued
        );
        assert_eq!(
            registry.submit(session, message("two")),
            SubmitResult::Queued
        );
        assert_eq!(registry.queued(session), 2);

        let drained = registry.mark_running(session);
        let texts: Vec<&str> = drained.iter().map(|input| input.input.text()).collect();

        assert_eq!(texts, vec!["one", "two"], "FIFO, oldest first");
        assert_eq!(registry.queued(session), 0);
        assert_eq!(registry.phase(session), Some(Phase::Running));
    }

    #[tokio::test]
    async fn an_input_to_a_running_session_reaches_the_owner() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();
        let mut rx = registry.register(session, SessionKind::Conversational, Phase::Creating);
        assert!(registry.mark_running(session).is_empty());

        assert_eq!(
            registry.submit(session, message("hello")),
            SubmitResult::Forwarded
        );

        let command = rx.recv().await.expect("the owner receives the input");
        assert_eq!(received_text(command), "hello");
        assert!(registry.is_live(session));
    }

    #[tokio::test]
    async fn an_unknown_session_has_no_owner() {
        let registry = SessionRegistry::new();

        assert_eq!(
            registry.submit(Uuid::new_v4(), message("hello")),
            SubmitResult::Rejected("session has no owner".to_string()),
        );
        assert!(registry.mark_running(Uuid::new_v4()).is_empty());
        assert_eq!(registry.tracked_sessions(), 0);
    }

    #[tokio::test]
    async fn the_first_input_to_a_parked_session_asks_for_a_relaunch_and_the_second_queues() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();
        let rx = registry.register(session, SessionKind::Conversational, Phase::Creating);
        registry.mark_running(session);
        drop(rx);
        registry.mark_parked(session);

        assert_eq!(
            registry.submit(session, message("one")),
            SubmitResult::ParkedNeedsRelaunch,
            "the input is queued and the caller resumes the session"
        );

        // The caller now claims the resume; a second input must not ask for
        // another one.
        let guard = registry
            .try_begin_launch(session)
            .expect("no other launch is in progress");
        assert_eq!(
            registry.submit(session, message("two")),
            SubmitResult::Queued
        );

        // The resume registers the session again, keeping the queue, and the
        // drain hands both inputs over in order.
        let _rx = registry.register(session, SessionKind::Conversational, Phase::Creating);
        let drained = registry.mark_running(session);
        drop(guard);

        let texts: Vec<&str> = drained.iter().map(|input| input.input.text()).collect();
        assert_eq!(texts, vec!["one", "two"]);
    }

    #[tokio::test]
    async fn a_closed_owner_channel_parks_the_session_instead_of_forwarding() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();
        let rx = registry.register(session, SessionKind::Conversational, Phase::Creating);
        registry.mark_running(session);

        // The owner exits without anybody telling the registry.
        drop(rx);
        assert!(!registry.is_live(session));

        assert_eq!(
            registry.submit(session, message("hello")),
            SubmitResult::ParkedNeedsRelaunch,
        );
        assert_eq!(registry.phase(session), Some(Phase::Parked));
        assert_eq!(registry.queued(session), 1, "the input is not lost");
    }

    /// The restart case: the row says `parked`, the registry knows nothing, and
    /// the input must queue and ask for a relaunch rather than be refused
    /// (ADR 0020).
    #[tokio::test]
    async fn an_input_to_a_parked_session_the_registry_lost_registers_it_and_asks_for_a_relaunch() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();

        assert_eq!(
            registry.submit(session, message("one")),
            SubmitResult::Rejected("session has no owner".to_string()),
            "without the row's state there is nothing to register",
        );

        assert_eq!(
            registry.submit_parked(session, SessionKind::Conversational, message("one")),
            SubmitResult::ParkedNeedsRelaunch,
        );
        assert_eq!(registry.phase(session), Some(Phase::Parked));
        assert_eq!(registry.kind(session), Some(SessionKind::Conversational));
        assert!(!registry.is_live(session));

        // The second message finds the entry this one made; only the resume
        // that is already claimed keeps it from asking for another.
        let guard = registry
            .try_begin_launch(session)
            .expect("no other launch is in progress");
        assert_eq!(
            registry.submit_parked(session, SessionKind::Conversational, message("two")),
            SubmitResult::Queued,
        );

        let _rx = registry.register(session, SessionKind::Conversational, Phase::Creating);
        let drained = registry.mark_running(session);
        drop(guard);

        let texts: Vec<&str> = drained.iter().map(|input| input.input.text()).collect();
        assert_eq!(texts, vec!["one", "two"], "the queue survived the resume");
    }

    /// A live owner wins over a row that says `parked`: the read was stale.
    #[tokio::test]
    async fn submit_parked_leaves_a_registered_session_alone() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();
        let mut rx = registry.register(session, SessionKind::Conversational, Phase::Creating);
        registry.mark_running(session);

        assert_eq!(
            registry.submit_parked(session, SessionKind::Conversational, message("hello")),
            SubmitResult::Forwarded,
        );
        assert_eq!(registry.phase(session), Some(Phase::Running));
        assert_eq!(
            received_text(rx.recv().await.expect("the owner receives the input")),
            "hello",
        );
    }

    #[tokio::test]
    async fn a_full_queue_refuses_further_inputs() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();
        let _rx = registry.register(session, SessionKind::Conversational, Phase::Creating);

        for index in 0..MAX_QUEUED {
            assert_eq!(
                registry.submit(session, message(&index.to_string())),
                SubmitResult::Queued,
            );
        }

        assert_eq!(
            registry.submit(session, message("one too many")),
            SubmitResult::Rejected("input queue full".to_string()),
        );
        assert_eq!(registry.queued(session), MAX_QUEUED);
    }

    #[tokio::test]
    async fn a_full_owner_channel_refuses_rather_than_reordering() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();
        let _rx = registry.register(session, SessionKind::Conversational, Phase::Creating);
        registry.mark_running(session);

        // The owner never polls, so the channel fills at its capacity.
        for index in 0..CHANNEL_CAPACITY {
            assert_eq!(
                registry.submit(session, message(&index.to_string())),
                SubmitResult::Forwarded,
            );
        }

        assert_eq!(
            registry.submit(session, message("one too many")),
            SubmitResult::Rejected("input queue full".to_string()),
        );
    }

    #[tokio::test]
    async fn stop_reaches_a_live_owner_and_reports_no_owner_without_one() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();
        let mut rx = registry.register(session, SessionKind::Conversational, Phase::Creating);
        registry.mark_running(session);

        assert_eq!(
            registry.stop(session, StopReason::User),
            StopOutcome::Sent,
            "a live owner could not be stopped",
        );
        assert!(matches!(
            rx.recv().await,
            Some(OwnerCommand::Stop {
                reason: StopReason::User
            })
        ));

        registry.mark_parked(session);
        assert_eq!(
            registry.stop(session, StopReason::User),
            StopOutcome::NoOwner,
            "a parked session has no owner to stop",
        );

        assert_eq!(
            registry.stop(Uuid::new_v4(), StopReason::User),
            StopOutcome::NoOwner,
            "an unknown session has no owner either",
        );
    }

    /// The second stop is not forwarded: the first one's grace period is
    /// already running and its reason is the one the exit records. `since` is
    /// what the idle reaper measures a `SIGKILL` escalation against.
    #[tokio::test]
    async fn a_second_stop_reports_when_the_first_was_taken() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();
        let mut rx = registry.register(session, SessionKind::Conversational, Phase::Creating);
        registry.mark_running(session);

        let before = Instant::now();
        assert_eq!(registry.stop(session, StopReason::Idle), StopOutcome::Sent);
        let after = Instant::now();

        let StopOutcome::AlreadyStopping { since } = registry.stop(session, StopReason::User)
        else {
            panic!("a second stop was forwarded to the owner");
        };
        assert!((before..=after).contains(&since));

        assert!(matches!(
            rx.recv().await,
            Some(OwnerCommand::Stop {
                reason: StopReason::Idle
            })
        ));
        assert!(
            rx.try_recv().is_err(),
            "the second stop reached the owner too",
        );

        // Leaving `running` clears it, so the next run's stop is a first one.
        registry.mark_parked(session);
        let _rx = registry.register(session, SessionKind::Conversational, Phase::Running);
        assert_eq!(registry.stop(session, StopReason::User), StopOutcome::Sent);
    }

    #[tokio::test]
    async fn stop_on_a_closed_channel_reports_no_owner_and_parks_the_entry() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();
        let rx = registry.register(session, SessionKind::Conversational, Phase::Creating);
        registry.mark_running(session);
        drop(rx);

        assert_eq!(
            registry.stop(session, StopReason::User),
            StopOutcome::NoOwner,
            "an exited owner was stopped",
        );
        assert_eq!(registry.phase(session), Some(Phase::Parked));
    }

    #[tokio::test]
    async fn only_one_launch_of_a_session_runs_at_a_time() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();

        let guard = registry
            .try_begin_launch(session)
            .expect("the first claim wins");
        assert_eq!(guard.session_id(), session);
        assert!(registry.is_launching(session));
        assert!(
            registry.try_begin_launch(session).is_none(),
            "a second concurrent launch must be refused"
        );

        // Another session is unaffected.
        let other = registry
            .try_begin_launch(Uuid::new_v4())
            .expect("launches of different sessions are independent");

        drop(guard);
        assert!(!registry.is_launching(session));
        assert!(
            registry.try_begin_launch(session).is_some(),
            "the guard releases the claim on drop"
        );
        drop(other);
    }

    #[tokio::test]
    async fn remove_forgets_the_entry_and_its_queue() {
        let registry = SessionRegistry::new();
        let session = Uuid::new_v4();
        let _rx = registry.register(session, SessionKind::Ephemeral, Phase::Creating);
        registry.submit(session, message("lost"));

        assert_eq!(registry.kind(session), Some(SessionKind::Ephemeral));
        assert_eq!(registry.tracked_sessions(), 1);

        registry.remove(session);

        assert_eq!(registry.tracked_sessions(), 0);
        assert_eq!(registry.kind(session), None);
        assert!(!registry.is_live(session));
        // Removing twice is harmless.
        registry.remove(session);
    }

    #[tokio::test]
    async fn a_clone_shares_the_map() {
        let registry = SessionRegistry::new();
        let clone = registry.clone();
        let session = Uuid::new_v4();

        let _rx = clone.register(session, SessionKind::Conversational, Phase::Creating);

        assert_eq!(registry.tracked_sessions(), 1);
        assert_eq!(
            registry.submit(session, message("one")),
            SubmitResult::Queued
        );
        assert_eq!(clone.queued(session), 1);
    }
}
