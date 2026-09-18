//! The Claude Code adapter (`ARCHITECTURE.md`, "Claude Code invocation").
//!
//! The launch command is built in [`launch`], `translate` dispatches to
//! [`translate::translate_line`] — whose `assistant`, `user` and
//! `stream_event` branches still answer `raw` until their tasks land — and
//! `encode_input` writes the SDK user-message shape (`ARCHITECTURE.md`,
//! "Input encoding").

use std::any::Any;

use serde_json::json;

use super::{AgentBackend, Command, LaunchContext, TranslateState};
use crate::events::{AgentEvent, SessionInput};
use crate::prelude::*;

pub mod launch;
mod native;
mod translate;

/// The Claude Code version this adapter is written against.
///
/// The same version `images/claude/Dockerfile` pins with
/// `ARG CLAUDE_CODE_VERSION` and records in the image tag, and the version the
/// fixtures under `tests/fixtures/claude/` were recorded from. A bump changes
/// all three together and adds fixtures rather than editing old ones
/// (`CLAUDE.md`, "Testing expectations").
pub const CLAUDE_CLI_VERSION: &str = "2.1.274";

/// The CLI binary as it is found on the session image's `PATH`.
pub const CLAUDE_CLI_BINARY: &str = launch::BINARY;

/// The Claude Code adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeBackend;

impl ClaudeBackend {
    pub fn new() -> Self {
        Self
    }
}

impl AgentBackend for ClaudeBackend {
    /// The invocation documented in `ARCHITECTURE.md`, "Claude Code
    /// invocation", built in [`launch`].
    fn launch_command(&self, ctx: &LaunchContext) -> Command {
        launch::launch_command(ctx)
    }

    /// The `stream-json` translation rules (`SPEC.md`, "AgentEvent").
    fn translate(&self, line: &str, state: &mut TranslateState) -> Vec<AgentEvent> {
        translate::translate_line(line, state)
    }

    fn encode_input(&self, input: &SessionInput) -> Result<String> {
        let line = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{ "type": "text", "text": input.text() }],
            },
        });
        Ok(format!("{line}\n"))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::super::{LaunchMode, TranslateConfig};
    use super::*;

    #[test]
    fn the_pinned_version_matches_the_image() {
        let dockerfile = include_str!("../../../../images/claude/Dockerfile");
        assert!(
            dockerfile.contains(&format!("ARG CLAUDE_CODE_VERSION={CLAUDE_CLI_VERSION}")),
            "the adapter's pin and the image's pin have drifted apart",
        );
    }

    #[test]
    fn the_launch_command_is_the_documented_invocation() {
        let ctx = LaunchContext::new(LaunchMode::Conversational { resume: None });
        assert_eq!(
            ClaudeBackend::new().launch_command(&ctx).argv,
            launch::build_argv(&ctx),
        );
        assert_eq!(
            ClaudeBackend::new().launch_command(&ctx).argv.first(),
            Some(&CLAUDE_CLI_BINARY.to_string()),
        );
    }

    /// The rules themselves are tested in `translate.rs`; this is the wiring.
    #[test]
    fn translate_dispatches_to_the_translator() {
        use crate::events::AgentEventBody;
        use crate::models::AgentBackend as Backend;

        let mut state = TranslateState::new(TranslateConfig::default());
        let events = ClaudeBackend::new().translate(
            r#"{"type":"system","subtype":"init","session_id":"fake-cli-session"}"#,
            &mut state,
        );
        assert_eq!(
            events,
            vec![
                AgentEventBody::Init {
                    cli_session_id: "fake-cli-session".to_string(),
                    model: None,
                    tools: vec![],
                    mcp_servers: vec![],
                    resumed: false,
                }
                .into()
            ],
        );

        let events = ClaudeBackend::new().translate("not json", &mut state);
        assert_eq!(
            events,
            vec![
                AgentEventBody::Raw {
                    backend: Backend::Claude,
                    native: json!("not json"),
                }
                .into()
            ],
        );
    }

    #[test]
    fn an_input_is_encoded_as_one_sdk_user_message_line() {
        let encoded = ClaudeBackend::new()
            .encode_input(&SessionInput::Message {
                text: "hi".to_string(),
            })
            .unwrap();
        assert!(encoded.ends_with('\n'));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(encoded.trim_end()).unwrap(),
            json!({
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{ "type": "text", "text": "hi" }],
                },
            }),
        );
    }
}
