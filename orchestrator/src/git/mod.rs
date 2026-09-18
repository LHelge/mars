//! The wrapper around the `git` binary, mirror and session clone operations
//! and the `GitCredentialProvider`. Git is never a crate (ADR 0011).
//!
//! [`GitCommand`] is the single invocation point: every operation in this
//! module and every one later epics add builds one rather than spawning a
//! process of its own, so the argv, the working directory, the environment and
//! the failure mapping have exactly one definition. [`GitError`] is what they
//! all fail with.
//!
//! [`credentials`] is the other half of that contract: where a command's
//! credential comes from ([`GitCredentialProvider`], ADR 0002) and how it
//! reaches the child — a temporary mode-0600 config selected through
//! `GIT_CONFIG_GLOBAL` ([`CredentialConfig`]), never argv
//! (`ARCHITECTURE.md`, "Git model", Credentials).
//!
//! [`mirror`] is the first operation built on both: creating a project's bare
//! repository, configuring it and seeding its integration heads
//! (`ARCHITECTURE.md`, "Git model", Project clone). [`paths`] is where under
//! `DATA_DIR` any of it lives (`ARCHITECTURE.md`, "Storage").

pub mod command;
pub mod credentials;
pub mod error;
pub mod lock;
pub mod mirror;
pub mod paths;
pub mod refs;

pub use command::{GitCommand, GitOutput};
pub use credentials::{
    CommitIdentity, CredentialConfig, GitActor, GitCredential, GitCredentialProvider,
    PatCredentialProvider,
};
pub use error::GitError;
pub use lock::{ProjectGitGuard, ProjectGitLocks};
pub use mirror::{InitOutcome, init_project_repo, remove_project_repo};
pub use paths::DataPaths;
pub use refs::{GitRef, RefEntry, ResolvedRef};

/// Real repositories for tests: `CLAUDE.md`, "Testing expectations" — git is
/// never mocked, so both the unit tests here and the integration tests under
/// `tests/` build fixtures with [`testutil`].
#[cfg(any(test, feature = "integration-tests"))]
pub mod testutil;

#[cfg(feature = "integration-tests")]
pub mod mock;
