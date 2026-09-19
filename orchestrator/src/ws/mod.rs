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
//! writer task that owns the sink. The main loop, and later the terminal
//! reader, both hold a `Sender` and neither has to share the sink. A send that
//! fails or times out ends the writer, which closes the channel, which is how
//! the main loop learns to stop: after a failed write nothing further is
//! written.
//!
//! The input, stop and terminal messages are parsed here and handed to
//! [`handle_client_message`] and [`handle_binary`], which are the seams the
//! input and terminal tasks fill in. The loop itself — the `select!` over the
//! socket, the fan-out receiver, the safety read and the ping — is settled
//! here.

pub mod protocol;

use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::response::Response;
use axum::routing::get;
use futures_util::SinkExt;
use futures_util::stream::{SplitStream, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::events::Notice;
use crate::prelude::*;
use crate::repositories::SessionRepository;
use crate::routes::{StreamAuthFailure, StreamPrincipal, StreamToken, authenticate_stream};
use crate::ws::protocol::{ClientMessage, ServerMessage};

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
    /// Who the socket was opened as, re-checked by [`reauthorize_hook`].
    principal: StreamPrincipal,
    cursor: i64,
    missed_pongs: u8,
    /// The terminal exec, when one is open. Always `None` here: opening it is
    /// the terminal task's, and this field is the seam it fills.
    #[allow(dead_code, reason = "the terminal task opens and reads this")]
    terminal: Option<Terminal>,
}

/// The PTY exec multiplexed onto this socket (`SPEC.md`, "WebSocket: session
/// stream", the terminal).
///
/// A placeholder: the terminal task gives it the exec handle and the reader's
/// join handle. It exists now so the shape of [`SocketContext`] does not have
/// to change when it arrives.
#[allow(dead_code, reason = "the terminal task constructs this")]
struct Terminal;

/// Why the socket task stopped.
///
/// Every arm of the loop returns one of these rather than writing a close
/// frame itself, so there is exactly one place that closes the socket.
enum End {
    /// The peer went away, or a write failed: nothing more can be written.
    Gone,
    /// Close cleanly with this code, after an `error` frame carrying
    /// `message` if one is given.
    ///
    /// The text rather than a [`ServerMessage`]: the only message that ever
    /// precedes a close is `error`, and a whole `ServerMessage` here would put
    /// a `Session` in every `Err` this module returns.
    Close { code: u16, message: Option<String> },
}

impl End {
    /// An `error` frame followed by a close with `code`.
    fn error(code: u16, message: &str) -> Self {
        End::Close {
            code,
            message: Some(message.to_string()),
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

    let mut context = SocketContext {
        state,
        session_id,
        principal,
        cursor: after,
        missed_pongs: 0,
        terminal: None,
    };

    let (sink, mut incoming) = socket.split();
    let (out, writer) = spawn_writer(sink);

    let end = stream(&mut context, &mut rx, &out, &mut incoming).await;
    let closing = matches!(end, End::Close { .. });

    if let End::Close { code, message } = end {
        if let Some(message) = message {
            let _ = send(&out, ServerMessage::Error { message }).await;
        }
        let _ = out
            .send(Message::Close(Some(CloseFrame {
                code,
                reason: "".into(),
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
    // Both fire immediately on their first tick; the stream has just read from
    // its cursor and has nothing to ping about yet.
    safety.tick().await;
    ping.tick().await;

    loop {
        let step = tokio::select! {
            notice = rx.recv() => on_notice(context, out, notice).await,
            _ = safety.tick() => drain(context, out).await,
            _ = ping.tick() => on_ping(context, out).await,
            frame = incoming.next() => on_frame(context, out, frame).await,
        };

        if let Err(end) = step {
            return end;
        }
    }
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

/// One ping tick: re-authorize, ping, and count the pong that has not come.
///
/// The count is raised *after* the ping is sent, so it names the pings still
/// outstanding: reaching [`MAX_MISSED_PONGS`] means two ticks passed with no
/// pong in between, which is the documented close.
async fn on_ping(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
) -> std::result::Result<(), End> {
    if let Err(failure) = reauthorize_hook(context).await {
        debug!(session_id = %context.session_id, %failure, "session websocket closed by re-authorization");
        return Err(End::error(close_code::POLICY, &failure.to_string()));
    }

    out.send(Message::Ping(PING_PAYLOAD.into()))
        .await
        .map_err(|_| End::Gone)?;

    context.missed_pongs = context.missed_pongs.saturating_add(1);
    if context.missed_pongs >= MAX_MISSED_PONGS {
        debug!(session_id = %context.session_id, "session websocket closed after two missed pongs");
        return Err(End::Close {
            code: close_code::AWAY,
            message: None,
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

/// The per-tick half of the stream authentication contract (`SPEC.md`,
/// "Authentication"; `routes::stream_auth`).
///
/// A no-op today: this task builds the read side, and nothing it sends depends
/// on the user still being authorized to *write*. The input task replaces the
/// body with `routes::reauthorize(&context.state, &context.principal)`, at
/// which point a revoked login closes the socket on the next ping.
async fn reauthorize_hook(context: &SocketContext) -> std::result::Result<(), StreamAuthFailure> {
    let _ = context.principal;
    Ok(())
}

/// Act on a parsed client message.
///
/// The seam for the input and terminal tasks: `input` and `stop` go to the
/// session's owner through the registry, `terminal_*` to the exec. Until then
/// a well-formed message is accepted and ignored, which is deliberately not
/// the same as refusing it — the protocol is what this task fixes, not the
/// behaviour behind it.
async fn handle_client_message(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
    message: ClientMessage,
) -> std::result::Result<(), End> {
    let _ = (out, message);
    debug!(session_id = %context.session_id, "session websocket received a client message");

    Ok(())
}

/// Bytes for the PTY (`SPEC.md`, "WebSocket: session stream", the terminal).
///
/// The terminal task writes them to the exec; with no terminal open there is
/// nowhere to put them and they are dropped.
async fn handle_binary(
    context: &mut SocketContext,
    out: &mpsc::Sender<Message>,
    bytes: axum::body::Bytes,
) -> std::result::Result<(), End> {
    let _ = (out, bytes);
    debug!(session_id = %context.session_id, "session websocket received a binary frame with no terminal open");

    Ok(())
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
