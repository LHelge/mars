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
//! repository, configuring it and seeding its integration heads, the recurring
//! `git fetch --prune origin` that the cron job, `POST /projects/{id}/fetch`
//! and a fresh launch share, and the branch listing behind
//! `GET /projects/{id}/branches` (`ARCHITECTURE.md`, "Git model", Project
//! clone; `SPEC.md`, "Projects"). [`paths`] is where under `DATA_DIR` any of
//! it lives (`ARCHITECTURE.md`, "Storage").

pub mod command;
pub mod credentials;
pub mod error;
pub mod lock;
pub mod mirror;
pub mod paths;
pub mod refs;
pub mod session;

pub use command::{GitCommand, GitOutput};
pub use credentials::{
    CommitIdentity, CredentialConfig, GitActor, GitCredential, GitCredentialProvider,
    PatCredentialProvider,
};
pub use error::GitError;
pub use lock::{ProjectGitGuard, ProjectGitLocks};
pub use mirror::{
    FetchOutcome, InitOutcome, fetch_project, fetch_upstream, init_project_repo, list_branches,
    remove_project_repo,
};
pub use paths::DataPaths;
pub use refs::{GitRef, RefEntry, ResolvedRef};
pub use session::{create_work_clone, fetch_back, remove_work_clone, resolve_base, session_branch};

/// Real repositories for tests: `CLAUDE.md`, "Testing expectations" — git is
/// never mocked, so both the unit tests here and the integration tests under
/// `tests/` build fixtures with [`testutil`].
#[cfg(any(test, feature = "integration-tests"))]
pub mod testutil;

#[cfg(feature = "integration-tests")]
pub mod mock;
