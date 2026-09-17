//! The wrapper around the `git` binary, mirror and session clone operations
//! and the `GitCredentialProvider`. Git is never a crate (ADR 0011).
//!
//! [`GitCommand`] is the single invocation point: every operation in this
//! module and every one later epics add builds one rather than spawning a
//! process of its own, so the argv, the working directory, the environment and
//! the failure mapping have exactly one definition. [`GitError`] is what they
//! all fail with.
//!
//! The credential side is still the shape the test harness depends on: the
//! trait, its two value types and a startup placeholder. The secrets epic adds
//! the provider that reads the project-scoped `GIT_CREDENTIAL` secret
//! (ADR 0002).
//!
//! **Async style.** `#[async_trait::async_trait]`, for the reason given in
//! [`crate::engine`]: the trait is held as `Arc<dyn GitCredentialProvider>`
//! and must stay dyn compatible.

use std::any::Any;
use std::fmt;

use async_trait::async_trait;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::prelude::*;

pub mod command;
pub mod error;
pub mod lock;

pub use command::{GitCommand, GitOutput};
pub use error::GitError;
pub use lock::{ProjectGitGuard, ProjectGitLocks};

#[cfg(feature = "integration-tests")]
pub mod mock;

/// The value shown instead of a credential in any formatted output (rule 3).
const REDACTED: &str = "<redacted>";

/// One credential for upstream git operations.
///
/// The wrapper turns it into `http.extraHeader = Authorization: Basic
/// <base64(username:token)>` in a temporary mode-0600 config selected through
/// `GIT_CONFIG_GLOBAL`, never into argv and never into the remote URL
/// (`ARCHITECTURE.md`, "Git model", Credentials).
#[derive(Clone, PartialEq, Eq)]
pub struct GitCredential {
    /// The basic-auth user; `x-access-token` for a GitHub PAT.
    pub username: String,
    /// The secret half. Never logged, never formatted.
    pub token: String,
}

impl fmt::Debug for GitCredential {
    /// Redacted: a credential must not reach a log through a `{:?}` on a
    /// struct that happens to contain one (rule 3).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitCredential")
            .field("username", &self.username)
            .field("token", &REDACTED)
            .finish()
    }
}

impl Drop for GitCredential {
    /// The orchestrator zeroizes its temporary credential buffers after use
    /// (`ARCHITECTURE.md`, "Secrets", Credential handling).
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

/// The name and email commits the orchestrator makes are attributed to.
///
/// `ARCHITECTURE.md`, "Git model", Commit identity: merge commits carry the
/// bot identity from `GIT_BOT_NAME` and `GIT_BOT_EMAIL`, obtained here, so a
/// future GitHub App provider can substitute the app's identity without
/// touching callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitIdentity {
    /// The committer name.
    pub name: String,
    /// The committer email.
    pub email: String,
}

/// Where every upstream git command gets its credential and its committer
/// identity (ADR 0002).
#[async_trait]
pub trait GitCredentialProvider: Send + Sync {
    /// Downcast hook, so a test can read what a handler asked for.
    fn as_any(&self) -> &dyn Any;

    /// The credential for this project's upstream, or `None` when the project
    /// has none and the remote is public.
    async fn credential(&self, project_id: Uuid) -> Result<Option<GitCredential>>;

    /// The identity for commits the orchestrator creates itself.
    fn commit_identity(&self) -> CommitIdentity;
}

/// The provider used until the git operations and secrets epics land.
///
/// It has no credential to hand out, and takes its identity from
/// `GIT_BOT_NAME` and `GIT_BOT_EMAIL` so the configured value is already the
/// one callers see.
#[derive(Debug, Clone)]
pub struct PlaceholderCredentialProvider {
    identity: CommitIdentity,
}

impl PlaceholderCredentialProvider {
    /// Build the provider around the configured bot identity.
    pub fn new(identity: CommitIdentity) -> Self {
        Self { identity }
    }
}

#[async_trait]
impl GitCredentialProvider for PlaceholderCredentialProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn credential(&self, _project_id: Uuid) -> Result<Option<GitCredential>> {
        Ok(None)
    }

    fn commit_identity(&self) -> CommitIdentity {
        self.identity.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_the_token() {
        let credential = GitCredential {
            username: "x-access-token".to_string(),
            token: "ghp_FAKE_TEST_TOKEN_0000000000".to_string(),
        };

        let rendered = format!("{credential:?}");

        assert!(rendered.contains("x-access-token"));
        assert!(!rendered.contains("ghp_FAKE_TEST_TOKEN_0000000000"));
        assert!(rendered.contains(REDACTED));
    }

    #[tokio::test]
    async fn the_placeholder_has_no_credential_and_the_configured_identity() {
        let provider = PlaceholderCredentialProvider::new(CommitIdentity {
            name: "Mars Bot".to_string(),
            email: "mars-bot@example.invalid".to_string(),
        });

        assert!(
            provider
                .credential(Uuid::new_v4())
                .await
                .expect("the placeholder never fails")
                .is_none()
        );
        assert_eq!(provider.commit_identity().name, "Mars Bot");
    }
}
