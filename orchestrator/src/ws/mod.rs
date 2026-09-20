//! The session WebSocket handler (`SPEC.md`, "WebSocket: session stream").
//!
//! `GET /ws/sessions/{id}?after=<seq>&token=<jwt>`, in two halves that must
//! not be confused:
//!
//! 1. **Before the upgrade** everything that can still be answered with an
//!    HTTP status is: the token (401 / 403), the session row (404) and the
//!    `after` cursor (400). A browser reading a status code is the only place
//!    those failures can be reported usefully, because once the socket is open
//!    the answer is a close frame nobody can act on.
//! 2. **After the upgrade** the read side of the stream, in the order
//!    `ARCHITECTURE.md`, "Event delivery" fixes: subscribe to the session's
//!    fan-out channel, *then* replay from the client's cursor, then follow the
//!    notices. Subscribing first is what makes the race benign — a row
//!    committed during the replay is either read by a later page or wakes the
//!    live loop afterwards — and the cursor advances per row, so no event is
//!    sent twice even before the client's own `seq` dedupe.
//!
//! Every outgoing frame goes through one `tokio::sync::mpsc` channel and a
//! writer task that owns the sink. The main loop is the only sender; a send
//! that fails or times out ends the writer, which closes the channel, which is
//! how the main loop learns to stop: after a failed write nothing further is
//! written.
//!
//! The input, stop and terminal messages are parsed here and handed to
//! [`handle_client_message`] and [`handle_binary`]. The terminal itself is
//! [`terminal::Terminal`], which owns the exec in a task of its own and talks
//! to this loop over a channel: its output is the loop's fifth `select!` arm,
//! and it is disposed of in [`run`], the one exit path every close goes
//! through. The loop itself — the `select!` over the socket, the fan-out
//! receiver, the safety read, the ping and the terminal — is settled here.
//!
//! Writing is authorized per message, not per socket: [`ensure_authorized`]
//! re-runs the database half of the stream contract before every application
//! message and at every ping tick, and a failure closes the socket with 1008
//! without touching the agent session (`SPEC.md`, "Authentication"; ADR 0025).

mod input;
pub mod protocol;
pub mod terminal;

use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::response::Response;
use axum::routing::get;
use futures_util::stream::{SplitStream, StreamExt};
use futures_util::{FutureExt, SinkExt};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::engine::{ContainerId, EngineError};
use crate::events::Notice;
use crate::models::SessionState;
use crate::prelude::*;
use crate::repositories::SessionRepository;
use crate::routes::{
    AUTH_REQUIRED, StreamPrincipal, StreamToken, authenticate_stream, reauthorize,
};
use crate::session::SessionService;
use crate::ws::protocol::{ClientMessage, ServerMessage};
use crate::ws::terminal::{NO_EXIT_CODE, TERMINAL_CMD, TERMINAL_USER, Terminal, TerminalOutput};

/// What a socket for a session that does not exist is answered with (404).
const SESSION_NOT_FOUND: &str = "session not found";

/// What an `after` that is not a sequence number is answered with (400).
///
/// One message for a negative number and for something that is not a number at
/// all: neither is a cursor, and which mistake the caller made is not worth a
/// second string.
const BAD_AFTER: &str = "after must be >= 0";

/// What a text frame that is not a [`ClientMessage`] is answered with, before
/// the close.
const MALFORMED: &str = "malformed message";

/// What the client is told when the stream cannot continue for a reason that
/// is this orchestrator's fault.
const INTERNAL: &str = "internal error";

/// The ping payload, so a pong can be recognised in a capture. Not a secret
/// and not a cursor: anything in it would be logged by a proxy.
const PING_PAYLOAD: &[u8] = b"mars";

/// How many events one replay or cursor read carries.
///
/// The documented page ceiling (`repositories::MAX_EVENT_PAGE`), because the
/// caller here is a stream that keeps reading until a page comes back short:
/// larger pages mean fewer round trips, and the bound only exists so that a
/// client thousands of events behind does not have the whole run buffered in
/// memory before the first frame is written.
const PAGE: u32 = 500;

/// How long one frame is given to reach the client before the stream is
/// considered dead.
///
/// A slow client must not pin the fan-out receiver and the task behind it for
/// ever; ten seconds is far beyond any real write and well inside the two ping
/// intervals that would close the socket anyway.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a socket that has sent a close frame waits for the client's echo
/// before the connection is dropped.
///
/// Short: it costs one idle task and it is only ever spent on a peer that has
/// stopped reading. See the linger in [`run`].
const CLOSE_LINGER: Duration = Duration::from_secs(5);

/// How many frames may be queued for the writer before a sender waits.
///
/// Generous, because a replay fills it in a burst; a client that cannot drain
/// it is handled by [`SEND_TIMEOUT`], not by the buffer.
const OUTGOING_BUFFER: usize = 64;

/// How many terminal outputs may be queued before the exec's owner task
/// waits.
///
/// The consumer is the socket's own loop, which does nothing but hand each
/// chunk to the writer; the buffer exists so a burst of PTY output — a `cat`
/// of a long file — does not make the owner task ping-pong with the loop for
/// every chunk.
const TERMINAL_BUFFER: usize = 32;

/// How many unanswered pings close the socket (`SPEC.md`, "WebSocket: session
/// stream": "closes after two missed pongs").
const MAX_MISSED_PONGS: u8 = 2;

/// The WebSocket routes, merged at the *root* of the application router.
///
/// Not under `/api`: `SPEC.md` spells the path `/ws/sessions/{id}` and nginx
/// proxies `location /ws/` (`ARCHITECTURE.md`, "Frontend architecture").
pub fn routes() -> Router<AppState> {
    Router::new().route("/ws/sessions/{id}", get(session_socket))
}

/// `?after=` alone. The token is [`StreamToken`]'s, from the same query
/// string, which is why neither struct denies unknown fields.
///
/// A `String` rather than an `i64`, for the reason `routes::sessions` keeps
/// `?state=` a string: a typed field would answer serde's own message about an
/// invalid digit, and the documented answer is [`BAD_AFTER`].
#[derive(Debug, Deserialize)]
struct AfterQuery {
    after: Option<String>,
}

impl AfterQuery {
    /// The cursor to replay from: `0` when the client named none, or 400.
    fn cursor(&self) -> Result<i64> {
        let Some(raw) = self.after.as_deref() else {
            return Ok(0);
        };

        match raw.trim().parse::<i64>() {
            Ok(after) if after >= 0 => Ok(after),
            _ => Err(Error::BadRequest(BAD_AFTER.to_string())),
        }
    }
}

/// `GET /ws/sessions/{id}?after=&token=` (401, 403, 404, 400, then upgrade).
///
/// Every check that has an HTTP answer happens here, in the documented order,
/// and the upgrade is the last thing the handler does.
async fn session_socket(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<AfterQuery>,
    token: StreamToken,
    upgrade: WebSocketUpgrade,
) -> Result<Response> {
    let user = authenticate_stream(&state, &token.0).await?;

    // The row, before the cursor: a client watching a session that has been
    // deleted gets the 404 it can act on, whatever it asked to replay from.
    SessionRepository::new(&state.pool)
        .get(id)
        .await
        .map_err(|error| match error {
            Error::NotFound => Error::Missing(SESSION_NOT_FOUND.to_string()),
            other => other,
        })?;

    let after = query.cursor()?;
    let principal = StreamPrincipal::new(&user);

    debug!(session_id = %id, user_id = %user.id, "session websocket opening");

    Ok(upgrade.on_upgrade(move |socket| run(state, id, principal, after, socket)))
}

/// One open socket's state.
///
/// `cursor` is the contract with the client: every event already written to
/// the socket has `seq <= cursor`, so every read is `seq > cursor` and no row
/// can be sent twice or skipped.
struct SocketContext {
    state: AppState,
    session_id: Uuid,
    /// The lifecycle rules for everything this socket asks of the session, so
    /// `input` and `stop` are the same call the REST routes make. Built once
    /// per socket: it is an [`AppState`] clone and nothing more.
    service: SessionService,
    /// Who the socket was opened as, re-checked by [`ensure_authorized`].
    principal: StreamPrincipal,
    cursor: i64,
    missed_pongs: u8,
    /// The terminal exec, while one is open (`SPEC.md`, "WebSocket: session
    /// stream", the terminal). `None` is the socket with no terminal, which is
    /// every socket until a `terminal_open` and every socket again after a
    /// `terminal_closed`.
    terminal: Option<Terminal>,
    /// The sending half of the terminal's output channel, kept for the whole
    /// life of the socket.
    ///
    /// Held even with no terminal open, for two reasons: a `terminal_open`
    /// needs one to hand the new attachment, and a channel whose last sender
    /// were dropped would make the loop's terminal arm resolve immediately,
    /// for ever.
    terminal_out: mpsc::Sender<TerminalOutput>,
}

/// Why the socket task stopped.
///
/// Every arm of the loop returns one of these rather than writing a close
/// frame itself, so there is exactly one place that closes the socket.
enum End {
    /// The peer went away, or a write failed: nothing more can be written.
    Gone,
    /// Close cleanly with this code, after an `error` frame carrying
    /// `message` if one is given, and with `reason` in the close frame itself.
    ///
    /// The text rather than a [`ServerMessage`]: the only message that ever
    /// precedes a close is `error`, and a whole `ServerMessage` here would put
    /// a `Session` in every `Err` this module returns.
    ///
    /// `reason` is empty for every close but the revoked one: a close frame's
    /// reason is not shown to anybody and the `error` message is where a
    /// client reads what happened, but `SPEC.md`, "Authentication" spells the
    /// revocation close out as 1008 `authentication required`, and a client
    /// that only sees the close event is exactly the one this is for.
    Close {
        code: u16,
        message: Option<String>,
        reason: &'static str,
    },
}

impl End {
    /// An `error` frame followed by a close with `code`.
    fn error(code: u16, message: &str) -> Self {
        End::Close {
            code,
            message: Some(message.to_string()),
            reason: "",
        }
    }

    /// The documented close for a socket whose user may no longer be there:
    /// `error { authentication required }` and 1008 (`SPEC.md`,
    /// "Authentication").
    fn unauthenticated() -> Self {
        End::Close {
            code: close_code::POLICY,
            message: Some(AUTH_REQUIRED.to_string()),
            reason: AUTH_REQUIRED,
        }
    }
}

/// The socket task: subscribe, snapshot, replay, then follow.
///
/// `rx` is subscribed *before* the replay and dropped when this returns, which
/// is what lets the fan-out forget a session nobody is watching.
async fn run(
    state: AppState,
    session_id: Uuid,
    principal: StreamPrincipal,
    after: i64,
    socket: WebSocket,
) {
    let mut rx = state.fanout.subscribe_session(session_id);

    let service = SessionService::new(&state);
    let (terminal_out, mut terminal_rx) = mpsc::channel(TERMINAL_BUFFER);
    let mut context = SocketContext {
        state,
        session_id,
        service,
        principal,
        cursor: after,
        missed_pongs: 0,
        terminal: None,
        terminal_out,
    };

    let (sink, mut incoming) = socket.split();
    let (out, writer) = spawn_writer(sink);

    let end = stream(&mut context, &mut rx, &out, &mut incoming, &mut terminal_rx).await;
    let closing = matches!(end, End::Close { .. });

    // The one disposal, on every exit path there is: a revoked login, a client
    // that went away, a failed write (`SPEC.md`, "Authentication": "Dispose of
    // that socket's terminal attachment"). Before the close frame, so the exec
    // is gone by the time the client learns the socket is.
    if let Some(terminal) = context.terminal.take() {
        let exit_code = terminal.close().await;
        debug!(session_id = %session_id, exit_code, "the socket's terminal was disposed of");
    }

    if let End::Close {
        code,
        message,
        reason,
    } = end
    {
        if let Some(message) = message {
            let _ = send(&out, ServerMessage::Error { message }).await;
        }
        let _ = out
            .send(Message::Close(Some(CloseFrame {
                code,
                reason: reason.into(),
            })))
            .await;
    }

    // Dropping the sender is what ends the writer task; dropping `rx` is what
    // lets the fan-out drop the channel for a session nobody watches any more.
    drop(out);
    drop(rx);
    let _ = writer.await;

    if closing {
        // RFC 6455: having sent a close frame, wait for the peer's echo before
        // letting the connection go. Dropping it straight away resets the TCP
        // connection, and a client that is not reading at that instant — the
        // very client two missed pongs are about — would find its buffered
        // frames gone instead of the close it was sent.
        let _ = tokio::time::timeout(CLOSE_LINGER, async {
            while let Some(Ok(frame)) = incoming.next().await {
                if matches!(frame, Message::Close(_)) {
                    break;
                }
            }
        })
        .await;
    }

    debug!(session_id = %session_id, "session websocket closed");
}

/// The whole read side: the initial snapshot, the replay, the live loop.
async fn stream(
    context: &mut SocketContext,
    rx: &mut tokio::sync::broadcast::Receiver<Notice>,
    out: &mpsc::Sender<Message>,
    incoming: &mut SplitStream<WebSocket>,
    terminal_rx: &mut mpsc::Receiver<TerminalOutput>,
) -> End {
    // One `session` before anything else, so the client has the current state
    // without a REST round trip (`SPEC.md`, "WebSocket: session stream").
    if let Err(end) = send_session(context, out).await {
        return end;
    }
    if let Err(end) = drain(context, out).await {
        return end;
    }

    let timings = context.state.realtime_timings;
    let mut safety = tokio::time::interval(timings.safety_read);
    let mut ping = tokio::time::interval(timings.ping);
    // Neither timer catches up on the ticks a slow turn of this loop swallowed.
    // A tick here is not an event to deliver but a period to observe, and the
    // default burst would spend the whole pong budget in one instant: two
    // pings fired back to back reach [`MAX_MISSED_PONGS`] before the client
    // has been given a chance to answer either, and close a socket that never
    // missed a pong. `SPEC.md`, "WebSocket: session stream" says two *missed*
    // pongs, which is two ping periods with no answer in between. The safety
    // read is the same argument with nothing at stake: replaying the reads a
    // stall swallowed would only re-read from a cursor that has not moved.
    safety.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Both fire immediately on their first tick; the stream has just read from
    // its cursor and has nothing to ping about yet.
    safety.tick().await;
    ping.tick().await;

    loop {
        // Which arm fired, rather than what to do about it: the ping arm needs
        // `incoming`, which another arm of the same `select!` already borrows,
        // so the handling happens after it — the move `sse::StreamContext`
        // makes for the same reason.
        let woke = tokio::select! {
            notice = rx.recv() => Woke::Notice(notice),
            _ = safety.tick() => Woke::Safety,
            _ = ping.tick() => Woke::Ping,
            frame = incoming.next() => Woke::Frame(frame),
            output = terminal_rx.recv() => Woke::Terminal(output),
        };

        let step = match woke {
            Woke::Notice(notice) => on_notice(context, out, notice).await,
            Woke::Safety => drain(context, out).await,
            Woke::Ping => on_ping(context, out, incoming).await,
            Woke::Frame(frame) => on_frame(context, out, frame).await,
            Woke::Terminal(output) => on_terminal_output(context, out, output).await,
        };

        if let Err(end) = step {
            return end;
        }
    }
}

/// What woke the live loop.
enum Woke {
    /// The fan-out published, lagged or closed.
    Notice(std::result::Result<Notice, RecvError>),
    /// The safety-read period elapsed.
    Safety,
    /// The ping period elapsed.
    Ping,
    /// The client sent a frame, or the socket ended.
    Frame(Option<std::result::Result<Message, axum::Error>>),
    /// The terminal produced output, or ended.
    Terminal(Option<TerminalOutput>),
}

/// One notice from the fan-out.
///
/// `Lagged` is treated exactly like a notice — the notices it dropped can only
/// have said "read from your cursor", which is what this does (ADR 0005) — and
/// never closes the socket.
async fn on_notice(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
    notice: std::result::Result<Notice, RecvError>,
) -> std::result::Result<(), End> {
    match notice {
        Ok(Notice::SessionEvents { .. }) | Ok(Notice::Resync) | Err(RecvError::Lagged(_)) => {
            drain(context, out).await
        }
        Ok(Notice::SessionState { .. }) => send_session(context, out).await,
        // Another aggregate's stream; a session socket has nothing to do with
        // it.
        Ok(Notice::TaskEvents { .. }) => Ok(()),
        // The fan-out has gone away, which only happens at shutdown.
        Err(RecvError::Closed) => Err(End::Gone),
    }
}

/// One ping tick: take what the client already sent, re-authorize, ping, and
/// count the pong that has not come.
///
/// The count is raised *after* the ping is sent, so it names the pings still
/// outstanding: reaching [`MAX_MISSED_PONGS`] means two ticks passed with no
/// pong in between, which is the documented close.
///
/// The frames that are already there are read first, and that is not a
/// nicety. This loop's other arms wait on the database — the safety read, the
/// re-authorization below — and while one of them is waiting no frame is
/// taken from the socket, so a pong the client sent promptly can still be
/// unread when this tick comes round. Counting it as missed would close a
/// socket whose client answered every ping, for no reason but that the
/// *server* was slow, which is the opposite of what `SPEC.md`, "WebSocket:
/// session stream" describes. `next()` on this stream is cancel-safe, so
/// taking only the frames that are ready loses nothing.
async fn on_ping(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
    incoming: &mut SplitStream<WebSocket>,
) -> std::result::Result<(), End> {
    while let Some(frame) = incoming.next().now_or_never() {
        on_frame(context, out, frame).await?;
    }

    ensure_authorized(context).await?;

    out.send(Message::Ping(PING_PAYLOAD.into()))
        .await
        .map_err(|_| End::Gone)?;

    context.missed_pongs = context.missed_pongs.saturating_add(1);
    if context.missed_pongs >= MAX_MISSED_PONGS {
        debug!(session_id = %context.session_id, "session websocket closed after two missed pongs");
        return Err(End::Close {
            code: close_code::AWAY,
            message: None,
            reason: "",
        });
    }

    Ok(())
}

/// One frame from the client.
async fn on_frame(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
    frame: Option<std::result::Result<Message, axum::Error>>,
) -> std::result::Result<(), End> {
    // Never the frame itself: a text frame is user content and a binary one is
    // terminal traffic (rule 3, ADR 0027).
    match frame {
        Some(Ok(Message::Pong(_))) => {
            context.missed_pongs = 0;
            Ok(())
        }
        Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientMessage>(&text) {
            Ok(message) => handle_client_message(context, out, message).await,
            Err(_) => {
                debug!(session_id = %context.session_id, "session websocket sent a malformed message");
                Err(End::error(close_code::UNSUPPORTED, MALFORMED))
            }
        },
        Some(Ok(Message::Binary(bytes))) => handle_binary(context, out, bytes).await,
        // A ping from the client is answered by axum itself.
        Some(Ok(Message::Ping(_))) => Ok(()),
        Some(Ok(Message::Close(_))) | None => Err(End::Gone),
        Some(Err(error)) => {
            debug!(session_id = %context.session_id, %error, "session websocket read failed");
            Err(End::Gone)
        }
    }
}

/// Read from the cursor until a page comes back short, sending each event.
///
/// The one read in this module: the replay and every live, safety and lag
/// read are the same call, which is why a notice can never mean anything the
/// safety read would not also recover.
async fn drain(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
) -> std::result::Result<(), End> {
    loop {
        let page = SessionRepository::new(&context.state.pool)
            .events_after_page(context.session_id, context.cursor, PAGE)
            .await
            .map_err(|error| {
                error!(session_id = %context.session_id, %error, "session websocket could not read events");
                End::error(close_code::ERROR, INTERNAL)
            })?;

        let short = page.len() < PAGE as usize;

        for event in page {
            let seq = event.seq;
            send(out, ServerMessage::Event { event }).await?;
            context.cursor = seq;
        }

        if short {
            return Ok(());
        }
    }
}

/// Send the session row as it is now.
///
/// A row that has disappeared mid-stream is the documented `error`
/// `session not found` and a normal close: the session was deleted, which is
/// not an error the client should retry through.
async fn send_session(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
) -> std::result::Result<(), End> {
    let session = SessionRepository::new(&context.state.pool)
        .get(context.session_id)
        .await
        .map_err(|error| match error {
            Error::NotFound => End::error(close_code::NORMAL, SESSION_NOT_FOUND),
            error => {
                error!(session_id = %context.session_id, %error, "session websocket could not read the session");
                End::error(close_code::ERROR, INTERNAL)
            }
        })?;

    send(out, ServerMessage::Session { session }).await
}

/// Queue one JSON frame for the writer task.
///
/// A closed channel means the writer has stopped, which it does on the first
/// failed or timed-out write: after that nothing more is written to the
/// socket.
async fn send(out: &mpsc::Sender<Message>, message: ServerMessage) -> std::result::Result<(), End> {
    let text = serde_json::to_string(&message).map_err(|error| {
        error!(%error, "a server message did not serialise");
        End::Gone
    })?;

    out.send(Message::Text(text.into()))
        .await
        .map_err(|_| End::Gone)
}

/// The one owner of the socket's sink.
///
/// Returns the sender every writer holds and the task's handle. The task ends
/// when the last sender is dropped, or on the first write that fails or
/// exceeds [`SEND_TIMEOUT`] — a slow client must not hold the session's
/// fan-out subscription open indefinitely.
fn spawn_writer(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
) -> (mpsc::Sender<Message>, JoinHandle<()>) {
    let (out, mut queue) = mpsc::channel::<Message>(OUTGOING_BUFFER);

    let writer = tokio::spawn(async move {
        while let Some(message) = queue.recv().await {
            match tokio::time::timeout(SEND_TIMEOUT, sink.send(message)).await {
                Ok(Ok(())) => {}
                // Either the socket is gone or the client is not reading. In
                // both cases the channel closes with this task and every
                // sender learns on its next send.
                Ok(Err(_)) | Err(_) => break,
            }
        }
    });

    (out, writer)
}

/// The per-message and per-tick half of the stream authentication contract
/// (`SPEC.md`, "Authentication"; `routes::stream_auth`).
///
/// One unlocked read of the user row — no transaction, no lock — before every
/// application message and at every ping tick. Both failures close: a database
/// that cannot be read is [`StreamAuthFailure::Unavailable`](crate::routes::StreamAuthFailure::Unavailable)
/// and "database failures must not authorize input".
///
/// Closing is *all* it does. The agent session is untouched — no stop, no
/// state change — because the person whose login was revoked is not the
/// session, and a revoked browser tab must not end somebody's run.
async fn ensure_authorized(context: &SocketContext) -> std::result::Result<(), End> {
    match reauthorize(&context.state, &context.principal).await {
        Ok(()) => Ok(()),
        Err(failure) => {
            debug!(
                session_id = %context.session_id,
                user_id = %context.principal.user_id,
                ?failure,
                "session websocket closed by re-authorization",
            );
            Err(End::unauthenticated())
        }
    }
}

/// Act on a parsed client message.
///
/// Re-authorization first, for every message alike: the specification puts it
/// "before every incoming WebSocket application message", and doing it here
/// rather than in each handler is what makes that true of a message added
/// later as well.
///
/// `input` and `stop` are [`input`]'s, which takes them through
/// [`SessionService`]. The terminal messages are still the terminal task's
/// seam: a well-formed one is accepted and ignored, which is deliberately not
/// the same as refusing it.
async fn handle_client_message(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
    message: ClientMessage,
) -> std::result::Result<(), End> {
    ensure_authorized(context).await?;

    match message {
        ClientMessage::Input {
            client_id,
            input: sent,
        } => input::handle_input(context, out, client_id, sent).await,
        ClientMessage::Stop => input::handle_stop(context).await,
        ClientMessage::TerminalOpen { cols, rows } => {
            handle_terminal_open(context, out, cols, rows).await
        }
        ClientMessage::TerminalResize { cols, rows } => {
            handle_terminal_resize(context, cols, rows).await
        }
        ClientMessage::TerminalClose => handle_terminal_close(context, out).await,
    }
}

/// `terminal_open { cols, rows }` (`SPEC.md`, "WebSocket: session stream":
/// "session must be `running`").
///
/// A second `terminal_open` on a socket that already has one is ignored rather
/// than refused: the protocol has one terminal per socket, and a client that
/// asks twice is a client that has lost track, not one to close the stream on.
///
/// Everything that stops the terminal being opened answers `terminal_closed
/// { exit_code: -1 }` and leaves the socket up. `error` is not available for
/// it: `error` is followed by a close, and a terminal that could not open is
/// not a reason to take the transcript down with it.
async fn handle_terminal_open(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
    cols: u16,
    rows: u16,
) -> std::result::Result<(), End> {
    if context.terminal.is_some() {
        debug!(session_id = %context.session_id, "a second terminal_open on a socket that already has one");
        return Ok(());
    }

    // The row as it is now, not the snapshot this socket opened with: the
    // session may have parked, ended or been relaunched since.
    let session = SessionRepository::new(&context.state.pool)
        .get(context.session_id)
        .await
        .map_err(|error| match error {
            Error::NotFound => End::error(close_code::NORMAL, SESSION_NOT_FOUND),
            error => {
                error!(session_id = %context.session_id, %error, "a terminal could not read its session");
                End::error(close_code::ERROR, INTERNAL)
            }
        })?;

    let container = match (session.state, session.container_id) {
        (SessionState::Running, Some(container_id)) => ContainerId(container_id),
        (state, _) => {
            debug!(
                session_id = %context.session_id,
                ?state,
                "a terminal was asked for on a session with no running container",
            );
            return terminal_closed(out, NO_EXIT_CODE).await;
        }
    };

    // Clamped here as well as in the adapter: a zero the engine would refuse
    // never reaches it, and what the engine records is what a test can read.
    let cmd: Vec<String> = TERMINAL_CMD
        .iter()
        .map(|part| (*part).to_string())
        .collect();

    match Terminal::open(
        context.state.engine.as_ref(),
        &container,
        &cmd,
        TERMINAL_USER,
        cols.max(1),
        rows.max(1),
        context.terminal_out.clone(),
    )
    .await
    {
        Ok(terminal) => {
            debug!(session_id = %context.session_id, container = %container, "a terminal opened");
            context.terminal = Some(terminal);
            Ok(())
        }
        // The container stopped between the row and the exec, which is the
        // same answer as a session that was not running when it was read.
        Err(error @ (EngineError::Conflict(_) | EngineError::NotFound(_))) => {
            debug!(session_id = %context.session_id, %error, "a terminal found no running container");
            terminal_closed(out, NO_EXIT_CODE).await
        }
        Err(error) => {
            error!(session_id = %context.session_id, %error, "a terminal exec could not be started");
            terminal_closed(out, NO_EXIT_CODE).await
        }
    }
}

/// `terminal_resize { cols, rows }`.
///
/// Nothing is sent either way: a resize that the engine refuses leaves the
/// shell at the size it had, which is a wrapped line and not a reason to take
/// the terminal down.
async fn handle_terminal_resize(
    context: &mut SocketContext,
    cols: u16,
    rows: u16,
) -> std::result::Result<(), End> {
    let Some(terminal) = context.terminal.as_ref() else {
        debug!(session_id = %context.session_id, "a terminal_resize with no terminal open");
        return Ok(());
    };

    if let Err(error) = terminal.resize(cols.max(1), rows.max(1)).await {
        debug!(session_id = %context.session_id, %error, "a terminal_resize was not applied");
    }

    Ok(())
}

/// `terminal_close {}`: the exec ends, the socket stays, and a later
/// `terminal_open` starts a fresh shell.
async fn handle_terminal_close(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
) -> std::result::Result<(), End> {
    let Some(terminal) = context.terminal.take() else {
        debug!(session_id = %context.session_id, "a terminal_close with no terminal open");
        return Ok(());
    };

    let exit_code = terminal.close().await;
    terminal_closed(out, exit_code).await
}

/// One chunk, or one end, from the open terminal.
///
/// Anything that arrives for a terminal this socket no longer has is dropped:
/// the exec's owner task and this loop are two tasks, so a chunk written just
/// before a `terminal_close` can still be in the channel afterwards, and the
/// client has already been told that terminal ended.
async fn on_terminal_output(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
    output: Option<TerminalOutput>,
) -> std::result::Result<(), End> {
    // Unreachable: the context holds a sender for as long as the socket lives.
    let Some(output) = output else {
        return Err(End::Gone);
    };

    if context.terminal.is_none() {
        return Ok(());
    }

    match output {
        TerminalOutput::Data(bytes) => out
            .send(Message::Binary(bytes))
            .await
            .map_err(|_| End::Gone),
        TerminalOutput::Closed { exit_code } => {
            // The shell exited, or the container went away under it: the
            // attachment is spent and a `terminal_open` may start another.
            context.terminal = None;
            terminal_closed(out, exit_code).await
        }
    }
}

/// One `terminal_closed { exit_code }`.
async fn terminal_closed(
    out: &mpsc::Sender<Message>,
    exit_code: i64,
) -> std::result::Result<(), End> {
    send(out, ServerMessage::TerminalClosed { exit_code }).await
}

/// Bytes for the PTY (`SPEC.md`, "WebSocket: session stream", the terminal).
///
/// Re-authorized like every other application message: the specification names
/// terminal bytes explicitly ("including terminal bytes"), and a revoked login
/// must not keep typing into somebody's container.
///
/// With no terminal open there is nowhere to put the bytes and they are
/// dropped — the client raced its own `terminal_close`, or never opened one.
/// A write the exec refuses ends the terminal and tells the client so; the
/// socket itself carries on.
async fn handle_binary(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
    bytes: axum::body::Bytes,
) -> std::result::Result<(), End> {
    ensure_authorized(context).await?;

    let Some(terminal) = context.terminal.as_ref() else {
        debug!(session_id = %context.session_id, "session websocket received a binary frame with no terminal open");
        return Ok(());
    };

    // The length only, never the bytes: a terminal carries whatever was typed
    // or pasted into it (rule 3).
    let Err(error) = terminal.write(&bytes).await else {
        return Ok(());
    };

    debug!(session_id = %context.session_id, bytes = bytes.len(), %error, "a terminal write did not reach the exec");

    match context.terminal.take() {
        Some(terminal) => {
            let exit_code = terminal.close().await;
            terminal_closed(out, exit_code).await
        }
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(after: Option<&str>) -> AfterQuery {
        AfterQuery {
            after: after.map(str::to_string),
        }
    }

    #[test]
    fn a_missing_after_replays_from_the_beginning() {
        assert_eq!(query(None).cursor().expect("no cursor is zero"), 0);
    }

    #[test]
    fn a_sequence_is_the_cursor() {
        assert_eq!(query(Some("0")).cursor().expect("zero parses"), 0);
        assert_eq!(query(Some("41")).cursor().expect("a sequence parses"), 41);
    }

    #[test]
    fn a_negative_or_unparsable_after_is_the_one_bad_request() {
        for raw in ["-1", "-100", "", "seven", "1.5", "9999999999999999999999"] {
            let error = query(Some(raw))
                .cursor()
                .expect_err("{raw} is not a cursor");

            match error {
                Error::BadRequest(message) => assert_eq!(message, BAD_AFTER),
                other => panic!("{raw:?} must be a 400, not {other:?}"),
            }
        }
    }
}
