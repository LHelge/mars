//! The `AgentBackend` trait, the `claude/` adapter and native-output
//! translation into `AgentEvent`s.
//!
//! This module is the seam between the session owner and any CLI
//! (`ARCHITECTURE.md`, "Agent process model"; ADR 0003, ADR 0008). A backend
//! has exactly four responsibilities and no state of its own:
//!
//! 1. build the container command for a fresh, resumed or ephemeral launch
//!    from a [`LaunchContext`] the launcher filled from the profile and the
//!    session;
//! 2. translate one native output line into zero or more [`AgentEvent`]s,
//!    against a [`TranslateState`] that belongs to one process launch;
//! 3. encode one [`SessionInput`] into the line the CLI reads on stdin;
//! 4. name the secrets its CLI authenticates with, which the launcher resolves
//!    for every session of the backend without the profile listing them
//!    (ADR 0036).
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
use crate::models::SecretName;
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
/// (the mode and, on a resume, the `cli_session_id`). `system_prompt` is the
/// session's standing instructions already composed: Mars's session preamble
/// followed by the profile's own prompt
/// ([`crate::session::session_system_prompt`]), so a backend passes it on as
/// it is and never adds to it. Credentials are not here:
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

    /// The secret names this CLI authenticates with, most preferred first
    /// (`ARCHITECTURE.md`, "Secrets", Agent credentials; ADR 0036).
    ///
    /// The launcher resolves them for every session of this backend, treating
    /// the whole list as one slot: the row at the most specific scope wins
    /// whatever of these names it carries, and exactly one is injected. The
    /// order matters only for the tie a scope holding two of them would
    /// otherwise be, which is a row older than the one-per-scope write rule.
    ///
    /// An adapter whose image carries its own authentication returns an empty
    /// slice, which resolves nothing and warns about nothing.
    fn credential_names(&self) -> &'static [CredentialName];

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

/// Every backend there is, in the order [`credential_backend_of`] searches
/// them.
///
/// A new `agent_backend` value is added here, and `exhaustive_backend` below
/// is what makes that impossible to forget: its match stops compiling until
/// the new variant has an arm, and that arm asserts this list carries it.
pub const BACKENDS: &[Backend] = &[Backend::Claude];

/// Every credential name any backend declares, in backend order.
///
/// A flat list for a caller — the secrets service, which enforces one
/// credential per scope — that cares about the names and not about whose they
/// are (ADR 0036). Together with [`credential_backend_of`] this is the only
/// place outside the adapters that enumerates them.
pub fn all_credential_names() -> Vec<CredentialName> {
    BACKENDS
        .iter()
        .flat_map(|backend| backend_for(*backend).credential_names().to_vec())
        .collect()
}

/// Which backend, if any, authenticates with a secret called `name`.
///
/// The comparison is on the exact spelling, because that is what makes a
/// secret a credential: the CLI reads the variable by name, so a row under any
/// other name authenticates nothing (ADR 0036). A name no backend declares is
/// an ordinary secret and answers `None`.
pub fn credential_backend_of(name: &str) -> Option<Backend> {
    BACKENDS.iter().copied().find(|backend| {
        backend_for(*backend)
            .credential_names()
            .iter()
            .any(|credential| credential.as_str() == name)
    })
}

/// The backend's credential names as the launch resolver takes them
/// (`crate::secrets::resolve_for_launch`).
///
/// A name that is somehow not a valid [`SecretName`] is dropped with a log
/// line rather than failing the launch; it cannot happen, and
/// [`CredentialName::secret_name`] says why.
pub fn credential_secret_names(backend: &dyn AgentBackend) -> Vec<SecretName> {
    backend
        .credential_names()
        .iter()
        .filter_map(|name| name.secret_name())
        .collect()
}

/// The exhaustiveness guard behind [`BACKENDS`].
///
/// Only ever called with the members of that list; adding an `agent_backend`
/// value makes this match non-exhaustive, and the arm added to fix it is the
/// one that names the new backend here.
#[cfg(test)]
fn exhaustive_backend(backend: Backend) -> bool {
    match backend {
        Backend::Claude => BACKENDS.contains(&Backend::Claude),
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
    fn every_backend_is_listed() {
        for backend in BACKENDS {
            assert!(exhaustive_backend(*backend), "{backend:?}");
        }
    }

    #[test]
    fn the_claude_adapter_declares_both_of_its_credentials_in_order() {
        assert_eq!(
            backend_for(Backend::Claude).credential_names(),
            &[
                CredentialName::ClaudeCodeOauthToken,
                CredentialName::AnthropicApiKey,
            ],
        );
    }

    #[test]
    fn both_claude_credentials_are_credentials_of_the_claude_backend() {
        assert_eq!(
            credential_backend_of("CLAUDE_CODE_OAUTH_TOKEN"),
            Some(Backend::Claude)
        );
        assert_eq!(
            credential_backend_of("ANTHROPIC_API_KEY"),
            Some(Backend::Claude)
        );
    }

    #[test]
    fn an_ordinary_secret_name_is_no_backends_credential() {
        assert_eq!(credential_backend_of("DEPLOY_TOKEN"), None);
        // The spelling is the whole rule: case and shape included.
        assert_eq!(credential_backend_of("anthropic_api_key"), None);
        assert_eq!(credential_backend_of("ANTHROPIC_API_KEY_2"), None);
        assert_eq!(credential_backend_of(""), None);
    }

    #[test]
    fn the_flat_list_is_every_backends_names() {
        let all = all_credential_names();
        assert_eq!(
            all,
            vec![
                CredentialName::ClaudeCodeOauthToken,
                CredentialName::AnthropicApiKey,
            ],
        );
        for name in &all {
            assert!(credential_backend_of(name.as_str()).is_some(), "{name}");
        }
    }

    #[test]
    fn the_resolver_takes_the_declared_names_as_secret_names() {
        let names = credential_secret_names(backend_for(Backend::Claude).as_ref());
        let spellings: Vec<&str> = names.iter().map(|name| name.as_str()).collect();
        assert_eq!(
            spellings,
            vec!["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]
        );
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
