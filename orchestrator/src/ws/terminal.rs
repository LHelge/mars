//! The terminal attachment multiplexed onto the session socket (`SPEC.md`,
//! "WebSocket: session stream", the terminal; ADR 0025).
//!
//! "The terminal is an `exec` with a PTY into the session container running
//! `/bin/bash -l` as the `agent` user, multiplexed onto the same socket with
//! binary frames. It is an escape hatch for inspection; nothing it does is
//! recorded as events." Nothing here writes an event, and nothing here logs a
//! byte of PTY traffic: a terminal carries whatever the person pastes into it
//! (CLAUDE.md rule 3).
//!
//! [`Terminal`] knows nothing about WebSockets. It owns an
//! [`ExecSession`] and speaks to the rest of the world through two channels:
//! commands in, [`TerminalOutput`] out. That is what lets the same type be
//! driven against a real engine by the engine suite, and it is what keeps the
//! socket's `select!` loop free of any exec plumbing.
//!
//! **Why a task and not a mutex.** [`ExecSession`] is `Send` but not `Sync`,
//! and its four methods all take `&mut self`, so something has to own it. A
//! single `Mutex` would not do: a reader holding the lock across
//! `read().await` would block every keystroke and every resize for as long as
//! the shell had nothing to say, which for an idle prompt is for ever.
//! Instead one task owns the session and `select!`s between
//! [`ExecSession::read`] and the command channel, so a write never waits on a
//! read.
//!
//! That makes cancel-safety of `read` load-bearing: the command branch winning
//! drops a half-polled read. Both implementations qualify, and both say so
//! where they are written — `BollardExec::read` is `StreamExt::next` on the
//! attach stream, which buffers nothing in the future it hands out, and
//! `MockExecSession::read` keeps its bytes in the mock's own queues and parks
//! on a `Notify` it re-checks from scratch.

use std::time::Duration;

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::engine::{ContainerEngine, ContainerId, EngineError, ExecSession};
use crate::prelude::*;

/// What the terminal runs: the login shell `SPEC.md` names.
pub const TERMINAL_CMD: &[&str] = &["/bin/bash", "-l"];

/// Who it runs as: the numeric uid:gid of the session image's `agent` user
/// (`ARCHITECTURE.md`, "Session image", uid 1000), the same string the
/// container's own `User` carries.
///
/// Numeric rather than `agent` so the exec does not depend on the image having
/// that account by name: the engine suite runs this same command against a
/// plain test image, which has no `agent`.
pub const TERMINAL_USER: &str = "1000:1000";

/// The exit code reported when the engine gave none: a terminal that ended
/// without a code, a close that did not finish in time, an open that could not
/// be honoured (`ARCHITECTURE.md`, "Engine adapter").
pub const NO_EXIT_CODE: i64 = -1;

/// How long [`Terminal::close`] waits for the exec to end before giving up on
/// it and answering [`NO_EXIT_CODE`].
///
/// The socket is closing behind it: a shell that will not go must not hold the
/// connection's task, and the exec dies with its container in any case.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// How many commands may be queued for the exec's owner task.
///
/// Small: the producer is one socket's keystrokes and resizes, and a client
/// that outruns this is one whose PTY is not draining.
const COMMAND_BUFFER: usize = 32;

/// What the terminal sends towards the client.
///
/// The socket maps [`Data`](TerminalOutput::Data) to a binary frame and
/// [`Closed`](TerminalOutput::Closed) to `terminal_closed`; nothing else ever
/// comes out of a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalOutput {
    /// A chunk of PTY output, verbatim.
    Data(Bytes),
    /// The exec ended by itself — the shell exited, or the container stopped
    /// under it — with the code the engine reported.
    Closed {
        /// The shell's exit code, or [`NO_EXIT_CODE`] when the engine gave
        /// none.
        exit_code: i64,
    },
}

/// What the owner task takes.
enum Command {
    /// Bytes for the PTY, from a client binary frame.
    Write(Bytes),
    /// A new window size, already clamped.
    Resize { cols: u16, rows: u16 },
    /// End the exec and answer with its code. The `oneshot` is what makes
    /// [`Terminal::close`] able to return the code rather than wait for it to
    /// come round through [`TerminalOutput`].
    Close(oneshot::Sender<i64>),
}

/// What the owner task does next: carry on, or leave its loop this way.
enum Next {
    /// Nothing is settled; keep reading and serving commands.
    Continue,
    /// The exec ended on its own, or failed: the client is owed a
    /// `terminal_closed`.
    Ended,
    /// A [`Command::Close`] asked for it: the caller is owed the code and the
    /// client's `terminal_closed` is the socket's to send.
    Requested(oneshot::Sender<i64>),
    /// Nobody is listening any more — the [`Terminal`] was dropped, or the
    /// output channel closed. Clean up quietly.
    Detached,
}

/// One open terminal: a handle onto the task that owns the exec.
///
/// Dropping it without [`close`](Self::close) is safe and is itself a
/// disposal: the command channel closes, and the owner task ends the exec
/// before it exits.
pub struct Terminal {
    /// The owner task's inbox.
    commands: mpsc::Sender<Command>,
    /// The owner task, so a close that times out can stop it rather than leave
    /// it holding an exec.
    task: JoinHandle<()>,
}

impl Terminal {
    /// Start the PTY exec and the task that owns it.
    ///
    /// `cols` and `rows` are the client's, already clamped by the caller; the
    /// engine adapter clamps them again.
    pub async fn open(
        engine: &dyn ContainerEngine,
        container: &ContainerId,
        cmd: &[String],
        user: &str,
        cols: u16,
        rows: u16,
        out: mpsc::Sender<TerminalOutput>,
    ) -> std::result::Result<Terminal, EngineError> {
        let session = engine.exec_pty(container, cmd, user, cols, rows).await?;
        let (commands, inbox) = mpsc::channel(COMMAND_BUFFER);
        let task = tokio::spawn(own(session, inbox, out, container.clone()));

        Ok(Terminal { commands, task })
    }

    /// Send bytes to the PTY.
    ///
    /// The error is the terminal having ended under the caller, never the
    /// PTY's own refusal: a write the exec rejects ends the terminal, and the
    /// client hears about it as `terminal_closed`.
    pub async fn write(&self, data: &[u8]) -> std::result::Result<(), EngineError> {
        self.command(Command::Write(Bytes::copy_from_slice(data)))
            .await
    }

    /// Resize the PTY, in character cells.
    pub async fn resize(&self, cols: u16, rows: u16) -> std::result::Result<(), EngineError> {
        self.command(Command::Resize { cols, rows }).await
    }

    /// End the exec and answer with the shell's exit code.
    ///
    /// Bounded by [`CLOSE_TIMEOUT`] and [`NO_EXIT_CODE`] if it is not met: this
    /// is called on the socket's way out, where waiting for ever on a shell is
    /// the one thing that must not happen.
    pub async fn close(self) -> i64 {
        let (reply, answer) = oneshot::channel();

        let code = if self.commands.send(Command::Close(reply)).await.is_err() {
            // The owner task is already gone, which means it has already
            // closed the exec and reported whatever code it got.
            NO_EXIT_CODE
        } else {
            match tokio::time::timeout(CLOSE_TIMEOUT, answer).await {
                Ok(Ok(code)) => code,
                Ok(Err(_)) | Err(_) => NO_EXIT_CODE,
            }
        };

        // Either it has finished, in which case this is a no-op, or it has not
        // and nothing more is owed to it.
        self.task.abort();
        code
    }

    /// Queue one command, or report the terminal as gone.
    async fn command(&self, command: Command) -> std::result::Result<(), EngineError> {
        self.commands
            .send(command)
            .await
            .map_err(|_| EngineError::Conflict("the terminal exec has ended".to_string()))
    }
}

/// The task that owns the exec: read out, commands in, one close.
///
/// The `select!` yields a value rather than acting in its arms, so the borrow
/// of `session` the read future holds is over before a command handler takes
/// its own.
///
/// **Commands are served while a chunk is waiting for the socket, too.** A
/// shell that is producing faster than the client drains — `yes`, a runaway
/// `cat` — keeps the output channel full, and an owner parked in a plain
/// `send` would not be reading its inbox. The `close` behind it would then
/// wait out its whole timeout while the socket loop that should be draining
/// the channel is itself waiting on that close: a deadlock broken only by the
/// timeout, with the socket frozen for its duration and the exec never closed.
/// So the delivery is a `select!` of its own, over a
/// [`reserve`](mpsc::Sender::reserve) — cancel-safe, and it takes the slot
/// before the chunk is given up — and the command channel.
async fn own(
    mut session: Box<dyn ExecSession>,
    mut commands: mpsc::Receiver<Command>,
    out: mpsc::Sender<TerminalOutput>,
    container: ContainerId,
) {
    enum Step {
        Read(std::result::Result<Option<Bytes>, EngineError>),
        Command(Option<Command>),
    }

    let outcome = 'own: loop {
        let step = tokio::select! {
            chunk = session.read() => Step::Read(chunk),
            command = commands.recv() => Step::Command(command),
        };

        let next = match step {
            Step::Read(Ok(Some(bytes))) => {
                // The length only: the bytes are whatever is on the person's
                // screen, which may be anything they pasted (rule 3).
                debug!(container = %container, bytes = bytes.len(), "terminal output");

                // Held here until a slot is reserved for it, or until a
                // command settles what happens to it instead.
                let mut pending = Some(bytes);

                loop {
                    let Some(bytes) = pending.take() else {
                        break Next::Continue;
                    };

                    let command = tokio::select! {
                        permit = out.reserve() => match permit {
                            Ok(permit) => {
                                permit.send(TerminalOutput::Data(bytes));
                                None
                            }
                            // Nobody is reading the terminal's output any
                            // more, which is the socket having gone.
                            Err(_) => break 'own Next::Detached,
                        },
                        command = commands.recv() => {
                            // A write or a resize does not cost the chunk: it
                            // is put back and delivered after.
                            pending = Some(bytes);
                            Some(command)
                        }
                    };

                    let Some(command) = command else {
                        continue;
                    };

                    // A close discards whatever was pending: the client has
                    // said it is done with this terminal, and the socket is on
                    // its way out behind it.
                    match apply(&mut session, &container, command).await {
                        Next::Continue => {}
                        settled => break 'own settled,
                    }
                }
            }
            Step::Read(Ok(None)) => Next::Ended,
            Step::Read(Err(error)) => {
                debug!(container = %container, %error, "the terminal exec stopped reading");
                Next::Ended
            }
            Step::Command(command) => apply(&mut session, &container, command).await,
        };

        match next {
            Next::Continue => {}
            settled => break settled,
        }
    };

    // No command will be served again, so anybody who sends one now should
    // learn immediately rather than wait out a timeout on an owner that is
    // already disposing of the exec.
    drop(commands);

    let code = end(session, &container).await;

    match outcome {
        Next::Ended => {
            let _ = out.send(TerminalOutput::Closed { exit_code: code }).await;
        }
        Next::Requested(reply) => {
            let _ = reply.send(code);
        }
        // `Continue` never leaves the loop; it is here because the loop's
        // break value and one command's answer are the same type.
        Next::Detached | Next::Continue => {}
    }
}

/// Act on one command from the [`Terminal`], and say what it leaves the owner
/// to do.
async fn apply(
    session: &mut Box<dyn ExecSession>,
    container: &ContainerId,
    command: Option<Command>,
) -> Next {
    match command {
        Some(Command::Write(bytes)) => {
            let len = bytes.len();
            if let Err(error) = session.write(&bytes).await {
                debug!(container = %container, bytes = len, %error, "the terminal exec refused a write");
                return Next::Ended;
            }
            Next::Continue
        }
        Some(Command::Resize { cols, rows }) => {
            // A refused resize is cosmetic: the shell keeps running at the
            // size it had, and closing the terminal over it would be worse
            // than the wrapped line it costs.
            if let Err(error) = session.resize(cols, rows).await {
                debug!(container = %container, cols, rows, %error, "the terminal exec refused a resize");
            }
            Next::Continue
        }
        Some(Command::Close(reply)) => Next::Requested(reply),
        // The `Terminal` was dropped without a close: disposal all the same.
        None => Next::Detached,
    }
}

/// Close the exec, bounded, and answer with its code.
async fn end(session: Box<dyn ExecSession>, container: &ContainerId) -> i64 {
    match tokio::time::timeout(CLOSE_TIMEOUT, session.close()).await {
        Ok(Ok(code)) => code,
        Ok(Err(error)) => {
            debug!(container = %container, %error, "the terminal exec did not report an exit code");
            NO_EXIT_CODE
        }
        Err(_) => {
            debug!(container = %container, "the terminal exec did not close in time");
            NO_EXIT_CODE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use tokio::sync::Notify;

    use super::*;

    /// The code [`FakeExec`] reports when it is closed.
    const FAKE_EXIT_CODE: i64 = 7;

    /// A hand-written [`ExecSession`]: a queue of chunks to read out, a park
    /// once it is empty, and a record of everything it was asked.
    #[derive(Debug, Default)]
    struct FakeState {
        /// What [`ExecSession::read`] hands back, oldest first.
        output: std::collections::VecDeque<Bytes>,
        /// Whether the PTY is at end of stream once `output` is empty.
        ended: bool,
        /// Everything written to it, in order.
        written: Vec<Bytes>,
        /// Every resize, in order.
        resizes: Vec<(u16, u16)>,
        /// Whether it was closed.
        closed: bool,
    }

    /// A fake PTY shared between the test and the task that owns it.
    #[derive(Debug, Default)]
    struct Fake {
        state: std::sync::Mutex<FakeState>,
        wake: Notify,
    }

    impl Fake {
        fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
            self.state.lock().expect("the fake lock is healthy")
        }

        /// Queue a chunk for the reader and wake it.
        fn script(&self, bytes: &'static [u8]) {
            self.lock().output.push_back(Bytes::from_static(bytes));
            self.wake.notify_waiters();
        }

        /// Put the PTY at end of stream and wake the reader.
        fn end(&self) {
            self.lock().ended = true;
            self.wake.notify_waiters();
        }
    }

    /// The session half of a [`Fake`].
    struct FakeExec {
        fake: Arc<Fake>,
    }

    #[async_trait]
    impl ExecSession for FakeExec {
        async fn read(&mut self) -> std::result::Result<Option<Bytes>, EngineError> {
            loop {
                let notified = self.fake.wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();

                {
                    let mut state = self.fake.lock();
                    if let Some(chunk) = state.output.pop_front() {
                        return Ok(Some(chunk));
                    }
                    if state.ended {
                        return Ok(None);
                    }
                }

                notified.await;
            }
        }

        async fn write(&mut self, data: &[u8]) -> std::result::Result<(), EngineError> {
            self.fake.lock().written.push(Bytes::copy_from_slice(data));
            Ok(())
        }

        async fn resize(&mut self, cols: u16, rows: u16) -> std::result::Result<(), EngineError> {
            self.fake.lock().resizes.push((cols, rows));
            Ok(())
        }

        async fn close(self: Box<Self>) -> std::result::Result<i64, EngineError> {
            self.fake.lock().closed = true;
            Ok(FAKE_EXIT_CODE)
        }
    }

    /// A terminal over a fresh fake, with the receiving half of its output.
    fn terminal() -> (Terminal, Arc<Fake>, mpsc::Receiver<TerminalOutput>) {
        terminal_with_buffer(16)
    }

    /// The same, with the output channel's capacity chosen: a small one is how
    /// a client that is not draining is written.
    fn terminal_with_buffer(
        buffer: usize,
    ) -> (Terminal, Arc<Fake>, mpsc::Receiver<TerminalOutput>) {
        let fake = Arc::new(Fake::default());
        let (out, outputs) = mpsc::channel(buffer);
        let (commands, inbox) = mpsc::channel(COMMAND_BUFFER);

        let session: Box<dyn ExecSession> = Box::new(FakeExec {
            fake: Arc::clone(&fake),
        });
        let task = tokio::spawn(own(
            session,
            inbox,
            out,
            ContainerId("fake-container".to_string()),
        ));

        (Terminal { commands, task }, fake, outputs)
    }

    /// The next output, or a failure rather than a hang.
    async fn next(outputs: &mut mpsc::Receiver<TerminalOutput>) -> TerminalOutput {
        tokio::time::timeout(Duration::from_secs(2), outputs.recv())
            .await
            .expect("an output arrives within the window")
            .expect("the terminal has not detached")
    }

    #[tokio::test]
    async fn every_chunk_the_pty_produces_is_forwarded_verbatim() {
        let (terminal, fake, mut outputs) = terminal();

        fake.script(b"agent@mars:~$ ");
        fake.script(b"ls\r\n");

        assert_eq!(
            next(&mut outputs).await,
            TerminalOutput::Data(Bytes::from_static(b"agent@mars:~$ ")),
        );
        assert_eq!(
            next(&mut outputs).await,
            TerminalOutput::Data(Bytes::from_static(b"ls\r\n")),
        );

        assert_eq!(terminal.close().await, FAKE_EXIT_CODE);
    }

    #[tokio::test]
    async fn a_pty_at_end_of_stream_closes_the_terminal_with_its_code() {
        let (_terminal, fake, mut outputs) = terminal();

        fake.end();

        assert_eq!(
            next(&mut outputs).await,
            TerminalOutput::Closed {
                exit_code: FAKE_EXIT_CODE
            },
            "the reader closes the exec and reports its code",
        );
        assert!(fake.lock().closed, "the exec is closed, not merely dropped");
    }

    /// The whole reason the exec lives in a task rather than behind a mutex: a
    /// shell at its prompt produces nothing, and a keystroke must still get
    /// through.
    #[tokio::test]
    async fn a_write_is_delivered_while_a_read_is_pending_with_no_output() {
        let (terminal, fake, _outputs) = terminal();

        // Let the owner task reach its `read`, which parks: the fake has
        // nothing queued and is not at end of stream.
        tokio::task::yield_now().await;
        assert!(fake.lock().output.is_empty());

        tokio::time::timeout(Duration::from_secs(2), terminal.write(b"ls\n"))
            .await
            .expect("the write is taken within the window")
            .expect("the terminal is open");
        terminal
            .resize(100, 40)
            .await
            .expect("the terminal is still open");

        // The command is delivered asynchronously; the close behind it is
        // ordered after both, so the queue is settled by the time it answers.
        assert_eq!(terminal.close().await, FAKE_EXIT_CODE);

        let state = fake.lock();
        assert_eq!(state.written, vec![Bytes::from_static(b"ls\n")]);
        assert_eq!(state.resizes, vec![(100, 40)]);
    }

    /// A terminal whose output nobody is draining must still take commands:
    /// this is the case where the socket loop is itself inside
    /// [`Terminal::close`] and therefore not reading the output channel, so an
    /// owner parked in a plain `send` would deadlock until the timeout and
    /// leave the exec unclosed.
    #[tokio::test]
    async fn a_close_gets_through_while_a_chunk_waits_for_a_client_that_is_not_reading() {
        // One slot, and the receiving half is never read from: the first chunk
        // fills the channel and the second is left pending in the owner.
        let (terminal, fake, _outputs) = terminal_with_buffer(1);

        fake.script(b"yes\r\n");
        fake.script(b"yes\r\n");
        fake.script(b"yes\r\n");

        // A write still reaches the PTY with a chunk pending: the delivery
        // does not stop the owner serving its inbox.
        tokio::time::timeout(Duration::from_secs(1), terminal.write(b"q"))
            .await
            .expect("the write is taken while a chunk is pending")
            .expect("the terminal is open");

        let code = tokio::time::timeout(Duration::from_secs(1), terminal.close())
            .await
            .expect("the close is answered well inside the close timeout");

        assert_eq!(
            code, FAKE_EXIT_CODE,
            "the exec was closed properly, not abandoned on a timeout",
        );

        let state = fake.lock();
        assert!(state.closed, "the exec is closed, not merely dropped");
        assert_eq!(state.written, vec![Bytes::from_static(b"q")]);
    }

    #[tokio::test]
    async fn dropping_a_terminal_without_closing_it_still_ends_the_exec() {
        let (terminal, fake, _outputs) = terminal();

        drop(terminal);

        tokio::time::timeout(Duration::from_secs(2), async {
            while !fake.lock().closed {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the exec is closed once nobody holds the terminal");
    }

    #[tokio::test]
    async fn a_close_whose_owner_has_already_gone_answers_no_exit_code() {
        let (terminal, fake, mut outputs) = terminal();

        fake.end();
        assert_eq!(
            next(&mut outputs).await,
            TerminalOutput::Closed {
                exit_code: FAKE_EXIT_CODE
            },
        );

        // The owner reported its code through the output channel already; the
        // late close has nobody to ask.
        assert_eq!(terminal.close().await, NO_EXIT_CODE);
    }
}
