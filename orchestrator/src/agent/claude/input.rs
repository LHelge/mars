//! What the owner writes into the CLI's stdin (`ARCHITECTURE.md`, "Input
//! encoding").
//!
//! One JSON line per user input, in the SDK user-message shape. The shape is
//! not in the CLI reference documentation; it is what the Agent SDK sends over
//! the same protocol, and the live probe against the pinned version is what
//! confirms it. Two consequences for this module: the shape is written in one
//! place so the probe has one place to correct, and nothing is added to it on
//! spec — no `session_id`, because the CLI is not known to want one
//! (`docs/open-questions.md`, item 1).
//!
//! An `answer` is encoded exactly like a `message`. Under
//! `--permission-prompts none` the CLI is not expected to ask anything, so
//! there is no distinct answer shape to send; `reply_to` is orchestrator
//! bookkeeping that ties the answer to the `prompt` event in the transcript and
//! never reaches the CLI (`docs/open-questions.md`, item 3).

use serde_json::Value;

use crate::events::SessionInput;
use crate::prelude::*;

/// What an empty input is rejected with.
const EMPTY_INPUT: &str = "input text must not be empty";

/// One input as the line to write to stdin, terminated by the newline the CLI
/// reads lines on.
///
/// Whitespace-only text is rejected rather than sent: the CLI would answer a
/// turn to it, and the owner would have recorded a `user_message` for a message
/// the user did not write.
pub(crate) fn encode_input(input: &SessionInput) -> Result<String> {
    let text = input.text();
    if text.trim().is_empty() {
        return Err(Error::BadRequest(EMPTY_INPUT.to_string()));
    }

    Ok(user_message_line(text))
}

/// The documented stdin line for one message text, newline included.
///
/// Compact output, so the whole message is one line whatever the text
/// contains: a newline, a quote and a control character are all escaped by
/// serde, and the only raw newline in the returned string is the terminator.
///
/// The envelope's four constant keys are written out rather than assembled
/// from a `serde_json::Value`, for two reasons: the field order is then the
/// documented one whichever map implementation `serde_json` is built with, and
/// the function cannot fail, so the owner's write path has no error to handle.
/// The one variable — the text — goes through serde's string escaping.
pub(crate) fn user_message_line(text: &str) -> String {
    let text = Value::String(text.to_string());

    format!(
        r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":{text}}}]}}}}"#
    ) + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_the_documented_sdk_user_message_line() {
        let encoded = encode_input(&SessionInput::Message {
            text: "hi".to_string(),
        })
        .unwrap();

        assert_eq!(
            encoded,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\
             \"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n",
        );
    }

    #[test]
    fn quotes_newlines_and_unicode_are_escaped_into_one_line() {
        let encoded = encode_input(&SessionInput::Message {
            text: "say \"hi\"\nthen ✓".to_string(),
        })
        .unwrap();

        assert_eq!(
            encoded,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\
             [{\"type\":\"text\",\"text\":\"say \\\"hi\\\"\\nthen ✓\"}]}}\n",
        );
        // Exactly one newline, and it is the terminator.
        assert_eq!(encoded.matches('\n').count(), 1);
        assert!(encoded.ends_with('\n'));
    }

    #[test]
    fn an_answer_is_encoded_as_the_same_shape_without_its_reply_to() {
        let answer = encode_input(&SessionInput::Answer {
            reply_to: 7,
            text: "yes".to_string(),
        })
        .unwrap();

        assert_eq!(
            answer,
            encode_input(&SessionInput::Message {
                text: "yes".to_string(),
            })
            .unwrap(),
        );
        assert!(!answer.contains("reply_to"));
        assert!(!answer.contains('7'));
    }

    #[test]
    fn the_line_carries_no_session_id() {
        let encoded = encode_input(&SessionInput::Message {
            text: "hi".to_string(),
        })
        .unwrap();
        assert!(!encoded.contains("session_id"));
    }

    #[test]
    fn empty_or_whitespace_only_text_is_rejected() {
        for text in ["", "   ", "\n\t "] {
            let error = encode_input(&SessionInput::Message {
                text: text.to_string(),
            })
            .unwrap_err();
            assert!(
                matches!(&error, Error::BadRequest(message) if message == EMPTY_INPUT),
                "{text:?} produced {error:?}",
            );

            let error = encode_input(&SessionInput::Answer {
                reply_to: 1,
                text: text.to_string(),
            })
            .unwrap_err();
            assert!(matches!(&error, Error::BadRequest(message) if message == EMPTY_INPUT));
        }
    }

    #[test]
    fn surrounding_whitespace_is_sent_as_the_user_wrote_it() {
        // Only an entirely blank input is rejected; the text itself is never
        // rewritten, because the owner hashes exactly what it sends.
        let encoded = encode_input(&SessionInput::Message {
            text: "  padded  ".to_string(),
        })
        .unwrap();
        assert!(encoded.contains("\"text\":\"  padded  \""));
    }
}
