//! The Claude Code adapter (`ARCHITECTURE.md`, "Claude Code invocation").
//!
//! The launch command is built in [`launch`], `translate` dispatches to
//! [`translate::translate_line`] and `encode_input` to
//! [`input::encode_input`], which writes the SDK user-message shape
//! (`ARCHITECTURE.md`, "Input encoding").

use std::any::Any;

use super::{AgentBackend, Command, CredentialName, LaunchContext, TranslateState};
use crate::events::{AgentEvent, SessionInput};
use crate::prelude::*;

mod input;
pub mod launch;
mod native;
mod translate;

/// The argv builder, re-exported so the live probe can launch the real CLI
/// with the adapter's own command line (`tests/claude_probe.rs`).
pub use launch::build_argv;
/// The subagent tool names, re-exported for the same reason: the probe asserts
/// the tool the CLI actually used is one of them.
pub use translate::SUBAGENT_TOOL_NAMES;

/// The Claude Code version this adapter is written against.
///
/// The same version `images/claude/Dockerfile` pins with
/// `ARG CLAUDE_CODE_VERSION` and records in the image tag, and the version the
/// fixtures under `tests/fixtures/claude/` were recorded from. A bump changes
/// all three together and adds fixtures rather than editing old ones
/// (`CLAUDE.md`, "Testing expectations").
pub const CLAUDE_CLI_VERSION: &str = "2.1.282";

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

    /// The stdin line documented in `ARCHITECTURE.md`, "Input encoding", built
    /// in [`input`].
    fn encode_input(&self, input: &SessionInput) -> Result<String> {
        input::encode_input(input)
    }

    /// What the CLI authenticates with (`ARCHITECTURE.md`, "Claude Code
    /// invocation", Credentials; ADR 0036).
    ///
    /// The subscription token first: a scope holding both is a row older than
    /// the one-per-scope write rule, and the CLI's own precedence would
    /// silently take the API key and bill it, which is the outcome nobody
    /// asked for.
    fn credential_names(&self) -> &'static [CredentialName] {
        &[
            CredentialName::ClaudeCodeOauthToken,
            CredentialName::AnthropicApiKey,
        ]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::{LaunchMode, TranslateConfig};
    use super::*;

    /// The pin is one fact in three places: this constant, the Dockerfile's
    /// `ARG CLAUDE_CODE_VERSION` and the image tag built from it
    /// (`ARCHITECTURE.md`, "Session image"). This test is what keeps them from
    /// drifting, so it reads the Dockerfile rather than a copy of it and names
    /// both versions when they differ.
    #[test]
    fn the_pinned_version_matches_the_image() {
        const PIN: &str = "ARG CLAUDE_CODE_VERSION=";

        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../images/claude/Dockerfile");
        let dockerfile = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("{path} could not be read: {error}"));

        let pinned = dockerfile
            .lines()
            .find_map(|line| line.strip_prefix(PIN))
            .unwrap_or_else(|| {
                panic!("{path} has no `{PIN}<x.y.z>` line; the image's pin must stay greppable")
            })
            .trim();

        assert_eq!(
            pinned, CLAUDE_CLI_VERSION,
            "the pins have drifted apart: the image pins {pinned}, the adapter pins \
             {CLAUDE_CLI_VERSION}; bump both, retag the image and add fixtures under \
             tests/fixtures/claude/{pinned}/",
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

    /// The shape itself is tested in `input.rs`; this is the wiring.
    #[test]
    fn encode_input_dispatches_to_the_encoder() {
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

        assert!(
            ClaudeBackend::new()
                .encode_input(&SessionInput::Message {
                    text: "  ".to_string(),
                })
                .is_err(),
        );
    }
}
