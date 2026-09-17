//! The wrapper around the `git` binary, mirror and session clone operations
//! and the `GitCredentialProvider`. Git is never a crate (ADR 0011).
//!
//! Only the shape the test harness depends on exists yet: the credential
//! trait, its two value types, a [`GitError`] and a startup placeholder. The
//! git operations epic adds the command wrapper, and the secrets epic the
//! provider that reads the project-scoped `GIT_CREDENTIAL` secret (ADR 0002).
//!
//! **Async style.** `#[async_trait::async_trait]`, for the reason given in
//! [`crate::engine`]: the trait is held as `Arc<dyn GitCredentialProvider>`
//! and must stay dyn compatible.

use std::any::Any;
use std::fmt;

use async_trait::async_trait;
use axum::http::StatusCode;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::prelude::*;

pub mod lock;
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

/// Anything a git operation or a credential lookup fails with, apart from a
/// merge conflict, which is [`Error::GitConflict`] because it carries the
/// conflicting paths and answers 422.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The `git` binary exited non-zero, or could not be run. The string is
    /// git's own report; credentials never appear in it, because they live in
    /// a config file rather than in argv.
    #[error("the git command failed: {0}")]
    Command(String),
    /// No usable credential for the project.
    #[error("no git credential is available: {0}")]
    CredentialUnavailable(String),
}

impl GitError {
    /// The HTTP status this failure maps to. Both variants are internal
    /// faults; the git operations epic refines the mapping if it finds a case
    /// the caller can act on.
    pub fn status(&self) -> StatusCode {
        match self {
            GitError::Command(_) | GitError::CredentialUnavailable(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
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
