//! The two streaming halves of the bollard adapter: the stdin of an attached
//! container, and a running exec with a PTY.
//!
//! Both are implementation details of [`super::bollard`], which is the only
//! module that constructs them; callers see them as the plain
//! [`StdinWriter`](super::StdinWriter) and [`ExecSession`] trait objects the
//! engine trait hands back.
//!
//! The two carry very different things. The attachment carries *only* stdin:
//! the CLI's output is a file on the session volume that the owner tails, and
//! the socket is a pipe for input and a liveness signal, nothing more
//! (ADR 0010; `ARCHITECTURE.md`, "Durability and recovery"). The exec carries
//! a terminal's bytes in both directions, and nothing it carries is recorded
//! as an event (`SPEC.md`, "WebSocket: session stream").
//!
//! **Secrets.** Neither payload is ever logged. A user's message reaches the
//! CLI through the attachment and a user's keystrokes reach the shell through
//! the exec, so a log line here says `container`, a byte count and nothing
//! else, at `debug` (CLAUDE.md rule 3).

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
// Shadows the prelude's one-parameter `Result<T>` alias, exactly as the other
// engine modules do, so the signatures read as the trait declares them.
use std::result::Result;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use async_trait::async_trait;
use bollard::Docker;
use bollard::container::LogOutput;
use bollard::errors::Error as BollardError;
use bollard::exec::ResizeExecOptions;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

use super::{ContainerId, EngineError, ExecSession};
use crate::prelude::*;

/// The half of an attach or an exec the engine sends bytes on.
pub(super) type OutputStream = Pin<Box<dyn Stream<Item = Result<LogOutput, BollardError>> + Send>>;

/// The half the orchestrator writes bytes to. `Pin<Box<_>>` is itself
/// [`Unpin`], which is what lets [`BollardStdin`] satisfy
/// [`StdinWriter`](super::StdinWriter)'s blanket implementation.
pub(super) type InputSink = Pin<Box<dyn AsyncWrite + Send>>;

/// How long [`BollardExec::close`] waits for the PTY's output to end after the
/// input half has gone. A shell that ignores its EOF must not hold the
/// WebSocket handler's task open for longer than this.
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// How long [`resize_exec`] waits before its single retry.
const RESIZE_RETRY_DELAY: Duration = Duration::from_millis(50);

/// How many times `close` asks for an exit code while the engine still reports
/// the exec running.
///
/// Together with [`EXIT_CODE_DELAY`] a second, which is the race between the
/// PTY's last byte and the engine booking the exit, not a wait for a process
/// that is still working.
const EXIT_CODE_ATTEMPTS: u32 = 20;

/// How long `close` waits between two asks for an exit code.
const EXIT_CODE_DELAY: Duration = Duration::from_millis(50);

/// The exit code reported when the engine has none. `SPEC.md`, "WebSocket:
/// session stream" gives `terminal_closed` an `exit_code` either way.
const UNKNOWN_EXIT_CODE: i64 = -1;

/// The write half of a container's attached stdin.
///
/// It is an [`AsyncWrite`] that delegates to the hijacked connection's input
/// half, plus the task that drains the output half. The drain exists only to
/// keep the connection open: with `stdout` and `stderr` unattached the engine
/// sends almost nothing, but a reader that never reads is a connection the
/// engine may stop writing to, and the session owner ignores output entirely.
///
/// When the container exits the engine closes the connection, and the next
/// write answers with an [`io::Error`] rather than buffering forever. That is
/// the contract the session owner needs: a failed write is an input it records
/// as failed (`ARCHITECTURE.md`, "Session owner task").
pub(super) struct BollardStdin {
    /// Which container this is attached to, for the log lines below.
    container: ContainerId,
    /// The hijacked connection's input half.
    input: InputSink,
    /// The task draining the output half, aborted when this is dropped.
    drain: Option<JoinHandle<()>>,
    /// Set by the drain task when the engine's end of the connection has gone.
    ///
    /// The socket alone is not enough to tell: a rootless Podman has been seen
    /// to accept and discard writes to a container that has already exited, so
    /// a writer that only asked the socket would report every lost input as a
    /// success. The output half ending is the engine saying the attachment is
    /// over, and it is what turns the next write into a broken pipe.
    closed: Arc<AtomicBool>,
    /// How many bytes have gone through, for one `debug` line at the end.
    written: u64,
}

impl BollardStdin {
    /// Wrap both halves of a container attach, draining the output half in a
    /// task of its own.
    pub(super) fn attached(container: ContainerId, input: InputSink, output: OutputStream) -> Self {
        let closed = Arc::new(AtomicBool::new(false));
        let drain = drain_attach_output(container.clone(), output, Arc::clone(&closed));

        Self::new(container, input, Some(drain), closed)
    }

    /// The writer over an input half whose output half is somebody else's —
    /// the unit tests, which have no connection to drain.
    fn new(
        container: ContainerId,
        input: InputSink,
        drain: Option<JoinHandle<()>>,
        closed: Arc<AtomicBool>,
    ) -> Self {
        Self {
            container,
            input,
            drain,
            closed,
            written: 0,
        }
    }
}

impl AsyncWrite for BollardStdin {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();

        if this.closed.load(Ordering::SeqCst) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the container's attach stream has closed",
            )));
        }

        let written = ready!(this.input.as_mut().poll_write(cx, buf));

        if let Ok(bytes) = &written {
            // A partial write is legal and the caller loops; the count is the
            // only thing recorded, never the bytes themselves (rule 3).
            this.written += *bytes as u64;
        }

        Poll::Ready(written)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().input.as_mut().poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().input.as_mut().poll_shutdown(cx)
    }
}

impl Drop for BollardStdin {
    /// Ending the attachment ends its drain: the task holds the connection's
    /// read half, and nothing is left to read it for.
    fn drop(&mut self) {
        if let Some(drain) = self.drain.take() {
            drain.abort();
        }

        debug!(
            container = %self.container,
            bytes = self.written,
            "the stdin attachment closed"
        );
    }
}

/// Read the attach stream to its end, throw every byte away, and mark the
/// attachment closed when it ends.
///
/// Nothing is attached to the container's stdout or stderr, so there is
/// normally nothing here at all; what arrives is counted and dropped. The
/// stream ending is the container having gone, which is what `closed` tells
/// the writer, and it is how this task ends on its own; [`BollardStdin`]'s
/// [`Drop`] is how it ends early.
fn drain_attach_output(
    container: ContainerId,
    mut output: OutputStream,
    closed: Arc<AtomicBool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(item) = output.next().await {
            match item {
                Ok(chunk) => debug!(
                    container = %container,
                    bytes = chunk.into_bytes().len(),
                    "discarded bytes from the attach stream"
                ),
                Err(error) => {
                    debug!(
                        container = %container,
                        error = %error,
                        "the attach stream failed"
                    );
                    break;
                }
            }
        }

        closed.store(true, Ordering::SeqCst);
        debug!(container = %container, "the attach stream ended");
    })
}

/// A live exec with a PTY: the terminal view's end of the container.
///
/// `SPEC.md`, "WebSocket: session stream": [`read`](ExecSession::read) feeds
/// the binary `terminal` frames, [`write`](ExecSession::write) takes the
/// binary frames the client sends, [`resize`](ExecSession::resize) is
/// `terminal_resize`, and the code [`close`](ExecSession::close) answers with
/// is `terminal_closed`'s `exit_code`.
pub(super) struct BollardExec {
    /// The client the resize and the inspect go through. `Docker` is a cheap
    /// handle around a shared transport, so this clone opens no connection.
    docker: Docker,
    /// The engine's id for this exec.
    exec_id: String,
    /// The container it runs in, for the log lines.
    container: ContainerId,
    /// The PTY's input half.
    input: InputSink,
    /// The PTY's output half. With `tty: true` the engine multiplexes nothing,
    /// so this is raw terminal bytes.
    output: OutputStream,
}

impl BollardExec {
    /// Wrap a started exec.
    pub(super) fn new(
        docker: Docker,
        exec_id: String,
        container: ContainerId,
        input: InputSink,
        output: OutputStream,
    ) -> Self {
        Self {
            docker,
            exec_id,
            container,
            input,
            output,
        }
    }
}

#[async_trait]
impl ExecSession for BollardExec {
    async fn read(&mut self) -> Result<Option<Bytes>, EngineError> {
        next_chunk(&mut self.output).await
    }

    async fn write(&mut self, data: &[u8]) -> Result<(), EngineError> {
        // `write_all` and then `flush`: a terminal that holds a keystroke in a
        // buffer looks broken to the person typing.
        self.input.write_all(data).await?;
        self.input.flush().await?;

        debug!(
            container = %self.container,
            bytes = data.len(),
            "wrote to the terminal exec"
        );
        Ok(())
    }

    async fn resize(&mut self, cols: u16, rows: u16) -> Result<(), EngineError> {
        resize_exec(&self.docker, &self.exec_id, cols, rows).await
    }

    async fn close(self: Box<Self>) -> Result<i64, EngineError> {
        let Self {
            docker,
            exec_id,
            container,
            mut input,
            mut output,
        } = *self;

        // The half-close is the shell's EOF, which is what makes it exit. A
        // connection the engine has already torn down refuses it, and that is
        // the same end by another route.
        let _ = input.shutdown().await;
        drop(input);

        if timeout(OUTPUT_DRAIN_TIMEOUT, async {
            while output.next().await.is_some() {}
        })
        .await
        .is_err()
        {
            debug!(
                container = %container,
                "the terminal exec did not end within the drain timeout"
            );
        }
        drop(output);

        let code = exec_exit_code(&docker, &exec_id).await?;

        info!(
            container = %container,
            exit_code = code,
            "the terminal exec closed"
        );
        Ok(code)
    }
}

/// Resize an exec's PTY, retrying once.
///
/// Called on the exec that was just started and again for every
/// `terminal_resize`. Directly after `start_exec` the engine can answer before
/// the process has its PTY, which Docker reports as a 409 or a 500; one retry
/// covers that race and a second failure is the caller's to see.
///
/// Zero from a client is clamped to one: a client that reports a terminal with
/// no rows is describing a window that is not on screen yet, and an engine
/// asked for a zero-row PTY refuses.
pub(super) async fn resize_exec(
    docker: &Docker,
    exec_id: &str,
    cols: u16,
    rows: u16,
) -> Result<(), EngineError> {
    let options = ResizeExecOptions {
        height: rows.max(1),
        width: cols.max(1),
    };

    match docker.resize_exec(exec_id, options).await {
        Ok(()) => Ok(()),
        Err(first) => {
            debug!(error = %first, "the exec resize was refused; retrying once");
            sleep(RESIZE_RETRY_DELAY).await;
            Ok(docker.resize_exec(exec_id, options).await?)
        }
    }
}

/// The exit code of an exec whose output has ended.
///
/// The output ending and the engine booking the exit are two different
/// moments, so an inspect straight after the last byte can still report the
/// exec running with no code. This asks again for up to a second and then
/// answers with what there is: [`UNKNOWN_EXIT_CODE`] when the engine reports
/// no code at all.
async fn exec_exit_code(docker: &Docker, exec_id: &str) -> Result<i64, EngineError> {
    let mut code = None;

    for _ in 0..EXIT_CODE_ATTEMPTS {
        let response = docker.inspect_exec(exec_id).await?;
        code = response.exit_code;

        if response.running != Some(true) {
            break;
        }

        sleep(EXIT_CODE_DELAY).await;
    }

    Ok(code.unwrap_or(UNKNOWN_EXIT_CODE))
}

/// The next chunk of an output stream, as raw bytes.
///
/// Every [`LogOutput`] variant is unwrapped the same way. With a PTY the
/// engine multiplexes nothing and sends `Console`; the variant is the engine's
/// framing, not the terminal's, so concatenating the chunks of a read loop
/// reproduces exactly what the PTY wrote.
///
/// A connection that ends under the stream is the exec ending, not a failure:
/// the engine closes the hijacked connection when the process exits, and that
/// can arrive as an I/O error rather than as the end of the stream.
async fn next_chunk(output: &mut OutputStream) -> Result<Option<Bytes>, EngineError> {
    match output.next().await {
        Some(Ok(chunk)) => Ok(Some(chunk.into_bytes())),
        Some(Err(error)) if is_end_of_stream(&error) => Ok(None),
        Some(Err(error)) => Err(error.into()),
        None => Ok(None),
    }
}

/// Whether a stream failure is the far end having closed.
fn is_end_of_stream(error: &BollardError) -> bool {
    matches!(
        error,
        BollardError::IOError { err }
            if matches!(
                err.kind(),
                io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
            )
    )
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncReadExt;

    use super::*;

    fn container() -> ContainerId {
        ContainerId("mars-session-test".to_string())
    }

    /// A writer over an input half of the test's own, with no attach stream
    /// behind it and therefore nothing to drain.
    fn stdin_over(input: tokio::io::DuplexStream) -> BollardStdin {
        BollardStdin::new(
            container(),
            Box::pin(input),
            None,
            Arc::new(AtomicBool::new(false)),
        )
    }

    /// An output stream of exactly these items, as the engine would hand one
    /// back.
    fn stream_of(items: Vec<Result<LogOutput, BollardError>>) -> OutputStream {
        Box::pin(futures_util::stream::iter(items))
    }

    #[tokio::test]
    async fn a_stdin_writer_delegates_write_flush_and_shutdown_to_the_attached_half() {
        let (attached, mut engine_side) = tokio::io::duplex(64);
        let mut stdin = stdin_over(attached);

        stdin
            .write_all(b"{\"type\":\"user\"}\n")
            .await
            .expect("the half takes the bytes");
        stdin.flush().await.expect("the half flushes");

        let mut seen = vec![0_u8; 15];
        engine_side
            .read_exact(&mut seen)
            .await
            .expect("the bytes arrive on the other half");
        assert_eq!(seen, b"{\"type\":\"user\"}".to_vec());

        // The shutdown reaches the other half as an end of stream, which is
        // what tells a CLI reading stdin that there is no more input.
        stdin.shutdown().await.expect("the half shuts down");
        let mut rest = Vec::new();
        engine_side
            .read_to_end(&mut rest)
            .await
            .expect("the other half sees the end");
        assert_eq!(rest, b"\n".to_vec());
    }

    #[tokio::test]
    async fn a_stdin_writer_counts_the_bytes_it_wrote() {
        let (attached, mut engine_side) = tokio::io::duplex(64);
        let mut stdin = stdin_over(attached);

        stdin.write_all(b"one\n").await.expect("the half takes it");
        stdin.write_all(b"two\n").await.expect("the half takes it");

        let mut seen = vec![0_u8; 8];
        engine_side
            .read_exact(&mut seen)
            .await
            .expect("the bytes arrive");

        assert_eq!(stdin.written, 8);
    }

    /// The container exiting closes the connection under the writer. This is
    /// that shape in miniature: the far half is gone, and the write answers
    /// with an error instead of buffering forever.
    #[tokio::test]
    async fn a_write_to_a_closed_attachment_fails_instead_of_hanging() {
        let (attached, engine_side) = tokio::io::duplex(64);
        drop(engine_side);

        let mut stdin = stdin_over(attached);

        let error = stdin
            .write_all(b"{\"type\":\"user\"}\n")
            .await
            .expect_err("the connection is gone");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    /// The container exiting is the attach stream ending, and the drain task
    /// records it. A rootless Podman has been seen to accept writes to the
    /// socket long after that, so the flag — not the socket — is what makes
    /// the next input fail loudly enough for the owner to record it.
    #[tokio::test]
    async fn a_write_after_the_attach_stream_ended_is_a_broken_pipe() {
        let closed = Arc::new(AtomicBool::new(false));
        let drain = drain_attach_output(container(), stream_of(vec![]), Arc::clone(&closed));
        drain.await.expect("the drain ends with the empty stream");
        assert!(closed.load(Ordering::SeqCst), "the drain did not record it");

        // The far half is alive and would take the bytes; the attachment is
        // over regardless.
        let (attached, _engine_side) = tokio::io::duplex(64);
        let mut stdin = BollardStdin::new(container(), Box::pin(attached), None, closed);

        let error = stdin
            .write_all(b"{\"type\":\"user\"}\n")
            .await
            .expect_err("the attachment is over");
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[tokio::test]
    async fn dropping_the_writer_aborts_the_drain_task() {
        /// Set when the drain task's future is dropped, which an abort does.
        struct Guard(Arc<AtomicBool>);

        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let aborted = Arc::new(AtomicBool::new(false));
        let guard = Guard(Arc::clone(&aborted));
        let drain = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });

        let (attached, _engine_side) = tokio::io::duplex(64);
        let stdin = BollardStdin::new(
            container(),
            Box::pin(attached),
            Some(drain),
            Arc::new(AtomicBool::new(false)),
        );

        tokio::task::yield_now().await;
        assert!(!aborted.load(Ordering::SeqCst), "aborted too early");

        drop(stdin);
        tokio::task::yield_now().await;
        assert!(
            aborted.load(Ordering::SeqCst),
            "the drain task outlived the writer"
        );
    }

    #[tokio::test]
    async fn a_drain_task_reads_the_attach_stream_to_its_end() {
        let drain = drain_attach_output(
            container(),
            stream_of(vec![
                Ok(LogOutput::Console {
                    message: Bytes::from_static(b"ignored"),
                }),
                Err(BollardError::IOError {
                    err: io::Error::new(io::ErrorKind::UnexpectedEof, "the connection closed"),
                }),
            ]),
            Arc::new(AtomicBool::new(false)),
        );

        drain.await.expect("the drain ends with the stream");
    }

    #[tokio::test]
    async fn every_log_output_variant_reads_back_as_its_raw_bytes() {
        let mut output = stream_of(vec![
            Ok(LogOutput::Console {
                message: Bytes::from_static(b"40 100\r\n"),
            }),
            Ok(LogOutput::StdOut {
                message: Bytes::from_static(b"hello"),
            }),
            Ok(LogOutput::StdErr {
                message: Bytes::from_static(b" world"),
            }),
            Ok(LogOutput::StdIn {
                message: Bytes::from_static(b"!"),
            }),
        ]);

        let mut seen = Vec::new();
        while let Some(chunk) = next_chunk(&mut output).await.expect("the stream is fine") {
            seen.extend_from_slice(&chunk);
        }

        // No framing of the engine's own is left in the bytes: what the PTY
        // wrote is what the client's binary frames carry.
        assert_eq!(seen, b"40 100\r\nhello world!".to_vec());
        assert!(
            next_chunk(&mut output)
                .await
                .expect("an ended stream is not a failure")
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_closed_connection_ends_the_stream_and_anything_else_is_an_error() {
        for kind in [
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::BrokenPipe,
        ] {
            let mut output = stream_of(vec![Err(BollardError::IOError {
                err: io::Error::new(kind, "the connection closed"),
            })]);

            assert!(
                next_chunk(&mut output)
                    .await
                    .expect("a closed connection is the exec ending")
                    .is_none(),
                "unexpected for {kind:?}"
            );
        }

        let mut output = stream_of(vec![Err(BollardError::DockerResponseServerError {
            status_code: 409,
            message: "container is not running".to_string(),
        })]);
        let error = next_chunk(&mut output)
            .await
            .expect_err("the engine refused");
        assert!(
            matches!(error, EngineError::Conflict(_)),
            "unexpected: {error:?}"
        );
    }
}
