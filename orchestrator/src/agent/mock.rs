//! The agent-backend mock, compiled only with the `integration-tests` feature.
//!
//! It scripts a session's output exactly: a test writes `AgentEvent` JSON into
//! the transcript the owner tails and gets those events back, so the owner's
//! behaviour can be exercised without any CLI and without the Claude adapter
//! (`CLAUDE.md`, "Testing expectations"). Nothing here depends on
//! [`super::claude`].

use std::any::Any;
use std::sync::{Mutex, MutexGuard};

use serde_json::json;

use super::{AgentBackend, Command, CredentialName, LaunchContext, TranslateState};
use crate::events::{AgentEvent, AgentEventBody, SessionInput};
use crate::models::AgentBackend as Backend;
use crate::prelude::*;

/// The single argv element the mock's launch command carries.
pub const MOCK_CLI: &str = "mock-cli";

/// A backend that records its launches and translates scripted events.
#[derive(Debug, Default)]
pub struct MockAgentBackend {
    launches: Mutex<Vec<LaunchContext>>,
}

impl MockAgentBackend {
    /// A fresh mock with no recorded launches.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every context it was asked for a command with, in order. Cloned out of
    /// the mutex, so a test never holds the lock across an await.
    pub fn launches(&self) -> Vec<LaunchContext> {
        self.lock().clone()
    }

    /// Forget every recorded launch.
    pub fn clear(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> MutexGuard<'_, Vec<LaunchContext>> {
        // Test-only code: a poisoned lock means another test thread already
        // panicked, which is a failure in its own right.
        self.launches
            .lock()
            .expect("the mock backend lock is healthy")
    }
}

impl AgentBackend for MockAgentBackend {
    fn launch_command(&self, ctx: &LaunchContext) -> Command {
        self.lock().push(ctx.clone());
        Command {
            argv: vec![MOCK_CLI.to_string()],
        }
    }

    /// A line that is an `AgentEvent` is that event; anything else is `raw`.
    ///
    /// The state is untouched: the mock has no native shapes to remember, and
    /// a test that wants an echo suppressed scripts the events it wants.
    fn translate(&self, line: &str, _state: &mut TranslateState) -> Vec<AgentEvent> {
        match serde_json::from_str::<AgentEvent>(line) {
            Ok(event) => vec![event],
            Err(_) => {
                let native = serde_json::from_str(line).unwrap_or_else(|_| json!(line));
                vec![
                    AgentEventBody::Raw {
                        // The one value the enum has in v1; the mock stands in
                        // for whichever backend a test is driving.
                        backend: Backend::Claude,
                        native,
                    }
                    .into(),
                ]
            }
        }
    }

    fn encode_input(&self, input: &SessionInput) -> Result<String> {
        // The trait's contract, so a test driving the mock sees the same
        // rejection a real adapter gives.
        if input.text().trim().is_empty() {
            return Err(Error::BadRequest(
                "input text must not be empty".to_string(),
            ));
        }

        let line = serde_json::to_string(input).map_err(|err| {
            error!(error = %err, "the mock backend failed to encode an input");
            Error::Internal("failed to encode the session input".to_string())
        })?;
        Ok(format!("{line}\n"))
    }

    /// The Claude adapter's two names, in its order.
    ///
    /// The mock stands in for whichever backend a test drives, and a launcher
    /// test that seeds a credential seeds it under a real name, so the mock
    /// declaring the same list is what keeps those tests shaped like a launch
    /// (ADR 0036).
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
    use super::super::{LaunchMode, TranslateConfig};
    use super::*;

    #[test]
    fn every_launch_is_recorded_and_the_command_is_fixed() {
        let backend = MockAgentBackend::new();
        let ctx = LaunchContext::new(LaunchMode::Ephemeral {
            prompt: "go".to_string(),
        });

        let command = backend.launch_command(&ctx);

        assert_eq!(command.argv, vec!["mock-cli".to_string()]);
        assert_eq!(backend.launches(), vec![ctx]);

        backend.clear();
        assert!(backend.launches().is_empty());
    }

    #[test]
    fn a_scripted_event_round_trips() {
        let backend = MockAgentBackend::new();
        let mut state = TranslateState::new(TranslateConfig::default());
        let event: AgentEvent = AgentEventBody::Text {
            text: "hello".to_string(),
        }
        .into();
        let line = serde_json::to_string(&event).unwrap();

        assert_eq!(backend.translate(&line, &mut state), vec![event]);
    }

    #[test]
    fn a_line_that_is_not_an_event_becomes_raw() {
        let backend = MockAgentBackend::new();
        let mut state = TranslateState::new(TranslateConfig::default());

        assert_eq!(
            backend.translate(r#"{"type":"system"}"#, &mut state),
            vec![
                AgentEventBody::Raw {
                    backend: Backend::Claude,
                    native: json!({ "type": "system" }),
                }
                .into()
            ],
        );
        assert_eq!(
            backend.translate("not json", &mut state),
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
    fn an_input_is_encoded_as_its_own_json_line() {
        let backend = MockAgentBackend::new();
        let input = SessionInput::Message {
            text: "yes".to_string(),
        };

        let encoded = backend.encode_input(&input).unwrap();

        assert!(encoded.ends_with('\n'));
        assert_eq!(
            serde_json::from_str::<SessionInput>(encoded.trim_end()).unwrap(),
            input,
        );

        assert!(
            backend
                .encode_input(&SessionInput::Message {
                    text: " \n".to_string(),
                })
                .is_err(),
        );
    }

    #[test]
    fn as_any_downcasts_a_trait_object_back_to_the_mock() {
        let backend: Arc<dyn AgentBackend> = Arc::new(MockAgentBackend::new());
        backend.launch_command(&LaunchContext::new(LaunchMode::Conversational {
            resume: Some("sess-1".to_string()),
        }));

        let mock = backend
            .as_any()
            .downcast_ref::<MockAgentBackend>()
            .expect("the trait object is the mock");

        assert_eq!(mock.launches().len(), 1);
    }
}
