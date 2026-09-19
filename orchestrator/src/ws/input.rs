//! The write side of the session socket: `input` and `stop` (`SPEC.md`,
//! "WebSocket: session stream"; `ARCHITECTURE.md`, "Event delivery").
//!
//! Neither handler decides anything. Both go through
//! [`SessionService`](crate::session::SessionService), which is the same call
//! `POST /sessions/{id}/input` and `POST /sessions/{id}/stop` make, so the
//! socket and REST cannot drift apart: the relaunch of a parked session, the
//! refusal of an ephemeral one and the queue bound are all the service's and
//! the registry's, and this module only turns their answers into frames.
//!
//! The mapping is the one the two documents fix:
//!
//! | The service says | The socket sends |
//! | --- | --- |
//! | `Ok(())` | `input_accepted { client_id, seq }` |
//! | `Err(Conflict(reason))` | `input_rejected { client_id, reason }`, verbatim |
//! | `Err(BadRequest(reason))` | `input_rejected { client_id, reason }` ("refused ... `input_rejected` over the socket") |
//! | `Err(NotFound)` | `error { session not found }` and a normal close |
//! | anything else | `error { internal error }` and 1011 |
//!
//! `input_accepted` is acceptance by the orchestrator and not delivery to the
//! CLI (ADR 0020): a session that is `creating`, or parked and being resumed,
//! queues the input and is acknowledged just the same.
//!
//! The input text is never logged, at any level: it is transcript content
//! (`CLAUDE.md`, rule 3; ADR 0027).

use axum::extract::ws::{Message, close_code};
use axum::http::StatusCode;
use tokio::sync::mpsc;

use super::{End, INTERNAL, SESSION_NOT_FOUND, SocketContext, send};
use crate::events::SessionInput;
use crate::prelude::*;
use crate::repositories::SessionRepository;
use crate::ws::protocol::ServerMessage;

/// The longest `client_id` the socket echoes.
///
/// The id is the client's own reconciliation key (`SPEC.md`, "WebSocket:
/// session stream") and a UUID is twice inside this; the bound exists because
/// the string is echoed back and would otherwise let a client decide how much
/// this orchestrator allocates and writes per input.
const MAX_CLIENT_ID_BYTES: usize = 128;

/// What an over-long `client_id` is refused with, before the service is asked
/// anything at all.
const CLIENT_ID_TOO_LONG: &str = "client_id too long";

/// `input { client_id, input }` (`SPEC.md`, "WebSocket: session stream").
///
/// The `seq` in the acknowledgement is the highest committed sequence at
/// acceptance, read after the service returned: the `user_message` event that
/// records this input is written by the owner and arrives later, over the read
/// side, carrying the same `client_id`.
pub(super) async fn handle_input(
    context: &SocketContext,
    out: &mpsc::Sender<Message>,
    client_id: String,
    input: SessionInput,
) -> std::result::Result<(), End> {
    if client_id.len() > MAX_CLIENT_ID_BYTES {
        debug!(
            session_id = %context.session_id,
            bytes = client_id.len(),
            "an input carried a client_id over the bound",
        );
        return reject(out, client_id, CLIENT_ID_TOO_LONG.to_string()).await;
    }

    // Blank text and text over the cap are the REST route's two 400s, and the
    // document says the socket answers them with `input_rejected` rather than
    // a close.
    let accepted = match input.validate() {
        Ok(()) => {
            context
                .service
                .send_input(
                    context.session_id,
                    input,
                    Some(context.principal.user_id),
                    Some(client_id.clone()),
                )
                .await
        }
        Err(invalid) => Err(invalid),
    };

    match accepted {
        Ok(()) => {
            let seq = SessionRepository::new(&context.state.pool)
                .max_seq(context.session_id)
                .await
                .map_err(|error| {
                    error!(
                        session_id = %context.session_id,
                        %error,
                        "an accepted input could not be acknowledged with a sequence",
                    );
                    End::error(close_code::ERROR, INTERNAL)
                })?;

            send(out, ServerMessage::InputAccepted { client_id, seq }).await
        }
        Err(refusal) => refuse(context, out, client_id, refusal).await,
    }
}

/// Turn a refused input into the frame the client is owed.
///
/// The mapping is [`Error::status`]'s, which exists for exactly this: the
/// socket answers what the REST route would have, without building an HTTP
/// response. A 400 or a 409 is the user's own doing and is
/// [`ServerMessage::InputRejected`] with [`Error::user_message`] — the
/// service's and the model's own words, which is what makes the two paths say
/// the same thing. A 404 is a session deleted under the open socket, which
/// the read side ends the same way. Anything else is this orchestrator's
/// fault: it is logged in full and the client is told nothing but
/// `internal error`.
async fn refuse(
    context: &SocketContext,
    out: &mpsc::Sender<Message>,
    client_id: String,
    refusal: Error,
) -> std::result::Result<(), End> {
    let status = refusal.status();

    if status == StatusCode::BAD_REQUEST || status == StatusCode::CONFLICT {
        let reason = refusal.user_message();
        debug!(session_id = %context.session_id, reason, "an input was refused");
        return reject(out, client_id, reason).await;
    }

    if status == StatusCode::NOT_FOUND {
        return Err(End::error(close_code::NORMAL, SESSION_NOT_FOUND));
    }

    error!(session_id = %context.session_id, error = %refusal, "an input could not be accepted");
    Err(End::error(close_code::ERROR, INTERNAL))
}

/// `stop {}` (`SPEC.md`, "WebSocket: session stream"; `ARCHITECTURE.md`,
/// "Stop semantics").
///
/// Nothing is sent on success: the `SIGINT`, the state change and the
/// `session` snapshot that follows it all arrive through the read side.
///
/// A session that is not `running` has no run to stop, and the protocol has no
/// message for a refused stop — `error` is followed by a close, which would
/// take the stream down over a button pressed twice — so the conflict is
/// logged and the socket carries on.
pub(super) async fn handle_stop(context: &SocketContext) -> std::result::Result<(), End> {
    let Err(refusal) = context.service.stop(context.session_id).await else {
        return Ok(());
    };

    match refusal.status() {
        StatusCode::CONFLICT => {
            debug!(
                session_id = %context.session_id,
                reason = %refusal.user_message(),
                "a stop was refused",
            );
            Ok(())
        }
        StatusCode::NOT_FOUND => Err(End::error(close_code::NORMAL, SESSION_NOT_FOUND)),
        _ => {
            error!(session_id = %context.session_id, error = %refusal, "a session could not be stopped");
            Err(End::error(close_code::ERROR, INTERNAL))
        }
    }
}

/// One `input_rejected`, with the client's id echoed verbatim.
async fn reject(
    out: &mpsc::Sender<Message>,
    client_id: String,
    reason: String,
) -> std::result::Result<(), End> {
    send(out, ServerMessage::InputRejected { client_id, reason }).await
}
