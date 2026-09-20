//! What a user can send into a running session (`SPEC.md`, "WebSocket:
//! session stream").
//!
//! The same shape on both paths: the `input` WebSocket message carries it, and
//! so does `POST /sessions/{id}/input`. Accepting it is the session epic's
//! decision — this module only defines what is sayable.

use serde::{Deserialize, Serialize};

use crate::prelude::*;

/// What an input whose `text` is blank is refused with (400).
pub const EMPTY_TEXT: &str = "text must not be empty";

/// What an input whose `text` is over [`MAX_TEXT_BYTES`] is refused with (400).
pub const LONG_TEXT: &str = "text too long";

/// What an input whose `client_id` is over [`MAX_CLIENT_ID_BYTES`] is refused
/// with (400 over REST, `input_rejected` over the socket).
pub const CLIENT_ID_TOO_LONG: &str = "client_id too long";

/// The longest `client_id` an input may carry.
///
/// The id is the client's own reconciliation key (`SPEC.md`, "WebSocket:
/// session stream") and a UUID is twice inside this; the bound exists because
/// the string is echoed back — in `input_accepted` and in the `user_message`
/// that records the input — and would otherwise let a client decide how much
/// this orchestrator allocates and writes per input.
pub const MAX_CLIENT_ID_BYTES: usize = 128;

/// What is wrong with a `client_id` before anything is asked of the session.
///
/// Both paths that accept one — the socket's `input` message and `POST
/// /sessions/{id}/input` — check it here, so the bound cannot drift between
/// them; the socket turns the 400 into `input_rejected`, as it does for the
/// text's own refusals.
pub fn validate_client_id(client_id: &str) -> Result<()> {
    if client_id.len() > MAX_CLIENT_ID_BYTES {
        return Err(Error::BadRequest(CLIENT_ID_TOO_LONG.to_string()));
    }

    Ok(())
}

/// The longest `text` one input may carry (`SPEC.md`, "Sessions").
///
/// A mebibyte is far more than anything typed and far more than a pasted log or
/// stack trace needs, and it is a bound: without one, a single request decides
/// how much memory the orchestrator buffers and how long a line the owner
/// writes to the CLI's stdin.
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// One input from a client.
///
/// Tagged on `kind`, exactly as the TypeScript union: an unknown `kind` fails
/// to deserialise and the caller answers 400. `message` is the only kind, and
/// the enum stays an enum for that reason — a second kind is a variant, not a
/// change of shape on the wire. There is deliberately no `answer`: the CLI
/// never asks the host a question under the permission flags Mars launches it
/// with, as the live probe recorded against the pinned version (ADR 0033).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionInput {
    /// A message for the agent.
    Message { text: String },
}

impl SessionInput {
    /// The text the user typed, whichever kind this is.
    pub fn text(&self) -> &str {
        match self {
            Self::Message { text } => text,
        }
    }

    /// What is wrong with this input before anything is asked of the session
    /// (`SPEC.md`, "Sessions", `POST /sessions/{id}/input`).
    ///
    /// Blank text and text over [`MAX_TEXT_BYTES`] are the two things a caller
    /// can get wrong about a well-formed input; which states accept one at all
    /// is the session model's and is checked in
    /// [`crate::session::SessionService`]. Both answers are 400, on the REST
    /// route and over the socket, which is why the check lives with the type
    /// rather than in either caller. The length is counted in bytes, because
    /// that is what the cap protects — the memory the request occupies and the
    /// line written to the CLI's stdin — and not in characters.
    pub fn validate(&self) -> Result<()> {
        let text = self.text();

        if text.trim().is_empty() {
            return Err(Error::BadRequest(EMPTY_TEXT.to_string()));
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(Error::BadRequest(LONG_TEXT.to_string()));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_message_round_trips() {
        let input: SessionInput =
            serde_json::from_str(r#"{"kind":"message","text":"hi"}"#).unwrap();
        assert_eq!(
            input,
            SessionInput::Message {
                text: "hi".to_string()
            },
        );
        assert_eq!(
            serde_json::to_value(&input).unwrap(),
            json!({ "kind": "message", "text": "hi" }),
        );
        assert_eq!(input.text(), "hi");
    }

    #[test]
    fn an_unknown_kind_is_rejected() {
        assert!(serde_json::from_str::<SessionInput>(r#"{"kind":"shout","text":"hi"}"#).is_err());
        assert!(serde_json::from_str::<SessionInput>(r#"{"text":"hi"}"#).is_err());
    }

    /// Blank text and text over the documented cap are 400s; everything
    /// between is accepted.
    #[test]
    fn a_text_is_bounded_and_not_blank() {
        let message = |text: &str| SessionInput::Message {
            text: text.to_string(),
        };

        message("hello").validate().expect("ordinary text");
        message(&"x".repeat(MAX_TEXT_BYTES))
            .validate()
            .expect("the cap itself");

        for blank in ["", " ", "\n\t "] {
            let error = message(blank)
                .validate()
                .expect_err("blank text is refused");
            assert_eq!(error.to_string(), EMPTY_TEXT);
        }

        let error = message(&"x".repeat(MAX_TEXT_BYTES + 1))
            .validate()
            .expect_err("a text over the cap is refused");
        assert_eq!(error.to_string(), LONG_TEXT);

        // Bytes, not characters: two-byte characters reach the cap twice as
        // fast, which is what the memory bound means.
        let error = message(&"é".repeat(MAX_TEXT_BYTES / 2 + 1))
            .validate()
            .expect_err("the cap counts bytes");
        assert_eq!(error.to_string(), LONG_TEXT);
    }

    /// The echo key is bounded in bytes, and the bound is the one both the
    /// socket and `POST /sessions/{id}/input` apply.
    #[test]
    fn a_client_id_is_bounded() {
        validate_client_id("").expect("no id at all is an id of length zero");
        validate_client_id("11111111-1111-4111-8111-111111111111").expect("a UUID");
        validate_client_id(&"x".repeat(MAX_CLIENT_ID_BYTES)).expect("the bound itself");

        let error = validate_client_id(&"x".repeat(MAX_CLIENT_ID_BYTES + 1))
            .expect_err("an id over the bound is refused");
        assert_eq!(error.to_string(), CLIENT_ID_TOO_LONG);
    }

    /// `answer` was a kind until the probe showed nothing can ask (ADR 0033).
    /// It must now fail like any other unknown kind rather than being accepted
    /// and silently treated as a message.
    #[test]
    fn an_answer_is_no_longer_a_kind() {
        assert!(
            serde_json::from_str::<SessionInput>(r#"{"kind":"answer","reply_to":7,"text":"yes"}"#)
                .is_err()
        );
    }
}
