//! The `AgentBackend` trait, the `claude/` adapter and native-output
//! translation into `AgentEvent`s.
//!
//! This module is the seam between the session owner and any CLI
//! (`ARCHITECTURE.md`, "Agent process model"; ADR 0003, ADR 0008). A backend
//! has exactly three responsibilities and no state of its own:
//!
//! 1. build the container command for a fresh, resumed or ephemeral launch
//!    from a [`LaunchContext`] the launcher filled from the profile and the
//!    session;
//! 2. translate one native output line into zero or more [`AgentEvent`]s,
//!    against a [`TranslateState`] that belongs to one process launch;
//! 3. encode one [`SessionInput`] into the line the CLI reads on stdin.
//!
//! Everything that varies per launch lives in [`LaunchContext`] and everything
//! that varies per process lives in [`TranslateState`], so the implementations
//! are plain values behind an `Arc` and can be shared by every session of a
//! backend. Nothing here parses a native shape: the per-line rules belong to
//! the adapter that owns the CLI, in [`claude`].

use std::any::Any;

use crate::events::{AgentEvent, SessionInput};
// The model enum and this module's trait share the name `AgentBackend`
// (`docs/data-model.md`, "Enums", `agent_backend`). The trait keeps the name
// the documentation uses and the enum is referred to as `Backend` inside
// `agent/`; neither is renamed.
use crate::models::AgentBackend as Backend;
use crate::prelude::*;

pub mod claude;
pub mod state;

#[cfg(feature = "integration-tests")]
pub mod mock;

pub use claude::{CLAUDE_CLI_VERSION, ClaudeBackend};
pub use state::{CredentialName, InjectedCredential, TranslateConfig, TranslateState};

#[cfg(feature = "integration-tests")]
pub use mock::MockAgentBackend;

/// Where the launcher writes the MCP configuration inside every session
/// container (`ARCHITECTURE.md`, "Claude Code invocation"; `SPEC.md`, "Shared
/// directories").
///
/// One path for every session in v1, which is why [`LaunchContext`] still
/// carries it as a field rather than hard-coding it in the adapter: the
/// launcher owns the file, the backend only names it on the command line.
/// `models::MCP_CONFIG_PATH` is the same path as the segments the shared
/// directory validation compares against.
pub const DEFAULT_MCP_CONFIG_PATH: &str = "/session/mcp.json";

/// The container `Cmd` the image entrypoint execs
/// (`ARCHITECTURE.md`, "Session image").
///
/// Argv only: the environment, working directory, user and mounts are the
/// launcher's, not the backend's. The first element is the CLI binary name as
/// it is found on the image's `PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub argv: Vec<String>,
}

/// What a launch is for, with the invalid combinations unrepresentable.
///
/// A conversational launch reads stdin and may resume an earlier CLI session;
/// an ephemeral launch carries its whole prompt on the command line and never
/// resumes (`ARCHITECTURE.md`, "Claude Code invocation"). Splitting the two
/// means "ephemeral with a resume id" and "conversational with an inline
/// prompt" cannot be constructed, so no implementation has to reject them.
///
/// Deliberately not `Serialize`/`Deserialize`: it is constructed per launch
/// and never crosses a wire or a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchMode {
    /// A long-lived process fed over stdin. `resume` is the `cli_session_id`
    /// of the process being continued (`docs/data-model.md`, `sessions`).
    Conversational { resume: Option<String> },
    /// One prompt, one run, no input afterwards.
    Ephemeral { prompt: String },
}

/// Everything a backend needs to build its launch command.
///
/// The launcher fills it from the agent profile (`model`, `system_prompt`,
/// `partial_messages`; `docs/data-model.md`, `agent_profiles`) and the session
/// (the mode and, on a resume, the `cli_session_id`). Credentials are not here:
/// they are injected into the container's environment like any other secret
/// and never reach a command line (rule 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchContext {
    pub mode: LaunchMode,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub partial_messages: bool,
    pub mcp_config_path: String,
}

impl LaunchContext {
    /// A context for `mode` with no model or system prompt, partial messages
    /// off and the v1 MCP configuration path.
    pub fn new(mode: LaunchMode) -> Self {
        Self {
            mode,
            model: None,
            system_prompt: None,
            partial_messages: false,
            mcp_config_path: DEFAULT_MCP_CONFIG_PATH.to_string(),
        }
    }

    /// The `cli_session_id` this launch resumes, if any.
    pub fn resume(&self) -> Option<&str> {
        match &self.mode {
            LaunchMode::Conversational { resume } => resume.as_deref(),
            LaunchMode::Ephemeral { .. } => None,
        }
    }
}

/// One CLI adapter (`ARCHITECTURE.md`, "Agent process model").
///
/// Object-safe on purpose: the owner holds an `Arc<dyn AgentBackend>` chosen
/// by [`backend_for`] from the profile's backend, so adding a second CLI adds
/// an implementation and an enum value and changes nothing else.
pub trait AgentBackend: Send + Sync {
    /// The container command for a fresh, resumed or ephemeral launch.
    fn launch_command(&self, ctx: &LaunchContext) -> Command;

    /// Translate one native output line into the events it produces.
    ///
    /// Zero events is a valid answer (a line that only advances the state),
    /// and so is more than one. A line the adapter has no rule for becomes a
    /// single `raw` event rather than being dropped (`SPEC.md`, "AgentEvent").
    fn translate(&self, line: &str, state: &mut TranslateState) -> Vec<AgentEvent>;

    /// Encode one input as the single newline-terminated line the CLI reads.
    ///
    /// The returned string carries exactly one newline, at its end: whatever
    /// the text contains is escaped into the line. An input whose text is
    /// empty or only whitespace is rejected with [`Error::BadRequest`] rather
    /// than written.
    fn encode_input(&self, input: &SessionInput) -> Result<String>;

    /// Downcast hook, so a test that injected a concrete backend can read back
    /// what the owner did with it (`CLAUDE.md`, "Testing expectations").
    fn as_any(&self) -> &dyn Any;
}

/// The adapter for a profile's backend.
///
/// The match is exhaustive, so a new `agent_backend` enum value fails to
/// compile here until an adapter exists for it.
pub fn backend_for(backend: Backend) -> Arc<dyn AgentBackend> {
    match backend {
        Backend::Claude => Arc::new(ClaudeBackend::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_for_claude_is_the_claude_adapter() {
        let backend = backend_for(Backend::Claude);
        assert!(backend.as_any().downcast_ref::<ClaudeBackend>().is_some());
    }

    #[test]
    fn a_context_defaults_to_the_v1_mcp_config_path() {
        let ctx = LaunchContext::new(LaunchMode::Conversational { resume: None });
        assert_eq!(ctx.mcp_config_path, "/session/mcp.json");
        assert_eq!(ctx.model, None);
        assert_eq!(ctx.system_prompt, None);
        assert!(!ctx.partial_messages);
        assert_eq!(ctx.resume(), None);
    }

    #[test]
    fn only_a_conversational_launch_resumes() {
        let resumed = LaunchContext::new(LaunchMode::Conversational {
            resume: Some("sess-1".to_string()),
        });
        assert_eq!(resumed.resume(), Some("sess-1"));

        let ephemeral = LaunchContext::new(LaunchMode::Ephemeral {
            prompt: "do the thing".to_string(),
        });
        assert_eq!(ephemeral.resume(), None);
    }
}
