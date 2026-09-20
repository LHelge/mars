//! The `SessionOwner` task: the one reader of a session's transcript and the
//! one writer of its CLI's stdin (`ARCHITECTURE.md`, "Session owner task").
//!
//! The read side of that loop tails `log/stream.jsonl` from the offset the
//! database says was last committed, hands every *complete* native line to the
//! backend adapter, and commits that line's translated events, its end offset
//! and the counters it moved in one transaction — so either the whole line is
//! visible with its offset advanced or none of it is (`ARCHITECTURE.md`,
//! "Durability and recovery"; ADR 0010, 0021, 0028).
//!
//! Three rules shape everything below.
//!
//! **No in-memory sequence counter.** Sequences come from the table under the
//! session row lock, inside [`SessionRepository::append_native_line`]. The
//! owner's only memory of the database is the offset it believes is stored,
//! which that same call rechecks under the lock; a mismatch means a second
//! tailer exists and this owner stands down rather than double-appending.
//!
//! **Translation state belongs to the process, not to the task.** A fresh
//! launch, a resume and a retry all start with a fresh [`TranslateState`]; an
//! owner that *adopts* an already-running process after a restart rebuilds that
//! process's open subagents, denial bookkeeping, input-echo hashes and
//! cumulative cost baseline by replaying the committed part of its transcript,
//! publishing nothing (`ARCHITECTURE.md`, "Durability and recovery").
//!
//! **`running` is not this module's decision.** The pinned CLI writes nothing
//! at all, `system`/`init` included, until it has read a line of stdin, so the
//! launcher transitions the session when it attaches stdin and `init` only
//! records `cli_session_id` and checks the MCP server (ADR 0032;
//! `ARCHITECTURE.md`, "Launch sequence").
//!
//! The write and exit sides are here too. Inputs are recorded as
//! `user_message` before anything is written, so history shows what the user
//! sent even when the write fails and is not retried (ADR 0020). A stop is
//! `SIGINT`, then `SIGTERM` after `STOP_GRACE_SECS`, with the last signal sent
//! recorded on the `state_change` so the UI can say stopped rather than killed.
//! The signals are the same for every [`StopReason`] and only the reason the
//! exit records differs, which is how the idle reaper parks a conversational
//! session and fails an ephemeral one as `stalled` through the owner rather
//! than behind its back.
//! A container exit is drained to end of file first — the CLI's last `result`
//! is written just before it goes — and then read as `parked` or `failed` by
//! the rules in `ARCHITECTURE.md`, "Session lifecycle" and "Stop semantics".
//! An ephemeral session's `result` ends the run outright: fetch-back, stop,
//! `done`, never parked and never resumed ("Claude Code invocation").

use std::collections::VecDeque;
use std::io::{ErrorKind, SeekFrom};
use std::ops::ControlFlow;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use uuid::Uuid;

use crate::agent::{AgentBackend, TranslateConfig, TranslateState};
use crate::engine::{ContainerEngine, ContainerId, EngineError, ExitStatus, Signal};
use crate::events::{AgentEvent, AgentEventBody, GitOp, McpServerStatus, StopSignal};
use crate::git::{GitService, refs};
use crate::models::{GitSyncDetail, NewEvent, SessionKind, SessionState};
use crate::prelude::*;
use crate::repositories::{CostDelta, SessionRepository, Transition};
use crate::session::{OwnerCommand, OwnerRx, QueuedInput, SessionDirs};

/// How often the owner looks for new transcript bytes.
///
/// A plain interval rather than an inotify watch: the transcript is appended to
/// by a container on the same filesystem, one owner exists per running session,
/// and 100 ms of latency is invisible next to a model turn. It also keeps the
/// loop portable across the bind-mount and volume layouts the engines use
/// (`ARCHITECTURE.md`, "Session container specification").
pub const TAIL_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How many bytes one read of the transcript asks for.
const READ_CHUNK: usize = 64 * 1024;

/// How long the drain that follows a container exit waits between reads.
const DRAIN_SETTLE: Duration = Duration::from_millis(100);

/// How many consecutive empty reads mean the transcript has stopped growing.
///
/// Two, not one: the CLI writes its `result` and exits in the same breath, so a
/// single empty read can land between the exit the engine reported and the last
/// bytes reaching the file.
const DRAIN_EMPTY_READS: usize = 2;

/// How long the drain keeps going before the transition happens anyway.
///
/// A transcript that is still growing after this is something other than the
/// process that just exited, and the session's state matters more than the tail
/// of its output.
const DRAIN_LIMIT: Duration = Duration::from_secs(10);

/// The first backoff of a `wait` the engine could not answer.
const WAIT_RETRY_START: Duration = Duration::from_secs(1);

/// The cap that backoff doubles up to.
const WAIT_RETRY_MAX: Duration = Duration::from_secs(30);

/// The MCP server every session container is configured with
/// ([`crate::session::write_mcp_json`]; `ARCHITECTURE.md`, "MCP design").
///
/// The `init` event must list it as connected; anything else is a degraded
/// session and earns a `launch_warning`.
pub const MARS_MCP_SERVER: &str = "mars-orchestrator";

/// The status the warning names when the `init` event does not mention the
/// server at all.
const MISSING_STATUS: &str = "missing";

/// How a backend reports what a turn cost (`ARCHITECTURE.md`, "Cost
/// accounting").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostAccounting {
    /// Every `result` reports what its own turn cost, so the value *is* the
    /// delta.
    PerTurn,
    /// Every `result` reports the process's running total, so the delta is the
    /// increase over the previous `result` of the same process.
    Cumulative,
}

/// Which rule the owner accumulates `result.cost_usd` under.
///
/// `Cumulative`, and not a guess: the pinned CLI's three recorded turns
/// reported 0.0727, 0.1448 and 0.1633 USD and the probe's two-turn run reported
/// 0.0412 then 0.0471 for two turns that each said `num_turns: 1`
/// (`ARCHITECTURE.md`, "Cost accounting", the increase-over-previous rule;
/// `tests/fixtures/claude/2.1.274/NOTES.md`, `multi_turn`). `usage` is per turn
/// under either rule and is always summed as it arrives.
pub const COST_ACCOUNTING: CostAccounting = CostAccounting::Cumulative;

/// Everything one owner task needs to run a session's loop.
///
/// Built by the launcher, which owns the container, the directories and the
/// profile; the owner reads none of those for itself.
pub struct OwnerContext {
    /// The session being owned.
    pub session_id: Uuid,
    /// Conversational or ephemeral, which is what a `result` and an exit mean
    /// different things to: an ephemeral session ends on its `result` and never
    /// takes input, a conversational one parks and waits for the next message.
    pub kind: SessionKind,
    /// Where this session's transcript lives.
    pub dirs: SessionDirs,
    /// The adapter that translates the CLI's output and encodes its input.
    pub backend: Arc<dyn AgentBackend>,
    /// The byte offset tailing starts at: `MAX(_offset)` of the session's
    /// events, `0` for a transcript nothing has been committed from.
    pub start_offset: u64,
    /// What the launch knows before its first output line: whether it resumed,
    /// whether partial messages were asked for and which credential was
    /// injected. Carries the `resumed` flag the `init` event reports.
    pub translate: TranslateConfig,
    /// True when this owner took over a process that was already running — an
    /// orchestrator restart — rather than launching one. Only an adoption
    /// reconstructs translation state; an actual launch starts fresh
    /// (`ARCHITECTURE.md`, "Durability and recovery").
    pub adopted: bool,
    /// The container's attached stdin, when one is attached.
    ///
    /// `None` is a session whose attach failed: it still tails and still
    /// records its inputs, which is the difference between a degraded session
    /// and a lost one.
    pub stdin: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    /// The engine's id for the container the owner watches for an exit, kills on
    /// a stop and removes once the session leaves `running`.
    pub container_id: Option<ContainerId>,
    /// The channel [`crate::session::SessionRegistry::register`] handed out.
    pub commands: OwnerRx,
    /// The pool, the registry and the engine.
    pub state: AppState,
}

/// One native line, translated and ready to commit.
///
/// Held across a failed commit so the retry writes exactly the same rows rather
/// than translating the line a second time against a state the first attempt
/// already moved.
#[derive(Debug)]
struct TranslatedLine {
    /// The events to append, in order, including any `launch_warning` the
    /// `init` rule added.
    events: Vec<AgentEvent>,
    /// The byte offset just past the line, which the last event carries.
    end_offset: u64,
    /// What this line added to the session's counters, when it carried a
    /// `result`.
    cost: Option<CostDelta>,
    /// The id an `init` in this line announced, stored in the same transaction.
    cli_session_id: Option<String>,
    /// The end-of-run summary, when this line carried a `result`.
    result: Option<ResultSummary>,
}

/// What the owner's end-of-run hook is told about a `result`.
///
/// `terminal_reason` is the field that tells a turn the user stopped from a turn
/// that failed (`ARCHITECTURE.md`, "Stop semantics"), which is why the summary
/// carries it rather than only `is_error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultSummary {
    pub subtype: String,
    pub terminal_reason: Option<String>,
    pub is_error: bool,
}

/// Why a session was asked to stop, which is what the `state_change` the exit
/// writes says (`ARCHITECTURE.md`, "Stop semantics").
///
/// The signal sequence is the same for all three — `SIGINT`, then `SIGTERM`
/// after `STOP_GRACE_SECS` — and only the transition the exit leads to differs:
/// a user's stop and an idle conversational session are `parked`, an idle
/// ephemeral one is `failed` with `sessions.error = "stalled"`, because an
/// ephemeral session is never parked, resumed or retried (ADR 0003;
/// `ARCHITECTURE.md`, "Session lifecycle").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Somebody asked for it: `POST /sessions/{id}/stop`, `end`, the WebSocket.
    User,
    /// The idle reaper gave up on a conversational session that went quiet for
    /// longer than its profile's `idle_timeout_secs`.
    Idle,
    /// The idle reaper gave up on an *ephemeral* session that went quiet: the
    /// same silence, but a run that produced nothing (`ARCHITECTURE.md`, "Task
    /// tracker", "Liveness comes from the session").
    Stalled,
}

impl StopReason {
    /// The reason recorded on the `state_change` the exit writes, and — for
    /// [`StopReason::Stalled`] — in `sessions.error`.
    fn recorded(self) -> &'static str {
        match self {
            Self::User => "stopped by user",
            Self::Idle => "idle timeout",
            Self::Stalled => "stalled",
        }
    }

    /// The reason as it applies to a session of this kind.
    ///
    /// An ephemeral session can never end `parked`, so an idle one is stalled;
    /// a conversational session is never failed by idleness, so a stalled one
    /// is merely idle. Both mismatches are a caller's mistake — the idle reaper
    /// reads the kind before it sends — and are corrected rather than obeyed.
    fn for_kind(self, kind: SessionKind, session_id: Uuid) -> Self {
        match (self, kind) {
            (Self::Idle, SessionKind::Ephemeral) => {
                warn!(
                    session_id = %session_id,
                    "an idle stop reached an ephemeral session; treating it as stalled",
                );
                Self::Stalled
            }
            (Self::Stalled, SessionKind::Conversational) => {
                warn!(
                    session_id = %session_id,
                    "a stalled stop reached a conversational session; treating it as idle",
                );
                Self::Idle
            }
            (reason, _) => reason,
        }
    }
}

/// Why an owner's loop returned, and what that means for the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerExit {
    /// The owner was asked to stand down, was replaced, or could not finish what
    /// it started. The session row and the registry entry belong to somebody
    /// else — the orchestrator's shutdown hook, or a newer owner — so neither is
    /// touched.
    StoodDown,
    /// The session left `running` and this owner did the whole clean-up for it.
    Ended(SessionState),
}

/// What the container watch saw.
#[derive(Debug, Clone, Copy)]
enum ContainerExit {
    /// It exited, with this status.
    Exited(ExitStatus),
    /// The engine no longer has it at all.
    Gone,
}

/// The transition one container exit calls for, worked out before any of it is
/// written.
///
/// Owned strings rather than borrows: the reason names an exit code, and
/// [`Transition`] borrows what it is given.
#[derive(Debug)]
struct ExitPlan {
    to: SessionState,
    reason: String,
    signal: Option<StopSignal>,
    error: Option<String>,
}

/// The owner of one session's transcript and stdin.
pub struct SessionOwner {
    session_id: Uuid,
    kind: SessionKind,
    dirs: SessionDirs,
    backend: Arc<dyn AgentBackend>,
    state: AppState,
    stdin: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    container_id: Option<ContainerId>,
    /// Set once a `Stop` was taken, whether or not the signal reached the
    /// container: it is what makes the exit that follows a stop rather than a
    /// failure, and which transition it leads to (`ARCHITECTURE.md`, "Stop
    /// semantics"). Already corrected for the session's kind.
    stop_reason: Option<StopReason>,
    /// The last signal actually delivered, which the `state_change` carries.
    stop_signal: Option<StopSignal>,
    /// When the grace period after `SIGINT` runs out and `SIGTERM` follows.
    stop_deadline: Option<Instant>,
    /// Whether a `result` of this process has been committed, which is what
    /// tells an ephemeral session that ran from one that died first.
    result_seen: bool,
    /// The command channel, taken out of the struct by
    /// [`SessionOwner::run`] so the loop can borrow it independently of the
    /// rest of the owner.
    commands: Option<OwnerRx>,
    adopted: bool,
    /// This process's translation memory.
    translate: TranslateState,
    /// The transcript, opened on the first tail step and kept open.
    file: Option<File>,
    /// The byte offset just past everything read into [`SessionOwner::buffer`].
    read_offset: u64,
    /// Bytes read that do not yet form a complete line.
    buffer: Vec<u8>,
    /// Complete lines read and not yet committed, oldest first.
    queue: VecDeque<(String, u64)>,
    /// The offset the owner believes the database holds; only a successful
    /// commit moves it.
    committed_offset: u64,
    /// True while the transcript is shorter than [`Self::committed_offset`], so
    /// nothing it holds may be committed and the `error` event announcing that
    /// has already been written (`ARCHITECTURE.md`, "Durability and recovery").
    below_committed: bool,
    /// A translated line whose commit failed, retried before any newer line.
    pending: Option<TranslatedLine>,
    /// The `cost_usd` of the previous `result` of this process, the baseline the
    /// cumulative rule subtracts.
    last_result_cost: Option<f64>,
}

impl SessionOwner {
    /// Run a session's loop on its own task and leave the registry as the
    /// session's new state asks for.
    ///
    /// A `parked` session keeps its entry — that is where inputs queue until a
    /// resume drains them — and only its channel goes, which is exactly
    /// [`SessionRegistry::mark_parked`](crate::session::SessionRegistry::mark_parked).
    /// A `done` or `failed` session has nothing more to say to and is forgotten.
    /// An owner that stood down touches neither, because the entry it would
    /// reach for is a newer owner's.
    pub fn spawn(ctx: OwnerContext) -> JoinHandle<()> {
        let session_id = ctx.session_id;
        let registry = ctx.state.session_registry.clone();

        tokio::spawn(async move {
            match SessionOwner::new(ctx).run().await {
                OwnerExit::StoodDown => {}
                OwnerExit::Ended(SessionState::Parked) => registry.mark_parked(session_id),
                OwnerExit::Ended(_) => registry.remove(session_id),
            }
        })
    }

    /// The owner as it is before its first tail step.
    ///
    /// Public so a test can drive [`SessionOwner::read_available_lines`] without
    /// a loop; production code goes through [`SessionOwner::spawn`].
    pub fn new(ctx: OwnerContext) -> Self {
        Self {
            session_id: ctx.session_id,
            kind: ctx.kind,
            dirs: ctx.dirs,
            backend: ctx.backend,
            state: ctx.state,
            stdin: ctx.stdin,
            container_id: ctx.container_id,
            commands: Some(ctx.commands),
            stop_reason: None,
            stop_signal: None,
            stop_deadline: None,
            result_seen: false,
            adopted: ctx.adopted,
            translate: TranslateState::new(ctx.translate),
            file: None,
            read_offset: ctx.start_offset,
            buffer: Vec::new(),
            queue: VecDeque::new(),
            committed_offset: ctx.start_offset,
            below_committed: false,
            pending: None,
            last_result_cost: None,
        }
    }

    /// Tail, take commands, watch the container, and leave when the session
    /// stops being this owner's.
    async fn run(mut self) -> OwnerExit {
        // Out of the struct, so the `recv()` future in the loop's `select!`
        // borrows the channel and the tail step borrows everything else.
        let Some(mut commands) = self.commands.take() else {
            error!(session_id = %self.session_id, "a session owner was run twice");
            return OwnerExit::StoodDown;
        };

        if self.adopted {
            // A failed reconstruction is not a reason to refuse to tail: the
            // worst case is a delayed `subagent_end` that finds no open call
            // and an echo that becomes `raw`, both of which are visible in the
            // transcript rather than silent corruption.
            if let Err(err) = self.restore_translation_state().await {
                error!(
                    session_id = %self.session_id,
                    error = %err,
                    "could not reconstruct the adopted process's translation state",
                );
            }
        }

        let mut tail = tokio::time::interval(TAIL_POLL_INTERVAL);
        tail.set_missed_tick_behavior(MissedTickBehavior::Delay);

        // The exit watch is a task of its own rather than a `select!` branch
        // calling `wait` directly: `select!` drops the losing futures on every
        // tick, which would mean a fresh `wait` request to the engine ten times
        // a second. The task waits once, retries a transport failure with
        // backoff, and hands over the one answer it gets.
        let mut exit_watch = self.container_id.clone().map(|container_id| {
            watch_container(
                Arc::clone(&self.state.engine),
                container_id,
                self.session_id,
            )
        });

        let exit = loop {
            // Read before the futures are built: the `select!` arms below must
            // not all borrow `self`.
            let stop_deadline = self.stop_deadline;

            tokio::select! {
                _ = tail.tick() => {
                    if let ControlFlow::Break(exit) = self.tail_step().await {
                        break exit;
                    }
                }
                command = commands.recv() => {
                    match command {
                        Some(OwnerCommand::Input(input)) => self.deliver_input(input).await,
                        Some(OwnerCommand::Stop { reason }) => self.request_stop(reason).await,
                        Some(OwnerCommand::Shutdown) => {
                            debug!(session_id = %self.session_id, "session owner asked to stand down");
                            break OwnerExit::StoodDown;
                        }
                        // Every sender is gone, which only happens once the
                        // registry entry was replaced or removed.
                        None => break OwnerExit::StoodDown,
                    }
                }
                observed = next_exit(&mut exit_watch) => {
                    break self.on_container_exit(observed).await;
                }
                () = elapsed(stop_deadline) => self.escalate_stop().await,
            }
        };

        // A closed channel means the registry entry this owner was registered
        // under is gone or belongs to a newer owner, so the clean-up in `spawn`
        // would park or forget somebody else's session.
        let exit = if commands.is_closed() && exit != OwnerExit::StoodDown {
            debug!(
                session_id = %self.session_id,
                "this owner's registry entry is gone; leaving the registry alone",
            );
            OwnerExit::StoodDown
        } else {
            exit
        };

        debug!(
            session_id = %self.session_id,
            committed_offset = self.committed_offset,
            ?exit,
            "session owner finished",
        );

        exit
    }

    /// One pass: read whatever the CLI has written, then commit it line by line.
    async fn tail_step(&mut self) -> ControlFlow<OwnerExit> {
        match self.read_available_lines().await {
            Ok(lines) => self.queue.extend(lines),
            // Logged at the source, with the path; a transient read failure is
            // retried on the next tick from the same offset.
            Err(_) => return ControlFlow::Continue(()),
        }

        self.commit_queued().await
    }

    /// Commit the queued lines, retrying a batch a previous pass could not
    /// write before translating any newer line.
    async fn commit_queued(&mut self) -> ControlFlow<OwnerExit> {
        loop {
            let batch = match self.pending.take() {
                Some(batch) => batch,
                None => {
                    let Some((line, end_offset)) = self.queue.pop_front() else {
                        return ControlFlow::Continue(());
                    };
                    match self.translate_line(&line, end_offset) {
                        Some(batch) => batch,
                        // Nothing to commit: the line only moved the
                        // translator's state, so the database keeps the
                        // previous offset and the line is re-read harmlessly
                        // after a restart.
                        None => continue,
                    }
                }
            };

            match self.commit_line(&batch).await {
                Ok(()) => {
                    if let Some(result) = batch.result.clone() {
                        self.on_result(&result).await?;
                    }
                }
                Err(Error::Conflict(reason)) => {
                    // Another tailer of the same transcript owns this session's
                    // events. Standing down is the only safe answer: appending
                    // again would duplicate its work, and transitioning the
                    // session would fight the owner that is actually attached.
                    error!(
                        session_id = %self.session_id,
                        reason = %reason,
                        "the transcript offset moved under this owner; standing down",
                    );
                    return ControlFlow::Break(OwnerExit::StoodDown);
                }
                Err(err) => {
                    error!(
                        session_id = %self.session_id,
                        error = %err,
                        end_offset = batch.end_offset,
                        "could not commit a transcript line; retrying",
                    );
                    self.pending = Some(batch);
                    return ControlFlow::Continue(());
                }
            }
        }
    }

    /// Read whatever bytes the transcript has gained and return the complete
    /// lines in them, each with the byte offset just past it.
    ///
    /// A trailing partial line stays in the buffer: only a line terminated by
    /// `\n` is a line the CLI finished writing. Blank lines are skipped
    /// entirely rather than translated, so a transcript that ends in a newline
    /// produces no event.
    pub async fn read_available_lines(&mut self) -> Result<Vec<(String, u64)>> {
        if !self.open_transcript().await? {
            return Ok(Vec::new());
        }
        self.handle_truncation().await?;

        let path = self.dirs.stream_jsonl();
        let session_id = self.session_id;
        let mut gained = Vec::new();
        {
            let Some(file) = self.file.as_mut() else {
                return Ok(Vec::new());
            };

            let mut chunk = vec![0u8; READ_CHUNK];
            loop {
                let read = file
                    .read(&mut chunk)
                    .await
                    .map_err(|err| read_failure(&path, session_id, err))?;
                if read == 0 {
                    break;
                }
                gained.extend_from_slice(&chunk[..read]);
            }
        }

        self.read_offset += gained.len() as u64;
        self.buffer.append(&mut gained);

        Ok(self.usable_lines())
    }

    /// The complete lines the buffer holds that may be committed.
    ///
    /// Everything ending at or below [`Self::committed_offset`] is dropped
    /// without reaching the translator: `_offset` never moves backwards, so a
    /// transcript that shrank below the committed offset records nothing until it
    /// grows past that offset again (`ARCHITECTURE.md`, "Durability and
    /// recovery"). In ordinary operation every line ends above it and this keeps
    /// them all.
    fn usable_lines(&mut self) -> Vec<(String, u64)> {
        let committed = self.committed_offset;
        let mut lines = self.take_complete_lines();
        if !self.below_committed {
            return lines;
        }

        let before = lines.len();
        lines.retain(|(_, end_offset)| *end_offset > committed);
        let dropped = before - lines.len();
        if dropped > 0 {
            warn!(
                session_id = %self.session_id,
                dropped,
                committed_offset = committed,
                "transcript lines below the committed offset were not recorded",
            );
        }
        if !lines.is_empty() {
            // The file grew past the committed offset: the first line above it
            // is ordinary output again, and a later shrink announces itself
            // afresh.
            self.below_committed = false;
        }

        lines
    }

    /// Open the transcript at the read offset, answering whether it is open.
    ///
    /// A missing file is not an error: the owner may start before the
    /// entrypoint's redirection has created it, and the next tick tries again.
    async fn open_transcript(&mut self) -> Result<bool> {
        if self.file.is_some() {
            return Ok(true);
        }

        let path = self.dirs.stream_jsonl();
        match File::open(&path).await {
            Ok(mut file) => {
                file.seek(SeekFrom::Start(self.read_offset))
                    .await
                    .map_err(|err| read_failure(&path, self.session_id, err))?;
                self.file = Some(file);
                Ok(true)
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                debug!(session_id = %self.session_id, "the transcript does not exist yet");
                Ok(false)
            }
            Err(err) => Err(read_failure(&path, self.session_id, err)),
        }
    }

    /// Skip to the end of a transcript that shrank.
    ///
    /// Truncated or replaced under a running owner, the committed offsets no
    /// longer index this content. Rewinding to 0 would re-translate history the
    /// database already holds, so the owner gives up on the bytes it missed and
    /// continues from the new end.
    ///
    /// A shrink *below* the committed offset is more than missed bytes: the
    /// stored `MAX(_offset)` is then ahead of the whole file, so the lines it
    /// gains would carry offsets that go backwards. The session stays usable —
    /// the container runs, stop and exit handling are untouched — but one
    /// non-fatal `error` event says that output is not recorded until the file
    /// passes the committed offset again, and [`Self::usable_lines`] drops
    /// everything below it (`ARCHITECTURE.md`, "Durability and recovery").
    async fn handle_truncation(&mut self) -> Result<()> {
        let path = self.dirs.stream_jsonl();
        let session_id = self.session_id;
        let read_offset = self.read_offset;

        let length = {
            let Some(file) = self.file.as_mut() else {
                return Ok(());
            };
            file.metadata()
                .await
                .map_err(|err| read_failure(&path, session_id, err))?
                .len()
        };
        if length >= read_offset {
            return Ok(());
        }

        warn!(
            session_id = %session_id,
            length,
            read_offset,
            "the transcript shrank; continuing from its new end",
        );
        self.buffer.clear();
        self.read_offset = length;
        if let Some(file) = self.file.as_mut() {
            file.seek(SeekFrom::Start(length))
                .await
                .map_err(|err| read_failure(&path, session_id, err))?;
        }

        if length < self.committed_offset && !self.below_committed {
            let committed = self.committed_offset;
            self.below_committed = true;
            warn!(
                session_id = %session_id,
                length,
                committed_offset = committed,
                "the transcript shrank below the committed offset; recording nothing until it passes it again",
            );
            self.append_own_event(AgentEvent::new(AgentEventBody::Error {
                message: format!(
                    "the session transcript was truncated to {length} bytes, below the {committed} bytes already recorded; \
                     agent output is not recorded until the transcript grows past {committed} bytes again",
                ),
                fatal: false,
            }))
            .await;
        }

        Ok(())
    }

    /// Split the buffer into complete lines, leaving any partial tail in place.
    fn take_complete_lines(&mut self) -> Vec<(String, u64)> {
        let buffer_start = self.read_offset - self.buffer.len() as u64;
        let mut lines = Vec::new();
        let mut consumed = 0usize;

        for (index, byte) in self.buffer.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }

            let end_offset = buffer_start + index as u64 + 1;
            // Lossy: a transcript byte sequence that is not UTF-8 is the CLI's
            // problem, and a replacement character in a `raw` event is better
            // than a stalled tail.
            let line = String::from_utf8_lossy(&self.buffer[consumed..index])
                .trim_end_matches('\r')
                .to_string();
            consumed = index + 1;

            if line.trim().is_empty() {
                continue;
            }
            lines.push((line, end_offset));
        }

        self.buffer.drain(..consumed);
        lines
    }

    /// Translate one line and work out everything its commit has to carry.
    ///
    /// `None` when the line produced no events at all. The `init` rules are
    /// applied here rather than after the commit: `cli_session_id` and the MCP
    /// `launch_warning` belong in the same transaction as the `init` event
    /// itself.
    fn translate_line(&mut self, line: &str, end_offset: u64) -> Option<TranslatedLine> {
        let mut events = self.backend.translate(line, &mut self.translate);
        if events.is_empty() {
            return None;
        }

        let mut cli_session_id = None;
        let mut warnings = Vec::new();
        let mut cost = None;
        let mut result = None;

        for event in &events {
            match &event.body {
                AgentEventBody::Init {
                    cli_session_id: id,
                    mcp_servers,
                    ..
                } => {
                    cli_session_id = Some(id.clone());
                    if let Some(message) = mcp_warning(mcp_servers) {
                        warn!(
                            session_id = %self.session_id,
                            server = MARS_MCP_SERVER,
                            "the session's MCP server is not connected",
                        );
                        warnings.push(AgentEvent::new(AgentEventBody::LaunchWarning { message }));
                    }
                }
                AgentEventBody::Result {
                    subtype,
                    terminal_reason,
                    is_error,
                    cost_usd,
                    usage,
                    ..
                } => {
                    cost = Some(self.cost_delta(*cost_usd, usage.as_ref()));
                    result = Some(ResultSummary {
                        subtype: subtype.clone(),
                        terminal_reason: terminal_reason.clone(),
                        is_error: *is_error,
                    });
                }
                _ => {}
            }
        }

        events.append(&mut warnings);

        // Kinds only: a payload carries agent output, which is never logged
        // above `debug` (`CLAUDE.md`, rule 3).
        debug!(
            session_id = %self.session_id,
            kinds = ?events.iter().map(AgentEvent::kind).collect::<Vec<_>>(),
            end_offset,
            "a transcript line translated",
        );

        Some(TranslatedLine {
            events,
            end_offset,
            cost,
            cli_session_id,
            result,
        })
    }

    /// What one `result` adds to the session's counters.
    ///
    /// The cost follows [`COST_ACCOUNTING`]; the tokens are the turn's own under
    /// either rule and are taken as they arrive, defaulting to zero when the
    /// backend omits them. A cost lower than the baseline contributes nothing
    /// rather than a negative amount, and the baseline keeps the higher value so
    /// a later rise is not counted twice (`ARCHITECTURE.md`, "Cost
    /// accounting").
    fn cost_delta(&mut self, cost_usd: Option<f64>, usage: Option<&Value>) -> CostDelta {
        let total = match cost_usd {
            Some(total) if total.is_finite() && total >= 0.0 => total,
            Some(_) => {
                warn!(session_id = %self.session_id, "a result reported an unusable cost");
                0.0
            }
            None => 0.0,
        };

        let (cost_usd, baseline) = charge_cost(COST_ACCOUNTING, total, self.last_result_cost);
        self.last_result_cost = baseline;

        CostDelta {
            cost_usd,
            input_tokens: token_count(usage, "input_tokens"),
            output_tokens: token_count(usage, "output_tokens"),
        }
    }

    /// Commit one translated line: its events, its offset, its counters and any
    /// `cli_session_id` it announced, in one transaction.
    ///
    /// The offset in memory moves only after the commit returns, so an
    /// interrupted write leaves the owner believing exactly what the database
    /// believes.
    async fn commit_line(&mut self, batch: &TranslatedLine) -> Result<()> {
        let repository = SessionRepository::new(&self.state.pool);
        let mut tx = self.state.pool.begin().await?;

        if let Some(cli_session_id) = &batch.cli_session_id {
            // The same row lock `append_native_line` takes, taken once for the
            // whole transaction (ADR 0021).
            let session = repository.lock_session(&mut tx, self.session_id).await?;
            if session.state != SessionState::Running {
                // Non-fatal: the CLI restarted inside the container, or the
                // launcher has not attached stdin yet. The id is still the id
                // of the conversation to resume.
                warn!(
                    session_id = %self.session_id,
                    state = session.state.as_str(),
                    "an init arrived for a session that is not running",
                );
            }
            repository
                .set_cli_session_id(&mut tx, self.session_id, cli_session_id)
                .await?;
        }

        repository
            .append_native_line(
                &mut tx,
                self.session_id,
                &batch.events,
                batch.end_offset,
                self.committed_offset,
                batch.cost,
            )
            .await?;

        tx.commit().await?;
        self.committed_offset = batch.end_offset;

        Ok(())
    }

    /// Record an accepted input and write it to the CLI.
    ///
    /// The `user_message` event is written first, so the conversation shows what
    /// the user sent even if the write fails (`ARCHITECTURE.md`, "Session owner
    /// task", step 2), and the text is recorded in the translation state so the
    /// CLI's echo of it is suppressed rather than stored twice.
    ///
    /// A failed write is an `error` event with `fatal: false`, a `warn!` and
    /// nothing else: v1 has no durable input queue and no delivery status, so a
    /// retry here would risk writing the same turn twice rather than fixing
    /// anything (ADR 0020; `ARCHITECTURE.md`, "Input delivery across restarts").
    async fn deliver_input(&mut self, queued: QueuedInput) {
        if self.kind == SessionKind::Ephemeral {
            // The service refuses input to an ephemeral session, whose prompt is
            // in argv and whose stdin is never written to
            // (`ARCHITECTURE.md`, "Claude Code invocation").
            warn!(
                session_id = %self.session_id,
                "an input reached an ephemeral session's owner and was dropped",
            );
            return;
        }

        let event = AgentEvent::new(AgentEventBody::UserMessage {
            text: queued.input.text().to_string(),
            user_id: queued.user_id,
            client_id: queued.client_id.clone(),
        });
        let (kind, payload) = event.into_row_parts(None);
        let row = NewEvent {
            ts: queued.accepted_at,
            kind,
            payload,
        };

        let repository = SessionRepository::new(&self.state.pool);
        match self.state.pool.begin().await {
            Ok(mut tx) => {
                let appended = repository
                    .append_events(&mut tx, self.session_id, std::slice::from_ref(&row))
                    .await;
                match appended {
                    Ok(_) => {
                        if let Err(err) = tx.commit().await {
                            error!(
                                session_id = %self.session_id,
                                error = %err,
                                "could not record a user message",
                            );
                        }
                    }
                    Err(err) => error!(
                        session_id = %self.session_id,
                        error = %err,
                        "could not record a user message",
                    ),
                }
            }
            Err(err) => error!(
                session_id = %self.session_id,
                error = %err,
                "could not record a user message",
            ),
        }

        self.translate.record_sent_input(queued.input.text());

        let Some(stdin) = self.stdin.as_mut() else {
            debug!(session_id = %self.session_id, "no stdin is attached; the input was only recorded");
            return;
        };

        let failure = match self.backend.encode_input(&queued.input) {
            Ok(line) => match stdin.write_all(line.as_bytes()).await {
                Ok(()) => stdin.flush().await.err(),
                Err(err) => Some(err),
            },
            Err(err) => {
                // Not an `io::Error`, and not an event either: an input the
                // encoder refuses never reached the CLI and never will, and the
                // `user_message` above already says what the user sent.
                warn!(
                    session_id = %self.session_id,
                    error = %err,
                    "an accepted input could not be encoded",
                );
                return;
            }
        };

        if let Some(err) = failure {
            warn!(
                session_id = %self.session_id,
                error = %err,
                "could not write an input to the CLI's stdin",
            );
            self.append_own_event(AgentEvent::new(AgentEventBody::Error {
                message: format!("failed to write input to CLI: {err}"),
                fatal: false,
            }))
            .await;
        }
    }

    /// The end-of-run hook, called after a line carrying a `result` committed.
    ///
    /// A conversational session goes on: a `result` is the end of a turn, and
    /// the next message starts another on the same process. An ephemeral session
    /// is finished — fetch-back, stop, `done` — and never parked, resumed or
    /// retried (`ARCHITECTURE.md`, "Claude Code invocation").
    ///
    /// `terminal_reason` is deliberately not branched on here. A `result` with
    /// `is_error: true` and `terminal_reason: "aborted_streaming"` is the CLI
    /// answering the `SIGINT` Mars sent, and what it leads to is decided by the
    /// exit rule, which already knows a stop was requested (`ARCHITECTURE.md`,
    /// "Stop semantics").
    async fn on_result(&mut self, result: &ResultSummary) -> ControlFlow<OwnerExit> {
        debug!(
            session_id = %self.session_id,
            subtype = %result.subtype,
            terminal_reason = ?result.terminal_reason,
            is_error = result.is_error,
            "a turn ended",
        );
        self.result_seen = true;

        if self.kind != SessionKind::Ephemeral {
            return ControlFlow::Continue(());
        }

        ControlFlow::Break(self.end_ephemeral_run().await)
    }

    /// The ephemeral end-of-run: publish the branch, stop the container, `done`.
    ///
    /// In that order, and the order is the point: the fetch-back is what makes
    /// the session's work reachable, so it happens while the container is still
    /// there, before anything can fail on the way to `done`
    /// (`ARCHITECTURE.md`, "Claude Code invocation").
    async fn end_ephemeral_run(&mut self) -> OwnerExit {
        self.fetch_back().await;
        self.stop_after_result().await;

        let plan = ExitPlan {
            to: SessionState::Done,
            reason: "result received".to_string(),
            signal: None,
            error: None,
        };
        if !self.apply(SessionState::Running, &plan).await {
            return OwnerExit::StoodDown;
        }

        self.finish().await;
        self.state.session_ended(self.session_id).await;

        OwnerExit::Ended(SessionState::Done)
    }

    /// Fetch `session/<sid>` into `refs/sessions/<sid>` and record the outcome.
    ///
    /// The project git lock is taken first and released before the event's
    /// transaction opens: any git lock comes before any database lock, never the
    /// reverse (`ARCHITECTURE.md`, "Git model", Serialization; ADR 0021).
    ///
    /// A failure is an event, not an abort. The session ran and its `result` is
    /// committed; what is left to say is that its branch did not reach the
    /// project repository, which is what `git { ok: false }` says.
    async fn fetch_back(&mut self) {
        let repository = SessionRepository::new(&self.state.pool);
        let project_id = match repository.find(self.session_id).await {
            Ok(Some(session)) => session.project_id,
            Ok(None) => {
                warn!(session_id = %self.session_id, "the session is gone; nothing to fetch back");
                return;
            }
            Err(err) => {
                error!(
                    session_id = %self.session_id,
                    error = %err,
                    "could not read the session before its fetch-back",
                );
                return;
            }
        };

        let service = GitService::from_state(&self.state);
        let outcome = {
            let guard = self.state.git_locks.lock(project_id).await;
            service.sync_session_silent(&guard, self.session_id).await
        };

        let git_ref = refs::session_ref(self.session_id);
        let (ok, detail) = match outcome {
            Ok(commit) => (
                true,
                GitSyncDetail {
                    git_ref,
                    commit: Some(commit),
                    error: None,
                },
            ),
            Err(err) => {
                warn!(
                    session_id = %self.session_id,
                    error = %err,
                    "the end-of-run fetch-back failed",
                );
                (
                    false,
                    GitSyncDetail {
                        git_ref,
                        commit: None,
                        error: Some(err.user_message()),
                    },
                )
            }
        };

        let detail = match serde_json::to_value(&detail) {
            Ok(detail) => detail,
            Err(err) => {
                error!(
                    session_id = %self.session_id,
                    error = %err,
                    "a git sync detail could not be serialised",
                );
                return;
            }
        };

        self.append_own_event(AgentEvent::new(AgentEventBody::Git {
            op: GitOp::Sync,
            ok,
            detail,
        }))
        .await;
    }

    /// Give the CLI the grace period to exit on its own after its `result`, then
    /// `SIGTERM` it.
    ///
    /// `claude -p` exits by itself once it has written its `result`, so the
    /// ordinary path here is a wait of milliseconds; the signal is for the run
    /// that does not (`ARCHITECTURE.md`, "Stop semantics": ending a session is
    /// the stop followed by the fetch-back and the container's removal).
    async fn stop_after_result(&mut self) {
        let Some(container_id) = self.container_id.clone() else {
            return;
        };
        let grace = Duration::from_secs(self.state.config.stop_grace_secs);

        match tokio::time::timeout(grace, self.state.engine.wait(&container_id)).await {
            Ok(Ok(status)) => debug!(
                session_id = %self.session_id,
                exit_code = status.code,
                "the ephemeral CLI exited on its own after its result",
            ),
            Ok(Err(EngineError::NotFound(_))) => debug!(
                session_id = %self.session_id,
                "the ephemeral container was already gone after its result",
            ),
            Ok(Err(err)) => warn!(
                session_id = %self.session_id,
                error = %err,
                "could not wait for the ephemeral container to exit",
            ),
            Err(_) => self.send_signal(&container_id, Signal::Sigterm).await,
        }
    }

    /// Take a stop request: `SIGINT` now, `SIGTERM` when the grace period runs
    /// out (`ARCHITECTURE.md`, "Stop semantics").
    ///
    /// A second request while one is pending is ignored whatever its reason:
    /// the grace period is already running, a second `SIGINT` would only
    /// restart it, and the first stop's reason is the one the exit records.
    async fn request_stop(&mut self, reason: StopReason) {
        if self.stop_reason.is_some() {
            debug!(session_id = %self.session_id, "a stop is already pending");
            return;
        }

        let reason = reason.for_kind(self.kind, self.session_id);

        let Some(container_id) = self.container_id.clone() else {
            // The route answers 409 for a session that is not running; an owner
            // with no container is one whose launch has not got there yet.
            warn!(
                session_id = %self.session_id,
                "a stop reached an owner with no container and was ignored",
            );
            return;
        };

        self.stop_reason = Some(reason);
        self.send_signal(&container_id, Signal::Sigint).await;
        // Armed whatever the signal did: a container that is already gone
        // resolves the exit watch long before this fires, and a transport
        // failure must not leave the session waiting forever.
        self.stop_deadline =
            Some(Instant::now() + Duration::from_secs(self.state.config.stop_grace_secs));
    }

    /// The grace period ran out with the CLI still running: `SIGTERM`.
    async fn escalate_stop(&mut self) {
        self.stop_deadline = None;

        let Some(container_id) = self.container_id.clone() else {
            return;
        };
        debug!(
            session_id = %self.session_id,
            "the stop grace period ran out; sending SIGTERM",
        );
        self.send_signal(&container_id, Signal::Sigterm).await;
    }

    /// Send one signal and remember it as the one that ended the run.
    ///
    /// A container that has already exited answers `Conflict` and a removed one
    /// `NotFound`; neither is a failure — the process is gone, which is what the
    /// signal was for — and both leave the signal recorded, because it is what
    /// Mars asked for and what the `state_change` should say
    /// (`ARCHITECTURE.md`, "Engine adapter").
    async fn send_signal(&mut self, container_id: &ContainerId, signal: Signal) {
        let recorded = match signal {
            Signal::Sigint => Some(StopSignal::Sigint),
            Signal::Sigterm => Some(StopSignal::Sigterm),
            // Not part of the stop sequence and not a value the event carries.
            Signal::Sigkill => None,
        };

        match self.state.engine.kill(container_id, signal).await {
            Ok(()) => {
                debug!(session_id = %self.session_id, %signal, "signalled the session container");
            }
            Err(EngineError::Conflict(_) | EngineError::NotFound(_)) => {
                debug!(
                    session_id = %self.session_id,
                    %signal,
                    "the session container had already ended when the signal was sent",
                );
            }
            Err(err) => {
                error!(
                    session_id = %self.session_id,
                    %signal,
                    error = %err,
                    "could not signal the session container",
                );
                return;
            }
        }

        if recorded.is_some() {
            self.stop_signal = recorded;
        }
    }

    /// The container exited: commit the rest of the transcript, then apply the
    /// lifecycle rule to what the exit was.
    ///
    /// Draining first is what keeps the final `result` in front of the
    /// `state_change` that reports the exit: the CLI writes it and exits in the
    /// same breath (`ARCHITECTURE.md`, "Session owner task", step 3).
    async fn on_container_exit(&mut self, observed: ContainerExit) -> OwnerExit {
        debug!(session_id = %self.session_id, ?observed, "the session container ended");

        // An ephemeral session whose `result` is in the tail ends through
        // `on_result`, which is the run it had rather than an exit rule.
        if let ControlFlow::Break(exit) = self.drain_to_eof().await {
            return exit;
        }

        let repository = SessionRepository::new(&self.state.pool);
        let from = match repository.find(self.session_id).await {
            Ok(Some(session)) => session.state,
            Ok(None) => {
                warn!(session_id = %self.session_id, "the session is gone; nothing to transition");
                return OwnerExit::StoodDown;
            }
            Err(err) => {
                error!(
                    session_id = %self.session_id,
                    error = %err,
                    "could not read the session after its container exited",
                );
                return OwnerExit::StoodDown;
            }
        };

        let Some(plan) = self.exit_plan(from, observed) else {
            debug!(
                session_id = %self.session_id,
                state = from.as_str(),
                "the session had already left running when its container exited",
            );
            return OwnerExit::StoodDown;
        };

        if !self.apply(from, &plan).await {
            return OwnerExit::StoodDown;
        }

        self.finish().await;
        if plan.to != SessionState::Parked {
            self.state.session_ended(self.session_id).await;
        }

        OwnerExit::Ended(plan.to)
    }

    /// Which transition one exit calls for, or `None` when the session is not
    /// this owner's to transition any more.
    ///
    /// The rules are `ARCHITECTURE.md`, "Session lifecycle" and "Stop
    /// semantics", in this order:
    ///
    /// - a session still `creating` never saw its CLI start, so whatever the
    ///   exit code says, the launch failed;
    /// - a stop was requested, so the exit is the stop completing whatever the
    ///   code says, `parked` with the stop's own reason — `stopped by user`,
    ///   `idle timeout` or `stalled` ([`StopReason`]) — and the last signal
    ///   sent recorded;
    /// - the container is not there at all, so there is nothing to fail on:
    ///   `parked`, which is the state a session with no container is in;
    /// - an ephemeral session that exited before its `result` produced nothing
    ///   and is `failed`, because it is never parked or retried;
    /// - exit 0 is the CLI finishing, which for a conversational session is
    ///   `parked`;
    /// - anything else failed, and `sessions.error` says what the code was.
    ///
    /// An ephemeral session is never parked (ADR 0003), so wherever these rules
    /// say `parked` for one — a stop, which the idle reaper sends a stalled run,
    /// or a container that disappeared — it is `failed` with the same reason,
    /// the stop's signal still recorded.
    fn exit_plan(&self, from: SessionState, observed: ContainerExit) -> Option<ExitPlan> {
        let plan = self.kind_agnostic_exit_plan(from, observed)?;

        if self.kind == SessionKind::Ephemeral && plan.to == SessionState::Parked {
            return Some(ExitPlan {
                to: SessionState::Failed,
                error: Some(plan.reason.clone()),
                ..plan
            });
        }

        Some(plan)
    }

    /// [`SessionOwner::exit_plan`] before the ephemeral never-parked rule.
    fn kind_agnostic_exit_plan(
        &self,
        from: SessionState,
        observed: ContainerExit,
    ) -> Option<ExitPlan> {
        let parked = |reason: &str, signal: Option<StopSignal>| ExitPlan {
            to: SessionState::Parked,
            reason: reason.to_string(),
            signal,
            error: None,
        };
        let failed = |reason: String| ExitPlan {
            to: SessionState::Failed,
            reason: reason.clone(),
            signal: None,
            error: Some(reason),
        };

        match from {
            SessionState::Creating => Some(match observed {
                ContainerExit::Exited(status) => failed(format!(
                    "CLI exited with status {} before init",
                    status.code,
                )),
                ContainerExit::Gone => failed("container disappeared before init".to_string()),
            }),
            SessionState::Running => {
                if let Some(reason) = self.stop_reason {
                    return Some(parked(reason.recorded(), self.stop_signal));
                }
                let status = match observed {
                    ContainerExit::Exited(status) => status,
                    ContainerExit::Gone => return Some(parked("container disappeared", None)),
                };
                if self.kind == SessionKind::Ephemeral && !self.result_seen {
                    return Some(failed(format!(
                        "CLI exited with status {} before result",
                        status.code,
                    )));
                }
                if status.oom_killed {
                    return Some(failed(format!(
                        "the CLI was killed by the out-of-memory killer (exit status {})",
                        status.code,
                    )));
                }
                if status.code == 0 {
                    return Some(parked("CLI exited", None));
                }
                Some(failed(format!("CLI exited with status {}", status.code)))
            }
            // Parked, done or failed already: the reaper, the session service or
            // a newer owner got there first.
            _ => None,
        }
    }

    /// Write one planned transition and say whether it went through.
    ///
    /// `false` is never a reason to carry on with the clean-up: a lost race left
    /// the row saying what the winner decided, and removing the container under
    /// it would be this owner acting on a state it does not own.
    async fn apply(&mut self, from: SessionState, plan: &ExitPlan) -> bool {
        let mut change = Transition::new(from, plan.to, &plan.reason);
        if let Some(signal) = plan.signal {
            change = change.with_signal(signal);
        }
        if let Some(error) = &plan.error {
            change = change.with_error(error);
        }

        let repository = SessionRepository::new(&self.state.pool);
        let written = async {
            let mut tx = self.state.pool.begin().await?;
            repository
                .transition(&mut tx, self.session_id, &change)
                .await?;
            tx.commit().await?;
            Ok::<(), Error>(())
        }
        .await;

        match written {
            Ok(()) => true,
            Err(err) => {
                error!(
                    session_id = %self.session_id,
                    from = from.as_str(),
                    to = plan.to.as_str(),
                    error = %err,
                    "could not transition the session out of running",
                );
                false
            }
        }
    }

    /// Everything a session that has left `running` needs: no container, no
    /// container id and no stdin (`docs/data-model.md`, `sessions`).
    ///
    /// The registry is [`SessionOwner::spawn`]'s, so that the one place that
    /// knows the loop returned is the one place that touches it.
    async fn finish(&mut self) {
        if let Some(container_id) = self.container_id.take() {
            match self.state.engine.remove(&container_id, true).await {
                // A container that is not there is already removed.
                Ok(()) | Err(EngineError::NotFound(_)) => {}
                Err(err) => warn!(
                    session_id = %self.session_id,
                    error = %err,
                    "could not remove the session container",
                ),
            }

            let cleared = async {
                let repository = SessionRepository::new(&self.state.pool);
                let mut tx = self.state.pool.begin().await?;
                repository
                    .set_container_id(&mut tx, self.session_id, None)
                    .await?;
                tx.commit().await?;
                Ok::<(), Error>(())
            }
            .await;
            if let Err(err) = cleared {
                error!(
                    session_id = %self.session_id,
                    error = %err,
                    "could not clear the session's container id",
                );
            }
        }

        if let Some(mut stdin) = self.stdin.take() {
            // Best effort: the process it belonged to has exited, so a failed
            // shutdown of the attach stream says nothing worth an event.
            if let Err(err) = stdin.shutdown().await {
                debug!(
                    session_id = %self.session_id,
                    error = %err,
                    "closing the CLI's stdin failed",
                );
            }
        }
    }

    /// Read and commit until the transcript stops growing.
    ///
    /// Two consecutive empty reads rather than one: the exit the engine reported
    /// and the last bytes reaching the file are not ordered, so one empty read
    /// can happen between them.
    async fn drain_to_eof(&mut self) -> ControlFlow<OwnerExit> {
        let deadline = Instant::now() + DRAIN_LIMIT;
        let mut empty = 0usize;

        while empty < DRAIN_EMPTY_READS {
            match self.read_available_lines().await {
                Ok(lines) if lines.is_empty() => empty += 1,
                Ok(lines) => {
                    empty = 0;
                    self.queue.extend(lines);
                }
                // Logged at the source. A transcript that cannot be read is not
                // going to produce anything more either.
                Err(_) => empty += 1,
            }

            self.commit_queued().await?;

            if Instant::now() >= deadline {
                warn!(
                    session_id = %self.session_id,
                    "the transcript was still growing when the container exited",
                );
                break;
            }
            if empty < DRAIN_EMPTY_READS {
                tokio::time::sleep(DRAIN_SETTLE).await;
            }
        }

        ControlFlow::Continue(())
    }

    /// Append one event of the owner's own — not a translated line — in its own
    /// transaction.
    ///
    /// No offset moves and no counter changes, so there is nothing to commit
    /// together with it; a failure is logged and the loop carries on, because an
    /// event that could not be written is not a reason to abandon the session.
    async fn append_own_event(&mut self, event: AgentEvent) {
        let written = async {
            let repository = SessionRepository::new(&self.state.pool);
            let mut tx = self.state.pool.begin().await?;
            repository
                .append_event(&mut tx, self.session_id, &event)
                .await?;
            tx.commit().await?;
            Ok::<(), Error>(())
        }
        .await;

        if let Err(err) = written {
            error!(
                session_id = %self.session_id,
                kind = event.kind(),
                error = %err,
                "could not append a session event",
            );
        }
    }

    /// Rebuild the adopted process's translation memory from retained history.
    ///
    /// Two sources, in this order: the `user_message` events this process was
    /// sent, so an echo that has not arrived yet is still suppressed; then the
    /// committed part of *this process's* transcript region, replayed through
    /// the adapter so open subagent calls, denial bookkeeping and the announced
    /// `init` id come back. Recording the inputs first is what makes the
    /// difference between a delayed echo and one already seen: an echo inside
    /// the replayed region consumes its hash again, exactly as it did the first
    /// time (`ARCHITECTURE.md`, "Durability and recovery").
    ///
    /// Nothing is published: the replayed events are dropped, no counter moves
    /// and no input is resent. The only number kept is the cumulative cost
    /// baseline, which the rule in [`COST_ACCOUNTING`] needs to charge the next
    /// `result` correctly.
    async fn restore_translation_state(&mut self) -> Result<()> {
        let repository = SessionRepository::new(&self.state.pool);
        let start = repository.process_start(self.session_id).await?;

        let mut inputs = 0usize;
        for event in repository
            .events_after(self.session_id, start.launch_seq)
            .await?
        {
            if let AgentEventBody::UserMessage { text, .. } = &event.event.body {
                self.translate.record_sent_input(text);
                inputs += 1;
            }
        }

        let lines = self
            .replay(start.start_offset, self.committed_offset)
            .await?;

        debug!(
            session_id = %self.session_id,
            launch_seq = start.launch_seq,
            start_offset = start.start_offset,
            committed_offset = self.committed_offset,
            inputs,
            lines,
            "the adopted process's translation state was reconstructed",
        );

        Ok(())
    }

    /// Feed the transcript region `[from, to)` through the adapter, discarding
    /// its events, and answer how many lines it held.
    async fn replay(&mut self, from: u64, to: u64) -> Result<usize> {
        if to <= from {
            return Ok(0);
        }

        let path = self.dirs.stream_jsonl();
        let mut file = match File::open(&path).await {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                warn!(
                    session_id = %self.session_id,
                    "the transcript is gone; nothing to reconstruct from",
                );
                return Ok(0);
            }
            Err(err) => return Err(read_failure(&path, self.session_id, err)),
        };
        file.seek(SeekFrom::Start(from))
            .await
            .map_err(|err| read_failure(&path, self.session_id, err))?;

        let mut region = vec![0u8; (to - from) as usize];
        file.read_exact(&mut region)
            .await
            .map_err(|err| read_failure(&path, self.session_id, err))?;

        let mut lines = 0usize;
        let text = String::from_utf8_lossy(&region);
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            lines += 1;

            // The events are dropped on purpose: they are already in `events`.
            // Only the state the call mutates — and the cumulative baseline —
            // is what this replay is for.
            let events = self.backend.translate(line, &mut self.translate);
            for event in &events {
                if let AgentEventBody::Result { cost_usd, .. } = &event.body {
                    self.cost_delta(*cost_usd, None);
                    // An ephemeral session whose `result` is already committed
                    // did produce one, whatever this owner goes on to see.
                    self.result_seen = true;
                }
            }
        }

        Ok(lines)
    }
}

/// Watch one container for its exit on a task of its own.
///
/// A transport failure is not an exit: an unreachable engine says nothing about
/// the process, so the wait is retried with a doubling backoff and the session
/// keeps being tailed rather than parked (`ARCHITECTURE.md`, "Durability and
/// recovery"). Only an answer — an exit status, or a container the engine no
/// longer has — reaches the owner.
fn watch_container(
    engine: Arc<dyn ContainerEngine>,
    container_id: ContainerId,
    session_id: Uuid,
) -> oneshot::Receiver<ContainerExit> {
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let mut backoff = WAIT_RETRY_START;
        loop {
            match engine.wait(&container_id).await {
                Ok(status) => {
                    let _ = tx.send(ContainerExit::Exited(status));
                    return;
                }
                Err(EngineError::NotFound(_)) => {
                    let _ = tx.send(ContainerExit::Gone);
                    return;
                }
                Err(err) => {
                    error!(
                        session_id = %session_id,
                        error = %err,
                        backoff_secs = backoff.as_secs(),
                        "could not watch the session container; retrying",
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(WAIT_RETRY_MAX);
                }
            }

            if tx.is_closed() {
                // The owner is gone; nobody is left to tell.
                return;
            }
        }
    });

    rx
}

/// The container watch's answer, or never when there is no container to watch.
///
/// Clears the watch as it resolves, so the `select!` branch it feeds cannot be
/// taken twice with the same answer.
async fn next_exit(watch: &mut Option<oneshot::Receiver<ContainerExit>>) -> ContainerExit {
    let Some(receiver) = watch.as_mut() else {
        return std::future::pending().await;
    };

    match receiver.await {
        Ok(observed) => {
            *watch = None;
            observed
        }
        // The watcher task went away without an answer, which leaves nothing to
        // observe: an orchestrator shutdown or a panic, both of which the
        // session's next start reconciles.
        Err(_) => {
            *watch = None;
            std::future::pending().await
        }
    }
}

/// Resolve at `deadline`, or never when there is none.
///
/// The deadline is absolute, so rebuilding this future on every pass of the loop
/// resolves at the same moment rather than restarting the countdown.
async fn elapsed(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// What a `result` reporting `total` charges under `rule`, and the baseline the
/// next one is measured against.
///
/// Under [`CostAccounting::PerTurn`] the value is the charge and there is no
/// baseline to keep. Under [`CostAccounting::Cumulative`] the charge is the
/// increase over `previous`, never negative, and the baseline keeps the higher
/// of the two so a dip followed by a rise is not charged twice
/// (`ARCHITECTURE.md`, "Cost accounting").
fn charge_cost(rule: CostAccounting, total: f64, previous: Option<f64>) -> (f64, Option<f64>) {
    match rule {
        CostAccounting::PerTurn => (total, None),
        CostAccounting::Cumulative => {
            let baseline = previous.unwrap_or(0.0);
            ((total - baseline).max(0.0), Some(total.max(baseline)))
        }
    }
}

/// The `launch_warning` message for an `init` that does not report Mars's MCP
/// server as connected, or `None` when it does.
///
/// The status is compared case-insensitively and named in the message, because
/// an unreachable server is reported as `failed` and the session runs on without
/// MCP tools: a degraded session, not a failed launch (`ARCHITECTURE.md`, "MCP
/// design").
fn mcp_warning(servers: &[McpServerStatus]) -> Option<String> {
    let entry = servers
        .iter()
        .find(|server| server.name.eq_ignore_ascii_case(MARS_MCP_SERVER));

    match entry {
        Some(server) if server.status.eq_ignore_ascii_case("connected") => None,
        Some(server) => Some(format!(
            "MCP server {MARS_MCP_SERVER} is not connected (status: {})",
            server.status,
        )),
        None => Some(format!(
            "MCP server {MARS_MCP_SERVER} is not connected (status: {MISSING_STATUS})",
        )),
    }
}

/// One `usage` counter, `0` when the backend omitted it or reported something
/// that is not a non-negative integer.
fn token_count(usage: Option<&Value>, field: &str) -> i64 {
    usage
        .and_then(|usage| usage.get(field))
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0)
}

/// A transcript read failure, with the path in the log and never in the message.
fn read_failure(path: &Path, session_id: Uuid, err: std::io::Error) -> Error {
    error!(
        session_id = %session_id,
        path = %path.display(),
        error = %err,
        "could not read the session transcript",
    );

    Error::Internal("could not read the session transcript".to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn servers(pairs: &[(&str, &str)]) -> Vec<McpServerStatus> {
        pairs
            .iter()
            .map(|(name, status)| McpServerStatus {
                name: (*name).to_string(),
                status: (*status).to_string(),
            })
            .collect()
    }

    #[test]
    fn a_connected_mars_server_warns_about_nothing() {
        assert_eq!(
            mcp_warning(&servers(&[("mars-orchestrator", "connected")])),
            None,
        );
        // The CLI's own spelling of the status is not something Mars branches
        // on, so the comparison is case-insensitive.
        assert_eq!(
            mcp_warning(&servers(&[("MARS-ORCHESTRATOR", "Connected")])),
            None,
        );
    }

    #[test]
    fn a_failed_mars_server_warns_with_its_status() {
        assert_eq!(
            mcp_warning(&servers(&[
                ("repo-local", "connected"),
                ("mars-orchestrator", "failed"),
            ])),
            Some("MCP server mars-orchestrator is not connected (status: failed)".to_string()),
        );
    }

    #[test]
    fn an_absent_mars_server_warns_as_missing() {
        assert_eq!(
            mcp_warning(&servers(&[("repo-local", "connected")])),
            Some("MCP server mars-orchestrator is not connected (status: missing)".to_string()),
        );
        assert_eq!(
            mcp_warning(&[]),
            Some("MCP server mars-orchestrator is not connected (status: missing)".to_string()),
        );
    }

    #[test]
    fn token_counts_default_to_zero() {
        let usage = json!({ "input_tokens": 7, "output_tokens": -3, "other": "x" });

        assert_eq!(token_count(Some(&usage), "input_tokens"), 7);
        // Negative counters would walk a session's totals backwards, which the
        // repository refuses; they are read as nothing instead.
        assert_eq!(token_count(Some(&usage), "output_tokens"), 0);
        assert_eq!(token_count(Some(&usage), "missing"), 0);
        assert_eq!(token_count(None, "input_tokens"), 0);
    }

    #[test]
    fn the_accumulation_rule_is_the_increase_over_the_previous_result() {
        assert_eq!(COST_ACCOUNTING, CostAccounting::Cumulative);
    }

    /// The probe's own numbers: two turns reporting 0.0412 and 0.0471 cost the
    /// session 0.0412 and 0.0059, not 0.0883
    /// (`tests/fixtures/claude/2.1.274/NOTES.md`, `multi_turn`).
    #[test]
    fn a_cumulative_result_charges_only_its_increase() {
        let (first, baseline) = charge_cost(CostAccounting::Cumulative, 0.0412002, None);
        assert!((first - 0.0412002).abs() < f64::EPSILON);
        assert_eq!(baseline, Some(0.0412002));

        let (second, baseline) = charge_cost(CostAccounting::Cumulative, 0.0470666, baseline);
        assert!((second - 0.0058664).abs() < 1e-12);
        assert_eq!(baseline, Some(0.0470666));
    }

    #[test]
    fn a_cumulative_result_that_went_down_charges_nothing_and_keeps_the_baseline() {
        let (charged, baseline) = charge_cost(CostAccounting::Cumulative, 0.5, Some(2.0));

        assert_eq!(charged, 0.0);
        assert_eq!(baseline, Some(2.0));
    }

    #[test]
    fn a_per_turn_result_charges_its_own_value_and_keeps_no_baseline() {
        let (charged, baseline) = charge_cost(CostAccounting::PerTurn, 0.25, Some(2.0));

        assert_eq!(charged, 0.25);
        assert_eq!(baseline, None);
    }
}
