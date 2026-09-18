//! The `SessionOwner` task: the one reader of a session's transcript and the
//! one writer of its CLI's stdin (`ARCHITECTURE.md`, "Session owner task").
//!
//! This module is the read side of that loop. It tails `log/stream.jsonl` from
//! the offset the database says was last committed, hands every *complete*
//! native line to the backend adapter, and commits that line's translated
//! events, its end offset and the counters it moved in one transaction — so
//! either the whole line is visible with its offset advanced or none of it is
//! (`ARCHITECTURE.md`, "Durability and recovery"; ADR 0010, 0021, 0028).
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
//! `ARCHITECTURE.md`, "Launch sequence"). Stdin writing beyond the input hook,
//! container-exit handling and stop semantics belong to the launcher task that
//! follows this one; the hooks they plug into are [`SessionOwner::deliver_input`]
//! and [`SessionOwner::on_result`].

use std::collections::VecDeque;
use std::io::{ErrorKind, SeekFrom};
use std::ops::ControlFlow;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

use crate::agent::{AgentBackend, TranslateConfig, TranslateState};
use crate::events::{AgentEvent, AgentEventBody, McpServerStatus};
use crate::models::{NewEvent, SessionKind, SessionState};
use crate::prelude::*;
use crate::repositories::{CostDelta, SessionRepository};
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
    /// Conversational or ephemeral, which decides what an end-of-run means to
    /// the launcher's hook.
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
    pub stdin: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    /// The engine's id for the container, for the exit watch the next task adds.
    pub container_id: Option<String>,
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

/// The owner of one session's transcript and stdin.
pub struct SessionOwner {
    session_id: Uuid,
    #[allow(dead_code, reason = "the exit hook the next task adds reads it")]
    kind: SessionKind,
    dirs: SessionDirs,
    backend: Arc<dyn AgentBackend>,
    state: AppState,
    stdin: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    #[allow(dead_code, reason = "the container watch the next task adds reads it")]
    container_id: Option<String>,
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
    /// A translated line whose commit failed, retried before any newer line.
    pending: Option<TranslatedLine>,
    /// The `cost_usd` of the previous `result` of this process, the baseline the
    /// cumulative rule subtracts.
    last_result_cost: Option<f64>,
}

impl SessionOwner {
    /// Run a session's loop on its own task, removing the session from the
    /// registry when the loop returns.
    ///
    /// The registry entry outliving the task would leave inputs being forwarded
    /// into a channel nobody reads, which is why the removal is here and not in
    /// any one exit path.
    pub fn spawn(ctx: OwnerContext) -> JoinHandle<()> {
        let session_id = ctx.session_id;
        let registry = ctx.state.session_registry.clone();

        tokio::spawn(async move {
            SessionOwner::new(ctx).run().await;
            registry.remove(session_id);
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
            adopted: ctx.adopted,
            translate: TranslateState::new(ctx.translate),
            file: None,
            read_offset: ctx.start_offset,
            buffer: Vec::new(),
            queue: VecDeque::new(),
            committed_offset: ctx.start_offset,
            pending: None,
            last_result_cost: None,
        }
    }

    /// Tail, take commands, and leave on `Shutdown` or a lost transcript.
    async fn run(mut self) {
        // Out of the struct, so the `recv()` future in the loop's `select!`
        // borrows the channel and the tail step borrows everything else.
        let Some(mut commands) = self.commands.take() else {
            error!(session_id = %self.session_id, "a session owner was run twice");
            return;
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

        loop {
            tokio::select! {
                _ = tail.tick() => {
                    if self.tail_step().await.is_break() {
                        break;
                    }
                }
                command = commands.recv() => {
                    match command {
                        Some(OwnerCommand::Input(input)) => self.deliver_input(input).await,
                        Some(OwnerCommand::Stop) => {
                            // Stop semantics — SIGINT, the grace period, then
                            // SIGTERM — are the next task's
                            // (`ARCHITECTURE.md`, "Stop semantics").
                            debug!(session_id = %self.session_id, "stop requested");
                        }
                        Some(OwnerCommand::Shutdown) => {
                            debug!(session_id = %self.session_id, "session owner asked to stand down");
                            break;
                        }
                        // Every sender is gone, which only happens once the
                        // registry entry was replaced or removed.
                        None => break,
                    }
                }
            }
        }

        debug!(
            session_id = %self.session_id,
            committed_offset = self.committed_offset,
            "session owner finished",
        );
    }

    /// One pass: read whatever the CLI has written, then commit it line by line.
    async fn tail_step(&mut self) -> ControlFlow<()> {
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
    async fn commit_queued(&mut self) -> ControlFlow<()> {
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
                    if let Some(result) = &batch.result {
                        self.on_result(result).await;
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
                    return ControlFlow::Break(());
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

        Ok(self.take_complete_lines())
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
    /// CLI's echo of it is suppressed rather than stored twice. Delivery
    /// guarantees beyond this are out of scope for v1 (ADR 0020); the next task
    /// owns the failure handling of the write itself.
    async fn deliver_input(&mut self, queued: QueuedInput) {
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

        match self.backend.encode_input(&queued.input) {
            Ok(line) => {
                if let Err(err) = stdin.write_all(line.as_bytes()).await {
                    warn!(session_id = %self.session_id, error = %err, "could not write to the CLI's stdin");
                } else if let Err(err) = stdin.flush().await {
                    warn!(session_id = %self.session_id, error = %err, "could not flush the CLI's stdin");
                }
            }
            Err(err) => warn!(
                session_id = %self.session_id,
                error = %err,
                "an accepted input could not be encoded",
            ),
        }
    }

    /// The end-of-run hook, called after a line carrying a `result` committed.
    ///
    /// What follows a `result` differs per session kind — an ephemeral session
    /// is `done`, a conversational one waits for the next turn — and depends on
    /// `terminal_reason` to tell a stop from a failure
    /// (`ARCHITECTURE.md`, "Stop semantics", "Session lifecycle"). That is the
    /// next task's; here the hook exists, is called in the right place and
    /// records what it was called with.
    async fn on_result(&mut self, result: &ResultSummary) {
        debug!(
            session_id = %self.session_id,
            subtype = %result.subtype,
            terminal_reason = ?result.terminal_reason,
            is_error = result.is_error,
            "a turn ended",
        );
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
                }
            }
        }

        Ok(lines)
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
