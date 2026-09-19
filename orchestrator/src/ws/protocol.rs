//! The JSON frames the session WebSocket speaks (`SPEC.md`, "WebSocket:
//! session stream").
//!
//! The two tables in that section, one enum each, tagged on `type`. Nothing
//! here does any work: the point of writing the protocol down as types is that
//! the wire shape is decided once, in the same order as the document, and
//! every handler in `ws` is then obliged to spell a message correctly or not
//! compile.
//!
//! Terminal *data* is not here. It travels as binary frames in both
//! directions, which is the one part of the protocol that is not JSON;
//! `terminal_closed` is a JSON message and is.
//!
//! [`ServerMessage`] is only [`Serialize`] because
//! [`Session`](crate::models::Session) deliberately has no `Deserialize` — a
//! session row only ever comes out of the database — and [`ClientMessage`] is
//! both, so a test can send one the same way a browser does.

use serde::{Deserialize, Serialize};

use crate::events::{SessionEvent, SessionInput};
use crate::models::Session;

/// What the orchestrator sends (`SPEC.md`, "WebSocket: session stream",
/// server to client).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// One transcript event, replayed or live.
    ///
    /// Always a [`SessionEvent`] built by
    /// [`SessionEvent::from_row`](crate::events::SessionEvent::from_row), which
    /// is where the internal `_`-prefixed payload fields are dropped, so
    /// `_offset` cannot leave through this message (`SPEC.md`, "AgentEvent").
    Event { event: SessionEvent },
    /// The session row: once immediately after the upgrade, and then on every
    /// state change.
    Session { session: Session },
    /// An input the orchestrator took responsibility for, with the highest
    /// committed sequence at acceptance; the `user_message` event recording it
    /// follows later with the same `client_id`. Acceptance, not delivery
    /// (ADR 0020).
    InputAccepted { client_id: String, seq: i64 },
    /// An input the orchestrator refused, with the reason the REST route would
    /// have answered 400 or 409 with.
    InputRejected { client_id: String, reason: String },
    /// The terminal exec ended.
    TerminalClosed { exit_code: i64 },
    /// The stream is ending; a close frame follows.
    Error { message: String },
}

/// What a client may send (`SPEC.md`, "WebSocket: session stream", client to
/// server).
///
/// An unknown `type`, a missing field or a field of the wrong type all fail to
/// deserialise, and the handler answers the one documented way: `error
/// { message: "malformed message" }` and a close.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// A message for the agent, with the client-generated id echoed back on
    /// the answer.
    Input {
        client_id: String,
        input: SessionInput,
    },
    /// Interrupt the current turn (`ARCHITECTURE.md`, "Stop semantics").
    Stop,
    /// Open the terminal exec at this size; the session must be `running`.
    TerminalOpen { cols: u16, rows: u16 },
    /// The terminal window changed size.
    TerminalResize { cols: u16, rows: u16 },
    /// Close the terminal exec.
    TerminalClose,
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use serde_json::{Value, json};

    use super::*;
    use crate::events::{AgentEvent, AgentEventBody};

    /// A fixed, obviously fake timestamp, so the literal JSON below is stable.
    fn ts() -> DateTime<Utc> {
        "2026-01-02T03:04:05Z"
            .parse()
            .expect("a fixed RFC 3339 timestamp parses")
    }

    #[test]
    fn an_event_message_is_the_flat_agent_event_under_type_event() {
        let message = ServerMessage::Event {
            event: SessionEvent {
                seq: 42,
                ts: ts(),
                event: AgentEvent::new(AgentEventBody::Text {
                    text: "hello".to_string(),
                }),
            },
        };

        assert_eq!(
            serde_json::to_value(&message).expect("the message serialises"),
            json!({
                "type": "event",
                "event": {
                    "seq": 42,
                    "ts": "2026-01-02T03:04:05Z",
                    "kind": "text",
                    "text": "hello",
                },
            }),
        );
    }

    #[test]
    fn the_input_answers_carry_the_client_id_verbatim() {
        assert_eq!(
            serde_json::to_value(ServerMessage::InputAccepted {
                client_id: "client-1".to_string(),
                seq: 7,
            })
            .expect("the message serialises"),
            json!({ "type": "input_accepted", "client_id": "client-1", "seq": 7 }),
        );

        assert_eq!(
            serde_json::to_value(ServerMessage::InputRejected {
                client_id: "client-1".to_string(),
                reason: "session is done".to_string(),
            })
            .expect("the message serialises"),
            json!({
                "type": "input_rejected",
                "client_id": "client-1",
                "reason": "session is done",
            }),
        );
    }

    #[test]
    fn terminal_closed_and_error_are_the_two_remaining_server_messages() {
        assert_eq!(
            serde_json::to_value(ServerMessage::TerminalClosed { exit_code: 130 })
                .expect("the message serialises"),
            json!({ "type": "terminal_closed", "exit_code": 130 }),
        );

        assert_eq!(
            serde_json::to_value(ServerMessage::Error {
                message: "malformed message".to_string(),
            })
            .expect("the message serialises"),
            json!({ "type": "error", "message": "malformed message" }),
        );
    }

    /// Both directions of every client message: the literal JSON a browser
    /// sends parses into the variant, and the variant serialises back into the
    /// same JSON.
    fn round_trip(raw: Value, expected: ClientMessage) {
        let parsed =
            serde_json::from_value::<ClientMessage>(raw.clone()).expect("the message parses");

        assert_eq!(parsed, expected);
        assert_eq!(
            serde_json::to_value(&parsed).expect("the message serialises"),
            raw,
        );
    }

    #[test]
    fn every_client_message_round_trips_through_its_documented_json() {
        round_trip(
            json!({
                "type": "input",
                "client_id": "abc",
                "input": { "kind": "message", "text": "hi" },
            }),
            ClientMessage::Input {
                client_id: "abc".to_string(),
                input: SessionInput::Message {
                    text: "hi".to_string(),
                },
            },
        );
        round_trip(json!({ "type": "stop" }), ClientMessage::Stop);
        round_trip(
            json!({ "type": "terminal_open", "cols": 120, "rows": 40 }),
            ClientMessage::TerminalOpen {
                cols: 120,
                rows: 40,
            },
        );
        round_trip(
            json!({ "type": "terminal_resize", "cols": 80, "rows": 24 }),
            ClientMessage::TerminalResize { cols: 80, rows: 24 },
        );
        round_trip(
            json!({ "type": "terminal_close" }),
            ClientMessage::TerminalClose,
        );
    }

    #[test]
    fn an_unknown_or_incomplete_client_message_does_not_parse() {
        for raw in [
            json!({ "type": "resume" }),
            json!({ "type": "input", "client_id": "abc" }),
            json!({ "type": "input", "client_id": "abc", "input": { "kind": "answer" } }),
            json!({ "type": "terminal_open", "cols": 120 }),
            json!({ "cols": 120, "rows": 40 }),
        ] {
            assert!(
                serde_json::from_value::<ClientMessage>(raw.clone()).is_err(),
                "{raw} must not parse as a client message",
            );
        }
    }
}
