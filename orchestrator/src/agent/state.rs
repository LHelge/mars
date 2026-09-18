//! The per-process translation memory a backend's `translate` is given.
//!
//! One [`TranslateState`] belongs to one process launch: the owner builds a
//! fresh one on every launch, resume and retry, and reconstructs one before it
//! starts tailing a process it adopted after a restart (`ARCHITECTURE.md`,
//! "Durability and recovery"). Nothing in it is persisted — it is what the
//! translator would otherwise have to re-derive from the whole transcript.
//!
//! What it has to carry is set by `SPEC.md`, "AgentEvent": the `init` event's
//! `resumed` flag, whether partial messages are expected at all, the
//! credential to name in a fatal authentication `error`, the hashes of the
//! inputs the owner wrote (so the CLI's echo of them is suppressed instead of
//! being stored a second time), and the subagent and denial bookkeeping the
//! `subagent_start`/`subagent_end` and `permission_denied` rules need.

use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};

use crate::models::SecretScope;
// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

/// Which credential the launcher injected for the CLI
/// (`ARCHITECTURE.md`, "Claude Code invocation", Credentials).
///
/// A name only. The value never reaches this type, and the generated `error`
/// event names the variable and its scope so the user knows which secret to
/// replace without the message ever carrying one (rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialName {
    AnthropicApiKey,
    ClaudeCodeOauthToken,
}

impl CredentialName {
    /// The environment variable's exact spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            CredentialName::AnthropicApiKey => "ANTHROPIC_API_KEY",
            CredentialName::ClaudeCodeOauthToken => "CLAUDE_CODE_OAUTH_TOKEN",
        }
    }
}

impl std::fmt::Display for CredentialName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The credential a launch injected, as the translator has to describe it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InjectedCredential {
    pub name: CredentialName,
    /// Which secret row won resolution (`ARCHITECTURE.md`, "Secrets",
    /// Resolution at launch), so the message can say where to fix it.
    pub scope: SecretScope,
}

/// What the owner knows about a launch before its first output line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TranslateConfig {
    /// True when the process was launched with `--resume`, and on adoption of
    /// a process that was.
    pub resumed: bool,
    /// The profile's `partial_messages` (`docs/data-model.md`,
    /// `agent_profiles`).
    pub partial_messages: bool,
    /// The model credential the launcher injected, when one was resolved.
    pub credential: Option<InjectedCredential>,
}

/// One process launch's translation memory.
///
/// The fields are `pub(crate)` rather than public: the Claude translator reads
/// and mutates them directly, and nothing outside the crate has any business
/// with them. The owner builds the state with [`TranslateState::new`] and
/// feeds it only through [`TranslateState::record_sent_input`].
#[derive(Debug, Clone, Default)]
pub struct TranslateState {
    pub(crate) resumed: bool,
    pub(crate) partial_messages: bool,
    pub(crate) credential: Option<InjectedCredential>,
    /// SHA-256 of every input text the owner wrote into this process.
    ///
    /// A set, not a counter, and that is accepted: two identical user messages
    /// are each recorded as their own `user_message` event by the owner, but
    /// only one hash is stored, so the CLI's echo of the first is suppressed
    /// and the echo of the second becomes a `raw` event.
    pub(crate) sent_input_hashes: HashSet<[u8; 32]>,
    /// The `tool_use_id`s of subagents that started and have not ended, so
    /// `subagent_end` is only emitted for one that was opened.
    // `allow` rather than `expect`: the unit tests below do read the field,
    // so an expectation would be unfulfilled in the test build. The Claude
    // translator tasks are what read it in earnest, and the attribute goes
    // when they do.
    #[allow(dead_code)]
    pub(crate) open_subagents: HashMap<String, ()>,
    /// Tool calls already reported as denied, so the `permission_denied`
    /// system message and the same denial in `result.permission_denials` do
    /// not produce two events (`ARCHITECTURE.md`, "Claude Code invocation").
    pub(crate) denied_tool_use_ids: HashSet<String>,
    /// Whether the fatal authentication `error` was already emitted.
    ///
    /// The CLI reports one failed credential many times — a run of `api_retry`
    /// lines and then the `result` that ends the turn — and the user has one
    /// secret to replace, so the event is emitted once per process.
    pub(crate) authentication_failed: bool,
}

impl TranslateState {
    /// A fresh state for one process launch.
    pub fn new(config: TranslateConfig) -> Self {
        Self {
            resumed: config.resumed,
            partial_messages: config.partial_messages,
            credential: config.credential,
            sent_input_hashes: HashSet::new(),
            open_subagents: HashMap::new(),
            denied_tool_use_ids: HashSet::new(),
            authentication_failed: false,
        }
    }

    /// Whether this launch resumed an earlier CLI session.
    pub fn resumed(&self) -> bool {
        self.resumed
    }

    /// Whether partial messages were asked for.
    pub fn partial_messages(&self) -> bool {
        self.partial_messages
    }

    /// The credential the launcher injected, when one was.
    pub fn credential(&self) -> Option<InjectedCredential> {
        self.credential
    }

    /// Remember that `text` was written into this process.
    ///
    /// The raw text of the message the owner wrote — for an `answer`, its
    /// `text` — never the encoded line, so the echo match does not depend on
    /// one backend's encoding.
    pub fn record_sent_input(&mut self, text: &str) {
        self.sent_input_hashes.insert(input_hash(text));
    }

    /// Whether `text` matches an input this process was sent, which is what
    /// makes the CLI's echo of it suppressible.
    pub fn was_sent_input(&self, text: &str) -> bool {
        self.sent_input_hashes.contains(&input_hash(text))
    }
}

/// The hash stored per sent input.
pub(crate) fn input_hash(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_is_carried_into_the_state() {
        let state = TranslateState::new(TranslateConfig {
            resumed: true,
            partial_messages: true,
            credential: Some(InjectedCredential {
                name: CredentialName::ClaudeCodeOauthToken,
                scope: SecretScope::Project,
            }),
        });

        assert!(state.resumed());
        assert!(state.partial_messages());
        assert_eq!(
            state.credential(),
            Some(InjectedCredential {
                name: CredentialName::ClaudeCodeOauthToken,
                scope: SecretScope::Project,
            }),
        );
        assert!(state.sent_input_hashes.is_empty());
        assert!(state.open_subagents.is_empty());
        assert!(state.denied_tool_use_ids.is_empty());
        assert!(!state.authentication_failed);
    }

    #[test]
    fn a_default_config_is_a_fresh_unauthenticated_launch() {
        let state = TranslateState::new(TranslateConfig::default());
        assert!(!state.resumed());
        assert!(!state.partial_messages());
        assert_eq!(state.credential(), None);
    }

    #[test]
    fn record_sent_input_stores_the_sha256_of_the_text() {
        let mut state = TranslateState::new(TranslateConfig::default());
        state.record_sent_input("hello");

        // The published SHA-256 of "hello".
        let expected: [u8; 32] = [
            0x2c, 0xf2, 0x4d, 0xba, 0x5f, 0xb0, 0xa3, 0x0e, 0x26, 0xe8, 0x3b, 0x2a, 0xc5, 0xb9,
            0xe2, 0x9e, 0x1b, 0x16, 0x1e, 0x5c, 0x1f, 0xa7, 0x42, 0x5e, 0x73, 0x04, 0x33, 0x62,
            0x93, 0x8b, 0x98, 0x24,
        ];
        assert!(state.sent_input_hashes.contains(&expected));
        assert!(state.was_sent_input("hello"));
        assert!(!state.was_sent_input("hello "));
    }

    #[test]
    fn the_same_input_twice_stores_one_hash() {
        let mut state = TranslateState::new(TranslateConfig::default());
        state.record_sent_input("again");
        state.record_sent_input("again");
        assert_eq!(state.sent_input_hashes.len(), 1);
    }

    #[test]
    fn a_credential_name_displays_as_the_variable() {
        assert_eq!(
            CredentialName::AnthropicApiKey.to_string(),
            "ANTHROPIC_API_KEY"
        );
        assert_eq!(
            CredentialName::ClaudeCodeOauthToken.to_string(),
            "CLAUDE_CODE_OAUTH_TOKEN",
        );
    }
}
