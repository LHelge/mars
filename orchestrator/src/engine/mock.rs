//! The engine mock, compiled only with the `integration-tests` feature.
//!
//! `ARCHITECTURE.md`, "Orchestrator internals": every `Arc<dyn Trait>` in
//! [`AppState`] has a mock behind this feature so the whole API can be tested
//! without an engine. [`MockEngine`] is that mock: it keeps a small in-memory
//! container table, answers every [`ContainerEngine`] operation from it, and
//! records what it was asked — the spec it was given, the signals it was sent,
//! the bytes written to an attached stdin, the networks connected — so a test
//! can assert the orchestrator's side of an interaction with no socket
//! anywhere.
//!
//! **The normalised semantics are the interface.** What every operation
//! answers for a container that is missing, one that has exited and one that is
//! already running is `ARCHITECTURE.md`, "Engine adapter", Normalised
//! semantics, and this mock answers exactly that: a second `start` is `Ok`, a
//! `stop` of a container that is not running is `Ok`, a `remove` of a container
//! that is not there is `Ok`, a `kill` of one that is not running is
//! `Conflict`, an unforced `remove` of a running one is `Conflict`, and a write
//! to an attached stdin after the container exited fails instead of being
//! silently lost. `tests/engine_mock.rs` runs the conformance suite against
//! this type on every run of the test suite, so a normalisation that drifts
//! from the list is a failing test rather than a mock with an opinion of its
//! own. Nothing here normalises anything the list does not name.
//!
//! **What the test drives.** The mock never ends a container on its own
//! schedule: a container runs until the test calls [`MockEngine::exit`] with
//! the code the CLI would have exited on, or [`MockEngine::vanish`] to make it
//! disappear the way one reaped behind the orchestrator's back does
//! (`ARCHITECTURE.md`, "Restart procedure": a session whose container is gone
//! is parked). [`ContainerEngine::kill`] therefore records the signal and
//! leaves the container running, which is the shape the stop sequence needs:
//! `SIGINT`, then `SIGTERM` after the grace period (`ARCHITECTURE.md`, "Stop
//! semantics") — unless the container's command traps the signal, in which case
//! the mock exits it on the trap's code, because the contract's own
//! signal-delivery scenario is a command that traps `INT` and exits on a code
//! only a delivered signal could have produced. A command that ignores `TERM`
//! (`trap "" TERM`) is stopped the way an engine stops it: the stop lasts the
//! whole grace period, the container running throughout, and ends in the hard
//! kill's [`KILL_EXIT_CODE`] — the one wait in the mock that takes real time,
//! because the contract's grace-period scenario lists during it. A session's
//! command (`claude`) traps nothing, so nothing a session test drives changes.
//!
//! **Locking.** One `Mutex` guards the whole table and is never held across an
//! await: every operation takes what it needs, drops the guard, and only then
//! resolves the `oneshot`s a [`ContainerEngine::wait`] is parked on.
//!
//! **Secrets.** A recorded [`ContainerSpec`] carries a session's resolved
//! secrets in `ContainerSpec::secret_env`, still in their `Zeroizing` buffers,
//! and is kept here only so a test can read it back — which is why
//! [`MockEngine::specs`] and [`MockEngine::spec_of`] hand the whole spec over
//! with the values readable: this module exists only behind the
//! `integration-tests` feature and is never compiled into the shipped binary.
//! Nothing in this module logs, and `ContainerSpec`'s own redacting [`Debug`]
//! keeps a failing assertion's output free of values (CLAUDE.md rule 3).

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::pin::Pin;
// Shadows the prelude's one-parameter `Result<T>` alias; see [`super`] for why
// the engine trait returns `Result<T, EngineError>`.
use std::result::Result;
use std::sync::{Mutex, MutexGuard};
use std::task::{Context, Poll};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use tokio::io::AsyncWrite;
use tokio::sync::{Notify, oneshot};
use uuid::Uuid;

use super::{
    ContainerEngine, ContainerId, ContainerInfo, ContainerSpec, ContainerState, ContainerSummary,
    EngineError, EngineKind, ExecSession, ExitStatus, LABEL_SESSION_ID, Signal, StdinWriter,
};
// The crate convention (`CLAUDE.md`, "Backend conventions"); the mock reports
// the engine's own [`EngineError`], so the glob is here for [`Arc`] and for
// the doc links.
use crate::prelude::*;

/// The exit code [`ContainerEngine::stop`] leaves behind.
///
/// `ARCHITECTURE.md`, "Stop semantics": a plain stop is the engine's own
/// `SIGTERM`, which the CLI exits 143 on. The mock uses the same code, so a
/// test that stops a container sees what production would record.
pub const STOP_EXIT_CODE: i64 = 143;

/// The exit code [`ContainerEngine::stop`] leaves behind for a command that
/// ignores `TERM`: the engine's `SIGKILL` once the grace period is over.
pub const KILL_EXIT_CODE: i64 = 137;

/// The exit code [`MockExecSession::close`] reports.
///
/// The mock runs no process, so an exec always ends the way the terminal view's
/// shell ends when the client closes the socket: cleanly. It travels the same
/// path `BollardExec::close` puts the engine's own code on, which is what
/// `terminal_closed`'s `exit_code` carries (`SPEC.md`, "WebSocket: session
/// stream").
pub const EXEC_EXIT_CODE: i64 = 0;

/// One recorded [`ContainerEngine::exec_pty`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    /// The container the exec was started in.
    pub container: ContainerId,
    /// The command; `["/bin/bash", "-l"]` for the terminal view.
    pub cmd: Vec<String>,
    /// The user the exec runs as; `agent` for the terminal view.
    pub user: String,
    /// The client's initial width in character cells.
    pub cols: u16,
    /// The client's initial height in character cells.
    pub rows: u16,
}

/// One container the mock is pretending to run.
#[derive(Debug)]
struct MockContainer {
    /// The spec it was created from.
    spec: ContainerSpec,
    /// Where it is in its lifecycle.
    state: ContainerState,
    /// When the mock was asked to create it, which is what
    /// [`ContainerSummary::created`] reports. A test that needs an older
    /// container moves it with [`MockEngine::set_created`].
    created_at: DateTime<Utc>,
    /// The host pid [`ContainerEngine::inspect`] reports while it runs.
    pid: i64,
    /// Every signal [`ContainerEngine::kill`] sent, in order.
    signals: Vec<Signal>,
    /// Every grace period [`ContainerEngine::stop`] was given, in order.
    stop_grace: Vec<u32>,
    /// Everything written to an attached stdin, in order and across every
    /// attach: recovery reattaches, and the second writer appends to the
    /// first one's bytes.
    stdin: Vec<u8>,
    /// The networks [`ContainerEngine::connect_network`] attached, beyond the
    /// one the container was created on.
    connections: Vec<String>,
    /// What the container's command does with a signal, by signal name, read
    /// off the spec's `cmd` by [`traps_of`]. Empty for a command that traps
    /// nothing, which is every session's.
    traps: BTreeMap<String, Trap>,
    /// Everyone parked in [`ContainerEngine::wait`].
    exit_waiters: Vec<oneshot::Sender<Result<ExitStatus, EngineError>>>,
}

impl MockContainer {
    /// End this container on `code` and hand back everyone who was parked on
    /// it, to be woken once the lock is gone.
    fn end(&mut self, code: i64) -> Vec<oneshot::Sender<Result<ExitStatus, EngineError>>> {
        self.state = ContainerState::Exited { code };
        std::mem::take(&mut self.exit_waiters)
    }
}

/// Resolve everyone who was parked in [`ContainerEngine::wait`] on a container
/// that has now exited.
///
/// Always called with the mock's lock already dropped: a `oneshot` send is
/// cheap, but the rule is that the mutex is never held while anything else
/// runs.
fn wake(waiters: Vec<oneshot::Sender<Result<ExitStatus, EngineError>>>, code: i64) {
    for waiter in waiters {
        let _ = waiter.send(Ok(ExitStatus {
            code,
            oom_killed: false,
        }));
    }
}

/// Whether writes to an attached stdin still reach the container.
///
/// A created or running container takes bytes; a container that has ended is
/// one whose attachment the engine has closed, and a write to it fails
/// (`ARCHITECTURE.md`, "Engine adapter", the stdin row).
fn takes_stdin(state: &ContainerState) -> bool {
    matches!(state, ContainerState::Created | ContainerState::Running)
}

/// What a command's `trap` does with a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trap {
    /// `trap "exit <code>" <SIGNAL>`: the command exits on that code.
    Exit(i64),
    /// `trap "" <SIGNAL>`: the command ignores the signal and keeps running.
    Ignore,
}

/// What a command does with each signal, by signal name, as a shell `trap
/// "exit <code>" <SIGNAL>` or `trap "" <SIGNAL>` in its own argument list
/// declares it.
///
/// The mock runs nothing, so the command line is the only thing it knows about
/// the process, and a `trap` in it is the command saying what a delivered
/// signal does to it. That is what lets the conformance suite assert signal
/// delivery through an exit code the engine could not have produced by tearing
/// the container down, and a stop that lasts its grace period, without the
/// suite reaching for anything but `ContainerEngine`.
///
/// Anything it does not recognise — a handler that is neither an `exit` nor
/// empty, a signal name that is not one of [`Signal`]'s, `KILL`, which no
/// shell can trap — is left out, and a signal with no entry is one the mock
/// records and nothing more.
fn traps_of(cmd: &[String]) -> BTreeMap<String, Trap> {
    let mut traps = BTreeMap::new();

    for statement in cmd
        .iter()
        .flat_map(|part| part.split([';', '\n']))
        .map(str::trim)
    {
        let Some(rest) = statement.strip_prefix("trap ") else {
            continue;
        };
        let Some((handler, signals)) = quoted(rest.trim_start()) else {
            continue;
        };
        let handler = handler.trim();
        let trap = if handler.is_empty() {
            Trap::Ignore
        } else {
            let Some(code) = handler
                .strip_prefix("exit ")
                .and_then(|code| code.trim().parse::<i64>().ok())
            else {
                continue;
            };
            Trap::Exit(code)
        };

        for name in signals.split_whitespace() {
            if let Some(signal) = trappable_signal(name) {
                traps.insert(signal.to_string(), trap);
            }
        }
    }

    traps
}

/// The contents of a leading single- or double-quoted word and whatever
/// follows it, or `None` if the text does not start with a closed quote.
fn quoted(text: &str) -> Option<(&str, &str)> {
    let mut chars = text.chars();
    let quote = chars.next()?;

    if quote != '"' && quote != '\'' {
        return None;
    }

    let body = chars.as_str();
    let end = body.find(quote)?;
    Some((&body[..end], &body[end + quote.len_utf8()..]))
}

/// The signal one `trap` operand names, with or without its `SIG` prefix and in
/// any case, or `None` for a name no [`Signal`] covers or one a shell cannot
/// trap.
fn trappable_signal(name: &str) -> Option<Signal> {
    let upper = name.to_ascii_uppercase();

    match upper.strip_prefix("SIG").unwrap_or(&upper) {
        "INT" => Some(Signal::Sigint),
        "TERM" => Some(Signal::Sigterm),
        _ => None,
    }
}

/// Everything the mock remembers, behind one lock.
#[derive(Debug, Default)]
struct MockState {
    /// The live containers, by id. A removed or vanished one is gone from
    /// here, which is what lets a relaunch reuse its name.
    containers: BTreeMap<String, MockContainer>,
    /// The number the next `mock-<n>` id is minted from.
    next_id: u64,
    /// Every spec the mock was asked to create, in order, including the ones
    /// whose containers have since been removed.
    created: Vec<ContainerSpec>,
    /// Every [`ContainerEngine::ensure_network`], as `(name, internal)`.
    networks: Vec<(String, bool)>,
    /// Every image [`ContainerEngine::pull_image`] pulled, in order.
    pulled: Vec<String>,
    /// The images [`ContainerEngine::image_exists`] answers `false` for.
    missing_images: BTreeSet<String>,
    /// The message the next pull fails with, if a test armed one.
    next_pull_error: Option<String>,
    /// The message the next [`ContainerEngine::list_by_label`] fails with, if
    /// a test armed one: the engine being unreachable is the one failure a
    /// listing sweep reports as its own `Err`.
    next_list_error: Option<String>,
    /// Every [`ContainerEngine::exec_pty`], in order.
    exec_requests: Vec<ExecRequest>,
    /// Output queued for a container's exec session, oldest first.
    exec_output: BTreeMap<String, VecDeque<Bytes>>,
    /// Every [`ExecSession::resize`], by container, in order.
    exec_resizes: BTreeMap<String, Vec<(u16, u16)>>,
    /// The containers whose exec session was closed.
    exec_closed: BTreeSet<String>,
    /// Woken whenever a parked [`MockExecSession::read`] may have something
    /// new to report: output was scripted, or a container stopped running.
    ///
    /// An [`Arc`] inside the state rather than a field beside it, so a reader
    /// can clone it out under the lock and park on it with the guard gone.
    exec_wake: Arc<Notify>,
    /// Whether [`ContainerEngine::ping`] fails.
    unhealthy: bool,
    /// Whether [`ContainerEngine::remove`] parks instead of removing.
    ///
    /// The clean-up a session owner does after it has written its `parked`
    /// state change is a container removal, and under Podman it takes a few
    /// hundred milliseconds. A test that means to arrange what arrives in that
    /// window holds the removal here rather than hoping to hit it.
    removals_held: bool,
    /// Whether [`ContainerEngine::start`] parks instead of starting.
    ///
    /// The launch window: a session is `creating` from the moment its row
    /// exists until its container is started and stdin attached, and that is
    /// where an `end` has to be able to arrive. Holding the start here is what
    /// lets a test stand inside `creating` with the container created rather
    /// than race the launcher to it.
    starts_held: bool,
    /// How many pings the mock was asked for, healthy or not.
    pings: usize,
}

/// An engine that answers from an in-memory container table and remembers
/// everything it was asked.
///
/// `TestApp::spawn()` builds one, stores it in `AppState.engine` and hands it
/// out as `app.engine()`; a handler that only sees the
/// `Arc<dyn ContainerEngine>` reaches the same allocation through
/// [`ContainerEngine::as_any`] (`CLAUDE.md`, "Testing expectations").
#[derive(Debug)]
pub struct MockEngine {
    /// The kind [`ContainerEngine::kind`] reports; it decides whether the spec
    /// builder sets `UsernsMode: keep-id` (ADR 0004).
    kind: EngineKind,
    /// Shared with every stdin writer and exec session the mock handed out, so
    /// what a caller writes lands where a test reads it.
    state: Arc<Mutex<MockState>>,
}

impl Default for MockEngine {
    /// Podman: the target engine, so a code path that branches on the kind
    /// takes the branch production takes (ADR 0004).
    fn default() -> Self {
        Self::new(EngineKind::Podman)
    }
}

impl MockEngine {
    /// A fresh mock with no containers and nothing recorded.
    pub fn new(kind: EngineKind) -> Self {
        Self {
            kind,
            state: Arc::new(Mutex::new(MockState::default())),
        }
    }

    // ---- what a test reads back --------------------------------------------

    /// Every spec the mock was asked to create, in order.
    ///
    /// A creation log rather than a view of the table: a spec stays here after
    /// its container is removed, so a test can assert what a relaunch built.
    ///
    /// The clone carries `ContainerSpec::secret_env` with its values readable,
    /// so a test can assert what a launch injected; that is sound only because
    /// this whole module is behind the `integration-tests` feature.
    pub fn specs(&self) -> Vec<ContainerSpec> {
        self.lock().created.clone()
    }

    /// The spec one live container was created from, secret values included for
    /// the same reason [`MockEngine::specs`] includes them.
    pub fn spec_of(&self, id: &ContainerId) -> Option<ContainerSpec> {
        self.lock()
            .containers
            .get(&id.0)
            .map(|container| container.spec.clone())
    }

    /// Where one container is in its lifecycle, or `None` once it is removed
    /// or vanished.
    pub fn state_of(&self, id: &ContainerId) -> Option<ContainerState> {
        self.lock()
            .containers
            .get(&id.0)
            .map(|container| container.state.clone())
    }

    /// The signals [`ContainerEngine::kill`] sent to one container, in order:
    /// the stop sequence's `SIGINT` and then `SIGTERM` (`ARCHITECTURE.md`,
    /// "Stop semantics").
    pub fn signals(&self, id: &ContainerId) -> Vec<Signal> {
        self.lock()
            .containers
            .get(&id.0)
            .map(|container| container.signals.clone())
            .unwrap_or_default()
    }

    /// The grace periods [`ContainerEngine::stop`] was given, in order.
    pub fn stop_grace(&self, id: &ContainerId) -> Vec<u32> {
        self.lock()
            .containers
            .get(&id.0)
            .map(|container| container.stop_grace.clone())
            .unwrap_or_default()
    }

    /// Everything written to one container's stdin, across every attach.
    pub fn stdin_bytes(&self, id: &ContainerId) -> Vec<u8> {
        self.lock()
            .containers
            .get(&id.0)
            .map(|container| container.stdin.clone())
            .unwrap_or_default()
    }

    /// [`stdin_bytes`](Self::stdin_bytes) split on `\n`, which is one line per
    /// stream-json message the session owner wrote. A trailing newline does
    /// not add an empty last line.
    pub fn stdin_lines(&self, id: &ContainerId) -> Vec<String> {
        let bytes = self.stdin_bytes(id);
        let text = String::from_utf8_lossy(&bytes);
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();

        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }

    /// The networks [`ContainerEngine::connect_network`] attached to one
    /// container, beyond the one it was created on.
    pub fn connections(&self, id: &ContainerId) -> Vec<String> {
        self.lock()
            .containers
            .get(&id.0)
            .map(|container| container.connections.clone())
            .unwrap_or_default()
    }

    /// Every [`ContainerEngine::ensure_network`], as `(name, internal)`: the
    /// sessions network is internal and the egress network is not
    /// (`ARCHITECTURE.md`, "Networking").
    pub fn networks(&self) -> Vec<(String, bool)> {
        self.lock().networks.clone()
    }

    /// Every image [`ContainerEngine::pull_image`] pulled, in order.
    pub fn pulled_images(&self) -> Vec<String> {
        self.lock().pulled.clone()
    }

    /// Every [`ContainerEngine::exec_pty`], in order.
    pub fn exec_requests(&self) -> Vec<ExecRequest> {
        self.lock().exec_requests.clone()
    }

    /// Every [`ExecSession::resize`] on one container's exec session, in
    /// order, as `(cols, rows)`.
    pub fn exec_resizes(&self, id: &ContainerId) -> Vec<(u16, u16)> {
        self.lock()
            .exec_resizes
            .get(&id.0)
            .cloned()
            .unwrap_or_default()
    }

    /// Whether one container's exec session was closed, so a terminal test can
    /// assert the handler cleaned up.
    pub fn exec_closed(&self, id: &ContainerId) -> bool {
        self.lock().exec_closed.contains(&id.0)
    }

    /// How many times [`ContainerEngine::ping`] was called, healthy or not.
    pub fn pings(&self) -> usize {
        self.lock().pings
    }

    /// The id of the container carrying this session's [`LABEL_SESSION_ID`],
    /// if one is live.
    pub fn container_id_for_session(&self, session_id: Uuid) -> Option<ContainerId> {
        let wanted = session_id.to_string();

        self.lock()
            .containers
            .iter()
            .find(|(_, container)| container.spec.labels.get(LABEL_SESSION_ID) == Some(&wanted))
            .map(|(id, _)| ContainerId(id.clone()))
    }

    // ---- what a test arranges ----------------------------------------------

    /// End a container the way the CLI exiting would: its state becomes
    /// [`ContainerState::Exited`] and every parked [`ContainerEngine::wait`]
    /// resolves with the code.
    ///
    /// Returns whether there was such a container.
    pub fn exit(&self, id: &ContainerId, code: i64) -> bool {
        let waiters = {
            let mut state = self.lock();
            let Some(container) = state.containers.get_mut(&id.0) else {
                return false;
            };

            container.end(code)
        };

        wake(waiters, code);
        self.wake_execs();
        true
    }

    /// Make a container disappear, the way one reaped behind the
    /// orchestrator's back has: [`ContainerEngine::inspect`] and
    /// [`ContainerEngine::wait`] answer [`EngineError::NotFound`] afterwards,
    /// which is recovery's "the container is gone" path (`ARCHITECTURE.md`,
    /// "Restart procedure").
    ///
    /// Returns whether there was such a container.
    pub fn vanish(&self, id: &ContainerId) -> bool {
        self.drop_container(id)
    }

    /// Queue output the container's exec session reads before it echoes
    /// anything written to it. May be called before the exec is started.
    pub fn script_exec_output(&self, id: &ContainerId, bytes: impl Into<Bytes>) {
        self.lock()
            .exec_output
            .entry(id.0.clone())
            .or_default()
            .push_back(bytes.into());
        self.wake_execs();
    }

    /// The images [`ContainerEngine::image_exists`] answers `false` for; every
    /// other image exists. A successful pull removes one from the set.
    pub fn set_missing_images<I, S>(&self, images: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.lock().missing_images = images.into_iter().map(Into::into).collect();
    }

    /// Move a container's creation time, which is what
    /// [`ContainerSummary::created`] reports and what orphan cleanup's
    /// five-minute guard is measured against (`ARCHITECTURE.md`, "Background
    /// jobs"). The mock creates every container `now`, so a test that needs an
    /// older one says so here.
    ///
    /// Returns whether there was such a container.
    pub fn set_created(&self, id: &ContainerId, created: DateTime<Utc>) -> bool {
        match self.lock().containers.get_mut(&id.0) {
            Some(container) => {
                container.created_at = created;
                true
            }
            None => false,
        }
    }

    /// Make the next [`ContainerEngine::list_by_label`] fail with
    /// [`EngineError::Connection`] carrying this message: the engine being
    /// unreachable, which a sweep over a listing reports as its own `Err`.
    pub fn fail_next_list(&self, message: impl Into<String>) {
        self.lock().next_list_error = Some(message.into());
    }

    /// Make the next [`ContainerEngine::pull_image`] fail with
    /// [`EngineError::ImagePull`] carrying this message — the message the
    /// launcher puts into `sessions.error` verbatim.
    pub fn fail_next_pull(&self, message: impl Into<String>) {
        self.lock().next_pull_error = Some(message.into());
    }

    /// Whether [`ContainerEngine::ping`] fails, which is what `GET
    /// /api/health` reports as `engine: false` (`SPEC.md`, "Health").
    pub fn set_unhealthy(&self, unhealthy: bool) {
        self.lock().unhealthy = unhealthy;
    }

    /// Hold every [`ContainerEngine::remove`] until
    /// [`MockEngine::resume_removals`] lets it go.
    ///
    /// The window a session owner is in between committing the state change
    /// that leaves `running` and finishing with the session: a removal takes a
    /// few hundred milliseconds under Podman, and holding it here is what makes
    /// a test of what arrives in that window deterministic rather than timed.
    pub fn hold_removals(&self) {
        self.lock().removals_held = true;
    }

    /// Let every held [`ContainerEngine::remove`] finish, and stop holding new
    /// ones.
    pub fn resume_removals(&self) {
        self.lock().removals_held = false;
    }

    /// Hold every [`ContainerEngine::start`] until
    /// [`MockEngine::resume_starts`] lets it go.
    ///
    /// The window a launch is in while its session is `creating` and its
    /// container already exists. A test that means to arrange what arrives in
    /// that window — an `end` of a session the user launched by mistake —
    /// holds the start here rather than hoping to hit it.
    pub fn hold_starts(&self) {
        self.lock().starts_held = true;
    }

    /// Let every held [`ContainerEngine::start`] finish, and stop holding new
    /// ones.
    pub fn resume_starts(&self) {
        self.lock().starts_held = false;
    }

    // ---- internals ---------------------------------------------------------

    /// Test-only code: a poisoned lock means another test thread already
    /// panicked, which is a failure in its own right.
    fn lock(&self) -> MutexGuard<'_, MockState> {
        self.state.lock().expect("the mock engine lock is healthy")
    }

    /// Wake every parked [`MockExecSession::read`], so it re-reads the table.
    ///
    /// Called after anything that gives a PTY something to say: new scripted
    /// output, or a container that has stopped running and whose exec the
    /// engine would therefore have torn down. The clone is taken under the
    /// lock and the notification sent with the guard gone, like every other
    /// wake-up in this module.
    fn wake_execs(&self) {
        let wake = Arc::clone(&self.lock().exec_wake);
        wake.notify_waiters();
    }

    /// Drop a container and fail everyone parked on it, which is what both
    /// [`ContainerEngine::remove`] and [`vanish`](Self::vanish) do to the
    /// table; they differ only in what they check first.
    fn drop_container(&self, id: &ContainerId) -> bool {
        let removed = self.lock().containers.remove(&id.0);

        match removed {
            Some(container) => {
                for waiter in container.exit_waiters {
                    let _ = waiter.send(Err(EngineError::NotFound(id.0.clone())));
                }
                // A container that is gone takes its exec with it.
                self.wake_execs();
                true
            }
            None => false,
        }
    }
}

/// The one container, or [`EngineError::NotFound`] naming the id that was not
/// there — the variant every caller branches on.
fn container<'a>(
    state: &'a mut MockState,
    id: &ContainerId,
) -> Result<&'a mut MockContainer, EngineError> {
    state
        .containers
        .get_mut(&id.0)
        .ok_or_else(|| EngineError::NotFound(id.0.clone()))
}

#[async_trait]
impl ContainerEngine for MockEngine {
    fn kind(&self) -> EngineKind {
        self.kind
    }

    async fn ping(&self) -> Result<(), EngineError> {
        let mut state = self.lock();
        state.pings += 1;

        if state.unhealthy {
            return Err(EngineError::Connection(
                "the mock engine was set unhealthy".to_string(),
            ));
        }
        Ok(())
    }

    async fn ensure_network(&self, name: &str, internal: bool) -> Result<(), EngineError> {
        self.lock().networks.push((name.to_string(), internal));
        Ok(())
    }

    async fn image_exists(&self, image: &str) -> Result<bool, EngineError> {
        Ok(!self.lock().missing_images.contains(image))
    }

    async fn pull_image(&self, image: &str) -> Result<(), EngineError> {
        let mut state = self.lock();

        if let Some(message) = state.next_pull_error.take() {
            return Err(EngineError::ImagePull {
                image: image.to_string(),
                message,
            });
        }

        state.pulled.push(image.to_string());
        // The image is here now, which is what makes a launcher's "missing, so
        // pull, then create" path testable end to end.
        state.missing_images.remove(image);
        Ok(())
    }

    async fn create(&self, spec: &ContainerSpec) -> Result<ContainerId, EngineError> {
        let mut state = self.lock();

        if state
            .containers
            .values()
            .any(|container| container.spec.name == spec.name)
        {
            return Err(EngineError::Conflict(format!(
                "the container name {} is already in use",
                spec.name
            )));
        }

        let number = state.next_id;
        state.next_id += 1;
        let id = ContainerId(format!("mock-{number}"));

        state.created.push(spec.clone());
        state.containers.insert(
            id.0.clone(),
            MockContainer {
                spec: spec.clone(),
                state: ContainerState::Created,
                created_at: Utc::now(),
                // Deterministic and obviously not a real pid, so a test can
                // assert on it without pretending to know the host.
                pid: 10_000 + number as i64,
                signals: Vec::new(),
                stop_grace: Vec::new(),
                stdin: Vec::new(),
                connections: Vec::new(),
                traps: traps_of(&spec.cmd),
                exit_waiters: Vec::new(),
            },
        );

        Ok(id)
    }

    async fn connect_network(&self, id: &ContainerId, network: &str) -> Result<(), EngineError> {
        let mut state = self.lock();
        container(&mut state, id)?
            .connections
            .push(network.to_string());
        Ok(())
    }

    /// A container that is already running is `Ok` and is left alone: both
    /// engines answer 304 and the adapter reads it as success, so a start that
    /// races another start cannot fail on it.
    async fn start(&self, id: &ContainerId) -> Result<(), EngineError> {
        // A poll rather than a `Notify`, for the reason `remove` polls: the
        // flag is read under the same lock as everything else here.
        while self.lock().starts_held {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let mut state = self.lock();
        let container = container(&mut state, id)?;

        if container.state.is_running() {
            return Ok(());
        }

        container.state = ContainerState::Running;
        Ok(())
    }

    /// Records the grace period and ends the container on the code its command's
    /// `TERM` trap names, or [`STOP_EXIT_CODE`] when it traps nothing, the way
    /// the engine's own stop does. A command that ignores `TERM` is what an
    /// engine hard-kills once the grace period is over: the stop takes that
    /// long, the container running all the while, and it ends on
    /// [`KILL_EXIT_CODE`].
    ///
    /// A container that is not running is `Ok` and is left exactly as it ended:
    /// being stopped is what the caller asked for and it already is, which is
    /// the 304 both engines answer.
    async fn stop(&self, id: &ContainerId, grace_secs: u32) -> Result<(), EngineError> {
        let term = {
            let mut state = self.lock();
            let container = container(&mut state, id)?;

            // Recorded whether or not there was anything to stop: `stop_grace`
            // is the log of what the mock was asked, not of what it did.
            container.stop_grace.push(grace_secs);

            if !container.state.is_running() {
                return Ok(());
            }

            container.traps.get(&Signal::Sigterm.to_string()).copied()
        };

        let code = match term {
            Some(Trap::Exit(code)) => code,
            None => STOP_EXIT_CODE,
            Some(Trap::Ignore) => {
                // The grace period, with the lock dropped, so a listing or an
                // inspect in the meantime sees the container still running.
                tokio::time::sleep(std::time::Duration::from_secs(grace_secs.into())).await;
                KILL_EXIT_CODE
            }
        };

        let waiters = {
            let mut state = self.lock();
            match state.containers.get_mut(&id.0) {
                // Whatever ended it during the grace period — a kill, a test's
                // `exit`, a forced removal — already answered its waiters.
                Some(container) if container.state.is_running() => container.end(code),
                _ => return Ok(()),
            }
        };

        wake(waiters, code);
        self.wake_execs();
        Ok(())
    }

    /// Records the signal and delivers it to the command: a command that traps
    /// it exits on the trap's code, and one that traps nothing — every
    /// session's `claude` — is left running for the test to end with
    /// [`MockEngine::exit`].
    ///
    /// A container that is not running is [`EngineError::Conflict`], which the
    /// session owner reads as "it is already gone".
    async fn kill(&self, id: &ContainerId, signal: Signal) -> Result<(), EngineError> {
        let ended = {
            let mut state = self.lock();
            let container = container(&mut state, id)?;

            if !container.state.is_running() {
                return Err(EngineError::Conflict(format!(
                    "container {id} is not running"
                )));
            }

            container.signals.push(signal);

            // An ignored signal is one the command records and nothing more.
            match container.traps.get(&signal.to_string()).copied() {
                Some(Trap::Exit(code)) => Some((code, container.end(code))),
                Some(Trap::Ignore) | None => None,
            }
        };

        if let Some((code, waiters)) = ended {
            wake(waiters, code);
            self.wake_execs();
        }
        Ok(())
    }

    /// A container that is not there is `Ok`, because a container that is not
    /// there is already removed; a running one is [`EngineError::Conflict`]
    /// without `force` and `Ok` with it.
    async fn remove(&self, id: &ContainerId, force: bool) -> Result<(), EngineError> {
        // A poll rather than a `Notify`: the flag is read under the same lock
        // everything else here is, and a five-millisecond tick costs a test
        // nothing while a missed wake-up would hang it.
        while self.lock().removals_held {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        {
            let state = self.lock();
            let Some(container) = state.containers.get(&id.0) else {
                return Ok(());
            };

            if container.state.is_running() && !force {
                return Err(EngineError::Conflict(format!(
                    "container {id} is still running"
                )));
            }
        }

        self.drop_container(id);
        Ok(())
    }

    async fn inspect(&self, id: &ContainerId) -> Result<ContainerInfo, EngineError> {
        let mut state = self.lock();
        let container = container(&mut state, id)?;

        // The network the container was created on first, then every
        // `connect_network`: the order the launcher attaches them in.
        let mut networks = vec![container.spec.network.clone()];
        networks.extend(container.connections.iter().cloned());

        Ok(ContainerInfo {
            id: id.clone(),
            name: container.spec.name.clone(),
            labels: container.spec.labels.clone(),
            state: container.state.clone(),
            networks,
            pid: container.state.is_running().then_some(container.pid),
        })
    }

    async fn wait(&self, id: &ContainerId) -> Result<ExitStatus, EngineError> {
        let receiver = {
            let mut state = self.lock();
            let container = container(&mut state, id)?;

            if let ContainerState::Exited { code } = &container.state {
                return Ok(ExitStatus {
                    code: *code,
                    oom_killed: false,
                });
            }

            let (sender, receiver) = oneshot::channel();
            container.exit_waiters.push(sender);
            receiver
        };

        // The guard is gone before the await: parking here is what a wait on a
        // running container is for.
        match receiver.await {
            Ok(result) => result,
            // The sender went away without a verdict, which only happens if
            // the mock itself was dropped under the waiter.
            Err(_) => Err(EngineError::NotFound(id.0.clone())),
        }
    }

    async fn list_by_label(&self, label_key: &str) -> Result<Vec<ContainerSummary>, EngineError> {
        let mut state = self.lock();

        if let Some(message) = state.next_list_error.take() {
            return Err(EngineError::Connection(message));
        }

        Ok(state
            .containers
            .iter()
            .filter(|(_, container)| container.spec.labels.contains_key(label_key))
            .map(|(id, container)| ContainerSummary {
                id: ContainerId(id.clone()),
                name: container.spec.name.clone(),
                labels: container.spec.labels.clone(),
                running: container.state.is_running(),
                created: container.created_at,
            })
            .collect())
    }

    async fn attach_stdin(&self, id: &ContainerId) -> Result<Box<dyn StdinWriter>, EngineError> {
        {
            let mut state = self.lock();
            container(&mut state, id)?;
        }

        Ok(Box::new(MockStdin {
            state: Arc::clone(&self.state),
            id: id.clone(),
        }))
    }

    async fn exec_pty(
        &self,
        id: &ContainerId,
        cmd: &[String],
        user: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Box<dyn ExecSession>, EngineError> {
        {
            let mut state = self.lock();
            let container = container(&mut state, id)?;

            if !container.state.is_running() {
                return Err(EngineError::Conflict(format!(
                    "container {id} is not running"
                )));
            }

            state.exec_requests.push(ExecRequest {
                container: id.clone(),
                cmd: cmd.to_vec(),
                user: user.to_string(),
                cols,
                rows,
            });
        }

        Ok(Box::new(MockExecSession {
            state: Arc::clone(&self.state),
            id: id.clone(),
            echo: VecDeque::new(),
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The write half of an attached container's stdin: everything written lands
/// in the container's own buffer, where [`MockEngine::stdin_bytes`] reads it
/// back.
///
/// Every attach on one container writes into the same buffer, because recovery
/// reattaches to a container the previous owner already wrote to.
///
/// A write after the container ended fails with the [`io::ErrorKind::BrokenPipe`]
/// `BollardStdin` answers with, never a silent success: a lost input the session
/// owner was told had gone through is the failure the rule exists for
/// (`ARCHITECTURE.md`, "Engine adapter", the stdin row). Bytes written before
/// the exit stay in the buffer, so [`MockEngine::stdin_bytes`] still reports
/// them.
#[derive(Debug)]
struct MockStdin {
    /// The mock's table, shared with the engine that handed this out.
    state: Arc<Mutex<MockState>>,
    /// The container being written to.
    id: ContainerId,
}

impl AsyncWrite for MockStdin {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let mut state = this.state.lock().expect("the mock engine lock is healthy");

        match state.containers.get_mut(&this.id.0) {
            Some(container) if takes_stdin(&container.state) => {
                container.stdin.extend_from_slice(buf);
                Poll::Ready(Ok(buf.len()))
            }
            // The container ended under the writer: the engine closes the
            // attachment and the next write is a broken pipe, which is what
            // the session owner records as a failed input.
            Some(_) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("the attach stream of container {} has closed", this.id),
            ))),
            // The container vanished under the writer, which is what a session
            // owner writing to a reaped container sees.
            None => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("the mock engine has no container {}", this.id),
            ))),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// A scripted PTY: it reads out whatever [`MockEngine::script_exec_output`]
/// queued, then echoes whatever was written to it, and parks while the
/// container runs and there is nothing to say.
///
/// Echoing is what makes a terminal test meaningful without a shell: the
/// frontend's keystrokes come back as output frames, so the WebSocket's round
/// trip is exercised end to end (`SPEC.md`, "WebSocket: session stream").
///
/// Parking on an empty PTY is what a real one does — a shell at its prompt
/// produces nothing until somebody types — and the terminal's reader reads
/// [`Ok(None)`](Option::None) as "the shell exited", so a mock that reported
/// end of stream the moment it ran dry could never be written to at all. End
/// of stream is therefore the container no longer running, which is what
/// really ends an exec.
#[derive(Debug)]
pub struct MockExecSession {
    /// The mock's table, shared with the engine that started this exec.
    state: Arc<Mutex<MockState>>,
    /// The container the exec runs in; resizes and the close are recorded
    /// against it.
    id: ContainerId,
    /// What was written and has not been read back yet.
    echo: VecDeque<Bytes>,
}

#[async_trait]
impl ExecSession for MockExecSession {
    /// Cancel-safe: everything it might hand back stays in
    /// [`MockState::exec_output`] or [`Self::echo`] until it is returned, and
    /// the park is a [`Notify`] whose permit is re-checked from scratch on the
    /// next call. Dropping this future mid-`await` therefore loses no bytes,
    /// which is what the terminal's `select!` over `read` and its command
    /// channel relies on (`ws::terminal`).
    async fn read(&mut self) -> Result<Option<Bytes>, EngineError> {
        loop {
            // Registered *before* the table is looked at, so a writer between
            // the look and the park cannot be missed.
            let wake = Arc::clone(
                &self
                    .state
                    .lock()
                    .expect("the mock engine lock is healthy")
                    .exec_wake,
            );
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let (scripted, running) = {
                let mut state = self.state.lock().expect("the mock engine lock is healthy");
                let scripted = state
                    .exec_output
                    .get_mut(&self.id.0)
                    .and_then(VecDeque::pop_front);
                let running = state
                    .containers
                    .get(&self.id.0)
                    .is_some_and(|container| container.state.is_running());

                (scripted, running)
            };

            if scripted.is_some() {
                return Ok(scripted);
            }
            if let Some(echoed) = self.echo.pop_front() {
                return Ok(Some(echoed));
            }
            if !running {
                return Ok(None);
            }

            notified.await;
        }
    }

    async fn write(&mut self, data: &[u8]) -> Result<(), EngineError> {
        self.echo.push_back(Bytes::copy_from_slice(data));
        Ok(())
    }

    async fn resize(&mut self, cols: u16, rows: u16) -> Result<(), EngineError> {
        self.state
            .lock()
            .expect("the mock engine lock is healthy")
            .exec_resizes
            .entry(self.id.0.clone())
            .or_default()
            .push((cols, rows));
        Ok(())
    }

    async fn close(self: Box<Self>) -> Result<i64, EngineError> {
        self.state
            .lock()
            .expect("the mock engine lock is healthy")
            .exec_closed
            .insert(self.id.0.clone());
        Ok(EXEC_EXIT_CODE)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use tokio::io::AsyncWriteExt;

    use super::*;
    use crate::engine::{Bind, LABEL_PROJECT_ID, session_container_name};

    /// An obviously fake session id.
    const SESSION_ID: &str = "00000000-0000-4000-8000-00000000abcd";

    /// A spec with the labels and the obviously fake secret a session's has
    /// (rule 3).
    fn session_spec(session_id: &str) -> ContainerSpec {
        ContainerSpec {
            image: "mars-session-claude:dev".to_string(),
            name: format!("mars-session-{session_id}"),
            labels: BTreeMap::from([
                (LABEL_SESSION_ID.to_string(), session_id.to_string()),
                (
                    LABEL_PROJECT_ID.to_string(),
                    "00000000-0000-4000-8000-0000000000b1".to_string(),
                ),
            ]),
            user: "1000:1000".to_string(),
            working_dir: "/session/work".to_string(),
            cmd: vec!["claude".to_string()],
            env: Vec::new(),
            secret_env: vec![(
                "ANTHROPIC_API_KEY".to_string(),
                zeroize::Zeroizing::new("not-a-real-api-key".to_string()),
            )],
            binds: vec![Bind {
                host_source: PathBuf::from("/srv/mars/data/sessions/s/work"),
                container_target: "/session/work".to_string(),
                read_only: false,
            }],
            network: "mars-sessions".to_string(),
            extra_hosts: Vec::new(),
            runtime: None,
        }
    }

    /// A created and started container.
    async fn running(engine: &MockEngine, session_id: &str) -> ContainerId {
        let id = engine
            .create(&session_spec(session_id))
            .await
            .expect("the mock creates");
        engine.start(&id).await.expect("the mock starts");
        id
    }

    #[tokio::test]
    async fn create_start_kill_exit_and_wait_are_one_lifecycle() {
        let engine = MockEngine::default();
        let spec = session_spec(SESSION_ID);

        let id = engine.create(&spec).await.expect("the mock creates");
        assert_eq!(id, ContainerId("mock-0".to_string()));
        assert_eq!(engine.state_of(&id), Some(ContainerState::Created));
        assert_eq!(engine.specs(), vec![spec.clone()]);
        assert_eq!(engine.spec_of(&id), Some(spec));

        engine.start(&id).await.expect("the mock starts");
        assert_eq!(engine.state_of(&id), Some(ContainerState::Running));

        // The stop sequence: SIGINT, then SIGTERM after the grace period.
        engine
            .kill(&id, Signal::Sigint)
            .await
            .expect("the mock signals");
        engine
            .kill(&id, Signal::Sigterm)
            .await
            .expect("the mock signals");
        assert_eq!(engine.signals(&id), vec![Signal::Sigint, Signal::Sigterm]);
        assert_eq!(
            engine.state_of(&id),
            Some(ContainerState::Running),
            "a signal alone must not end the container"
        );

        assert!(engine.exit(&id, 130));
        assert_eq!(
            engine.state_of(&id),
            Some(ContainerState::Exited { code: 130 })
        );

        let status = engine.wait(&id).await.expect("an exited container waits");
        assert_eq!(
            status,
            ExitStatus {
                code: 130,
                oom_killed: false
            },
            "a wait on an already exited container resolves at once"
        );
    }

    #[tokio::test]
    async fn a_wait_on_a_running_container_parks_until_it_exits() {
        let engine = Arc::new(MockEngine::default());
        let id = running(&engine, SESSION_ID).await;

        let mut waiting = tokio::spawn({
            let engine = Arc::clone(&engine);
            let id = id.clone();
            async move { engine.wait(&id).await }
        });

        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut waiting)
                .await
                .is_err(),
            "the wait must park while the container runs"
        );

        engine.exit(&id, 0);

        let status = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the wait resolves once the container exits")
            .expect("the waiting task did not panic")
            .expect("the container exited cleanly");
        assert_eq!(
            status,
            ExitStatus {
                code: 0,
                oom_killed: false
            }
        );
    }

    #[tokio::test]
    async fn a_vanished_container_is_not_found_by_inspect_or_by_a_parked_wait() {
        let engine = Arc::new(MockEngine::default());
        let id = running(&engine, SESSION_ID).await;

        let waiting = tokio::spawn({
            let engine = Arc::clone(&engine);
            let id = id.clone();
            async move { engine.wait(&id).await }
        });
        // Let the wait park before the container goes; either order answers
        // `NotFound`, but this is the one recovery actually sees.
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(engine.vanish(&id));
        assert!(!engine.vanish(&id), "vanishing twice finds nothing");

        let error = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the parked wait is woken")
            .expect("the waiting task did not panic")
            .expect_err("the container is gone");
        assert!(
            matches!(error, EngineError::NotFound(_)),
            "unexpected: {error:?}"
        );

        let error = engine
            .inspect(&id)
            .await
            .expect_err("the container is gone");
        assert!(
            matches!(error, EngineError::NotFound(_)),
            "unexpected: {error:?}"
        );
        assert_eq!(engine.state_of(&id), None);
    }

    #[tokio::test]
    async fn a_stop_records_its_grace_and_ends_the_container() {
        let engine = MockEngine::default();
        let id = running(&engine, SESSION_ID).await;

        engine.stop(&id, 20).await.expect("the mock stops");

        assert_eq!(engine.stop_grace(&id), vec![20]);
        assert_eq!(
            engine.state_of(&id),
            Some(ContainerState::Exited {
                code: STOP_EXIT_CODE
            })
        );

        // A container that has already exited is `Ok` to stop and is left
        // exactly as it ended; the ask is still recorded.
        engine
            .stop(&id, 5)
            .await
            .expect("stopping an exited container is not a failure");
        assert_eq!(engine.stop_grace(&id), vec![20, 5]);
        assert_eq!(
            engine.state_of(&id),
            Some(ContainerState::Exited {
                code: STOP_EXIT_CODE
            }),
            "the second stop changed how the container had ended"
        );
    }

    /// The normalisations of `ARCHITECTURE.md`, "Engine adapter": what the mock
    /// answers where a caller cannot avoid asking twice.
    ///
    /// The conformance suite (`tests/engine_mock.rs`) is the definition; this
    /// is the same rules at the unit level, where a regression names the
    /// operation directly.
    #[tokio::test]
    async fn the_repeatable_operations_are_idempotent_as_the_contract_says() {
        let engine = MockEngine::default();
        let missing = ContainerId("mock-404".to_string());

        // A container that is not there is already removed, with force or
        // without.
        engine
            .remove(&missing, true)
            .await
            .expect("a container that is not there is already removed");
        engine
            .remove(&missing, false)
            .await
            .expect("force makes no difference to a container that is not there");

        let id = running(&engine, SESSION_ID).await;

        engine
            .start(&id)
            .await
            .expect("starting a running container is idempotent");
        assert_eq!(
            engine.state_of(&id),
            Some(ContainerState::Running),
            "the second start disturbed the container"
        );
    }

    #[tokio::test]
    async fn the_refusals_are_the_ones_the_contract_names() {
        let engine = MockEngine::default();
        let missing = ContainerId("mock-404".to_string());

        for error in [
            engine.start(&missing).await.expect_err("nothing to start"),
            engine.stop(&missing, 5).await.expect_err("nothing to stop"),
            engine
                .kill(&missing, Signal::Sigint)
                .await
                .expect_err("nothing to signal"),
            engine
                .inspect(&missing)
                .await
                .expect_err("nothing to inspect"),
            engine
                .wait(&missing)
                .await
                .expect_err("nothing to wait for"),
            engine
                .connect_network(&missing, "mars-egress")
                .await
                .expect_err("nothing to connect"),
            engine
                .attach_stdin(&missing)
                .await
                .err()
                .expect("nothing to attach to"),
        ] {
            assert!(
                matches!(error, EngineError::NotFound(_)),
                "unexpected: {error:?}"
            );
        }

        let id = engine
            .create(&session_spec(SESSION_ID))
            .await
            .expect("the mock creates");

        // `ExecSession` is not `Debug`, so the refusal is matched rather than
        // unwrapped with `expect_err`.
        let refused_exec = match engine
            .exec_pty(&id, &["/bin/bash".to_string()], "agent", 80, 24)
            .await
        {
            Ok(_) => panic!("an exec in a container that is not running must be refused"),
            Err(error) => error,
        };

        // Created, not running: a signal is a conflict on both engines, and so
        // is an exec.
        for error in [
            engine
                .kill(&id, Signal::Sigint)
                .await
                .expect_err("not running"),
            refused_exec,
        ] {
            assert!(
                matches!(error, EngineError::Conflict(_)),
                "unexpected: {error:?}"
            );
        }

        engine.start(&id).await.expect("the mock starts");
        let error = engine
            .remove(&id, false)
            .await
            .expect_err("a running container needs force");
        assert!(
            matches!(error, EngineError::Conflict(_)),
            "unexpected: {error:?}"
        );
        engine.remove(&id, true).await.expect("force removes it");
        assert_eq!(engine.state_of(&id), None);
    }

    /// A signal reaches the command: one that traps it exits on the trap's
    /// code, and one that traps nothing keeps running until the test ends it.
    #[tokio::test]
    async fn a_trapped_signal_ends_the_container_and_an_untrapped_one_does_not() {
        let engine = MockEngine::default();

        // The session's own command traps nothing, which is the shape the stop
        // sequence needs: SIGINT, then SIGTERM after the grace period.
        let session = running(&engine, SESSION_ID).await;
        engine
            .kill(&session, Signal::Sigint)
            .await
            .expect("the mock signals");
        engine
            .kill(&session, Signal::Sigterm)
            .await
            .expect("the mock signals");
        assert_eq!(
            engine.signals(&session),
            vec![Signal::Sigint, Signal::Sigterm]
        );
        assert_eq!(
            engine.state_of(&session),
            Some(ContainerState::Running),
            "a signal a command does not trap must not end the container"
        );

        // A command that traps both, as the conformance suite's does.
        let mut spec = session_spec("00000000-0000-4000-8000-00000000f00d");
        spec.cmd = vec![
            "sh".to_string(),
            "-c".to_string(),
            r#"trap "exit 143" TERM; trap 'exit 42' SIGINT; while true; do sleep 1; done"#
                .to_string(),
        ];
        let trapping = engine.create(&spec).await.expect("the mock creates");
        engine.start(&trapping).await.expect("the mock starts");

        engine
            .kill(&trapping, Signal::Sigint)
            .await
            .expect("the mock signals");
        assert_eq!(
            engine.state_of(&trapping),
            Some(ContainerState::Exited { code: 42 }),
            "the INT trap did not run"
        );
        assert_eq!(
            engine
                .wait(&trapping)
                .await
                .expect("the container has exited"),
            ExitStatus {
                code: 42,
                oom_killed: false
            }
        );

        // And a stop is the TERM the same command traps.
        let trapping = {
            let mut spec = spec.clone();
            spec.name = "mars-session-trapping-2".to_string();
            let id = engine.create(&spec).await.expect("the mock creates");
            engine.start(&id).await.expect("the mock starts");
            id
        };
        engine.stop(&trapping, 5).await.expect("the mock stops");
        assert_eq!(
            engine.state_of(&trapping),
            Some(ContainerState::Exited { code: 143 }),
            "a stop is the command's own TERM"
        );
    }

    /// A command that ignores `TERM` is stopped the way an engine stops it: it
    /// keeps running, and listed as running, for the whole grace period, and
    /// then ends on the hard kill's code.
    #[tokio::test(start_paused = true)]
    async fn a_stop_of_a_command_that_ignores_term_lasts_its_grace_period() {
        let engine = Arc::new(MockEngine::default());
        let mut spec = session_spec(SESSION_ID);
        spec.cmd = vec![
            "sh".to_string(),
            "-c".to_string(),
            r#"trap "" TERM; while true; do sleep 1; done"#.to_string(),
        ];
        let id = engine.create(&spec).await.expect("the mock creates");
        engine.start(&id).await.expect("the mock starts");

        // An ignored signal is recorded and nothing more.
        engine
            .kill(&id, Signal::Sigterm)
            .await
            .expect("the mock signals");
        assert_eq!(engine.state_of(&id), Some(ContainerState::Running));

        let stop = tokio::spawn({
            let engine = Arc::clone(&engine);
            let id = id.clone();
            async move { engine.stop(&id, 10).await }
        });
        tokio::time::sleep(Duration::from_secs(9)).await;
        assert!(
            !stop.is_finished(),
            "the stop ended inside its grace period"
        );
        let listed = engine
            .list_by_label(LABEL_SESSION_ID)
            .await
            .expect("the mock lists");
        assert!(
            listed.iter().all(|row| row.running),
            "a container in its grace period is running"
        );

        stop.await
            .expect("the stop task joins")
            .expect("the mock stops");
        assert_eq!(
            engine.state_of(&id),
            Some(ContainerState::Exited {
                code: KILL_EXIT_CODE
            })
        );
    }

    #[tokio::test]
    async fn a_name_is_taken_until_its_container_is_removed() {
        let engine = MockEngine::default();
        let spec = session_spec(SESSION_ID);

        let id = engine.create(&spec).await.expect("the mock creates");
        let error = engine
            .create(&spec)
            .await
            .expect_err("the name is already in use");
        assert!(
            matches!(error, EngineError::Conflict(_)),
            "unexpected: {error:?}"
        );

        engine.remove(&id, false).await.expect("the mock removes");
        let relaunched = engine
            .create(&spec)
            .await
            .expect("a relaunch reuses the name");
        assert_ne!(relaunched, id, "ids are monotonic, never reused");
        assert_eq!(engine.specs().len(), 2, "both creations are recorded");
    }

    #[tokio::test]
    async fn stdin_is_captured_across_every_attach_and_splits_into_lines() {
        let engine = MockEngine::default();
        let id = running(&engine, SESSION_ID).await;

        let mut stdin = engine.attach_stdin(&id).await.expect("the mock attaches");
        stdin
            .write_all(b"{\"type\":\"user\"}\n")
            .await
            .expect("the mock takes the bytes");

        // Recovery reattaches; the second writer appends to the first's bytes.
        let mut reattached = engine.attach_stdin(&id).await.expect("the mock reattaches");
        reattached
            .write_all(b"{\"type\":\"interrupt\"}\n")
            .await
            .expect("the mock takes the bytes");

        assert_eq!(
            engine.stdin_bytes(&id),
            b"{\"type\":\"user\"}\n{\"type\":\"interrupt\"}\n".to_vec()
        );
        assert_eq!(
            engine.stdin_lines(&id),
            vec![
                "{\"type\":\"user\"}".to_string(),
                "{\"type\":\"interrupt\"}".to_string(),
            ],
            "a trailing newline must not add an empty line"
        );

        let error = engine
            .attach_stdin(&ContainerId("mock-404".to_string()))
            .await
            .err()
            .expect("nothing to attach to");
        assert!(
            matches!(error, EngineError::NotFound(_)),
            "unexpected: {error:?}"
        );
    }

    /// A write after the container exited fails rather than being silently
    /// lost, and the bytes written before it are still there to read back.
    #[tokio::test]
    async fn a_write_after_the_container_exited_is_a_broken_pipe() {
        let engine = MockEngine::default();
        let id = running(&engine, SESSION_ID).await;

        let mut stdin = engine.attach_stdin(&id).await.expect("the mock attaches");
        stdin
            .write_all(b"{\"type\":\"user\"}\n")
            .await
            .expect("a running container takes the bytes");

        assert!(engine.exit(&id, 0));

        let error = stdin
            .write_all(b"{\"type\":\"interrupt\"}\n")
            .await
            .expect_err("a write after the container exited fails");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(
            engine.stdin_lines(&id),
            vec!["{\"type\":\"user\"}".to_string()],
            "the bytes written before the exit were lost, or the failed write was recorded"
        );
    }

    /// The same for a container that vanished under the writer, which is what a
    /// session owner writing to a reaped container sees.
    #[tokio::test]
    async fn a_write_to_a_vanished_container_is_a_broken_pipe() {
        let engine = MockEngine::default();
        let id = running(&engine, SESSION_ID).await;

        let mut stdin = engine.attach_stdin(&id).await.expect("the mock attaches");
        assert!(engine.vanish(&id));

        let error = stdin
            .write_all(b"{\"type\":\"user\"}\n")
            .await
            .expect_err("a write to a container that is gone fails");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[tokio::test]
    async fn networks_are_recorded_and_an_inspect_reports_both_of_a_containers() {
        let engine = MockEngine::default();

        engine
            .ensure_network("mars-sessions", true)
            .await
            .expect("the mock creates a network");
        engine
            .ensure_network("mars-egress", false)
            .await
            .expect("the mock creates a network");
        assert_eq!(
            engine.networks(),
            vec![
                ("mars-sessions".to_string(), true),
                ("mars-egress".to_string(), false),
            ]
        );

        let id = engine
            .create(&session_spec(SESSION_ID))
            .await
            .expect("the mock creates");
        engine
            .connect_network(&id, "mars-egress")
            .await
            .expect("the mock connects");

        assert_eq!(engine.connections(&id), vec!["mars-egress".to_string()]);
        let info = engine.inspect(&id).await.expect("the mock inspects");
        assert_eq!(
            info.networks,
            vec!["mars-sessions".to_string(), "mars-egress".to_string()],
            "the creation network first, then every connect"
        );
        assert_eq!(
            info.name,
            session_container_name(Uuid::parse_str(SESSION_ID).expect("a valid uuid"))
        );
        assert_eq!(info.pid, None, "a created container has no pid");

        engine.start(&id).await.expect("the mock starts");
        let info = engine.inspect(&id).await.expect("the mock inspects");
        assert!(info.pid.is_some(), "a running container reports a pid");
    }

    #[tokio::test]
    async fn list_by_label_returns_every_live_container_carrying_the_key() {
        let engine = MockEngine::default();

        let session = running(&engine, SESSION_ID).await;
        let other = engine
            .create(&session_spec("00000000-0000-4000-8000-00000000beef"))
            .await
            .expect("the mock creates");

        // A container with no session label at all: the probe's.
        let mut probe = session_spec("00000000-0000-4000-8000-00000000cafe");
        probe.name = "mars-probe".to_string();
        probe.labels = BTreeMap::from([("mars.probe".to_string(), "1".to_string())]);
        engine.create(&probe).await.expect("the mock creates");

        let listed = engine
            .list_by_label(LABEL_SESSION_ID)
            .await
            .expect("the mock lists");
        let ids: Vec<ContainerId> = listed.iter().map(|summary| summary.id.clone()).collect();
        assert_eq!(ids, vec![session.clone(), other]);
        assert!(listed[0].running, "the started one is running");
        assert!(!listed[1].running, "the created one is not");
        assert_eq!(
            engine
                .list_by_label("mars.probe")
                .await
                .expect("the mock lists")
                .len(),
            1,
            "the probe carries its own key and not the session one"
        );

        // Recovery's lookup: from a session id to the container it owns.
        let sid = Uuid::parse_str(SESSION_ID).expect("a valid uuid");
        assert_eq!(engine.container_id_for_session(sid), Some(session.clone()));

        engine.vanish(&session);
        assert_eq!(
            engine.container_id_for_session(sid),
            None,
            "a gone container is not found"
        );
        assert_eq!(
            engine
                .list_by_label(LABEL_SESSION_ID)
                .await
                .expect("the mock lists")
                .len(),
            1,
            "a gone container is not listed"
        );
    }

    #[tokio::test]
    async fn images_exist_unless_a_test_says_otherwise_and_a_pull_can_be_made_to_fail() {
        let engine = MockEngine::default();

        assert!(
            engine
                .image_exists("mars-session-claude:dev")
                .await
                .expect("the mock answers")
        );

        engine.set_missing_images(["mars-session-claude:dev"]);
        assert!(
            !engine
                .image_exists("mars-session-claude:dev")
                .await
                .expect("the mock answers")
        );

        engine.fail_next_pull("manifest unknown");
        let error = engine
            .pull_image("mars-session-claude:dev")
            .await
            .expect_err("the armed pull fails");
        match error {
            EngineError::ImagePull { image, message } => {
                assert_eq!(image, "mars-session-claude:dev");
                assert_eq!(message, "manifest unknown");
            }
            other => panic!("unexpected: {other:?}"),
        }
        assert!(
            engine.pulled_images().is_empty(),
            "a failed pull is not a pull"
        );

        engine
            .pull_image("mars-session-claude:dev")
            .await
            .expect("only the next pull was armed");
        assert_eq!(
            engine.pulled_images(),
            vec!["mars-session-claude:dev".to_string()]
        );
        assert!(
            engine
                .image_exists("mars-session-claude:dev")
                .await
                .expect("the mock answers"),
            "a pulled image exists"
        );
    }

    #[tokio::test]
    async fn an_exec_reads_its_script_then_echoes_and_records_its_resizes_and_close() {
        let engine = MockEngine::default();
        let id = running(&engine, SESSION_ID).await;

        engine.script_exec_output(&id, Bytes::from_static(b"agent@mars:~$ "));

        let cmd = vec!["/bin/bash".to_string(), "-l".to_string()];
        let mut exec = engine
            .exec_pty(&id, &cmd, "agent", 120, 40)
            .await
            .expect("the mock execs");

        assert_eq!(
            engine.exec_requests(),
            vec![ExecRequest {
                container: id.clone(),
                cmd,
                user: "agent".to_string(),
                cols: 120,
                rows: 40,
            }]
        );

        assert_eq!(
            exec.read().await.expect("the script is read first"),
            Some(Bytes::from_static(b"agent@mars:~$ "))
        );

        exec.write(b"ls\n").await.expect("the mock takes the write");
        assert_eq!(
            exec.read().await.expect("the write echoes back"),
            Some(Bytes::from_static(b"ls\n"))
        );

        // A drained PTY on a running container parks rather than reporting end
        // of stream: a shell at its prompt says nothing until it is typed at.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), exec.read())
                .await
                .is_err(),
            "a drained PTY on a running container must park",
        );

        exec.resize(100, 30).await.expect("the mock resizes");
        exec.resize(80, 24).await.expect("the mock resizes");
        assert_eq!(engine.exec_resizes(&id), vec![(100, 30), (80, 24)]);

        // The container ending is what a real exec's PTY sees as end of
        // stream, and is what the terminal reads as "the shell exited".
        engine.exit(&id, 0);
        assert_eq!(
            exec.read().await.expect("the container has ended"),
            None,
            "an exec in a container that is not running is at end of stream",
        );

        assert!(!engine.exec_closed(&id));
        assert_eq!(
            exec.close().await.expect("the mock closes"),
            EXEC_EXIT_CODE,
            "the exit code travels the path `terminal_closed` reads"
        );
        assert!(
            engine.exec_closed(&id),
            "the close is recorded for the handler's cleanup"
        );
    }

    #[tokio::test]
    async fn ping_is_counted_and_fails_only_when_the_test_says_so() {
        let engine = MockEngine::default();
        assert_eq!(engine.pings(), 0);

        engine.ping().await.expect("a healthy mock answers");
        assert_eq!(engine.pings(), 1);

        engine.set_unhealthy(true);
        let error = engine.ping().await.expect_err("an unhealthy mock does not");
        assert!(
            matches!(error, EngineError::Connection(_)),
            "unexpected: {error:?}"
        );
        assert_eq!(engine.pings(), 2, "a failed ping is still a ping");

        engine.set_unhealthy(false);
        engine.ping().await.expect("healthy again");
    }

    #[tokio::test]
    async fn the_mock_reports_its_kind_and_downcasts_out_of_a_trait_object() {
        assert_eq!(MockEngine::default().kind(), EngineKind::Podman);
        assert_eq!(
            MockEngine::new(EngineKind::Docker).kind(),
            EngineKind::Docker
        );

        let engine: Arc<dyn ContainerEngine> = Arc::new(MockEngine::default());
        engine.ping().await.expect("the mock answers");

        let mock = engine
            .as_any()
            .downcast_ref::<MockEngine>()
            .expect("the trait object is the mock");
        assert_eq!(mock.pings(), 1);
    }
}
