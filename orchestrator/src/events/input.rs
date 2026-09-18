//! What a user can send into a running session (`SPEC.md`, "WebSocket:
//! session stream").
//!
//! The same shape on both paths: the `input` WebSocket message carries it, and
//! so does `POST /sessions/{id}/input`. Accepting it is the session epic's
//! decision — this module only defines what is sayable.

use serde::{Deserialize, Serialize};

// The crate convention (`CLAUDE.md`, "Backend conventions"); this module needs
// nothing from the prelude yet.
#[allow(unused_imports)]
use crate::prelude::*;

/// One input from a client.
///
/// Tagged on `kind`, exactly as the TypeScript union: an unknown `kind` fails
/// to deserialise and the caller answers 400.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionInput {
    /// A message for the agent.
    Message { text: String },
    /// An answer to a `prompt` event.
    Answer {
        /// The `seq` of the prompt event being answered.
        reply_to: i64,
        text: String,
    },
}

impl SessionInput {
    /// The text the user typed, whichever kind this is.
    pub fn text(&self) -> &str {
        match self {
            Self::Message { text } | Self::Answer { text, .. } => text,
        }
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
    fn an_answer_round_trips() {
        let input: SessionInput =
            serde_json::from_str(r#"{"kind":"answer","reply_to":7,"text":"yes"}"#).unwrap();
        assert_eq!(
            input,
            SessionInput::Answer {
                reply_to: 7,
                text: "yes".to_string(),
            },
        );
        assert_eq!(
            serde_json::to_value(&input).unwrap(),
            json!({ "kind": "answer", "reply_to": 7, "text": "yes" }),
        );
        assert_eq!(input.text(), "yes");
    }

    #[test]
    fn an_unknown_kind_is_rejected() {
        assert!(serde_json::from_str::<SessionInput>(r#"{"kind":"shout","text":"hi"}"#).is_err());
        assert!(serde_json::from_str::<SessionInput>(r#"{"text":"hi"}"#).is_err());
        assert!(serde_json::from_str::<SessionInput>(r#"{"kind":"answer","text":"yes"}"#).is_err());
    }
}
