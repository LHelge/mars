//! The wrapper around the `git` binary, mirror and session clone operations
//! and the `GitCredentialProvider`. Git is never a crate (ADR 0011).
//!
//! [`GitCommand`] is the single invocation point: every operation in this
//! module and every one later epics add builds one rather than spawning a
//! process of its own, so the argv, the working directory, the environment and
//! the failure mapping have exactly one definition. [`GitError`] is what they
//! all fail with.
//!
//! **`--end-of-options`, and the two commands that cannot take it.** Every
//! invocation here puts `--end-of-options` after its flags so that a revision,
//! a remote name or a refspec can never be read as an option, however it was
//! validated. `git checkout` and `git reset` are the exceptions, because git
//! 2.39 — the minimum this crate supports, since the orchestrator is also run
//! directly on a host with whatever git the distribution ships — does not
//! support the option on either: `checkout` treats `--end-of-options` and
//! everything after it as pathspecs, so `checkout -b <branch>
//! --end-of-options <commit>` dies with "Cannot update paths and switch to
//! branch … at the same time", and `reset` rejects it outright with "option
//! '--end-of-options' must come before non-option arguments". Both take the
//! equivalent protection instead: the revision first and a trailing `--`, so
//! git reads the argument before it as a revision and the empty list after it
//! as the pathspec. A trailing `--` does not stop a leading `-` from being
//! read as an option, so those four call sites pass only a revision this
//! crate already knows is a full object id (validated by [`refs`]) or a ref it
//! wrote itself. Newer git accepts `--end-of-options` on both forms, so this
//! is one argv that works everywhere rather than a version fork
//! (`ARCHITECTURE.md`, "Git model", Supported git).
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
//!
//! [`diff`] is the read-only pair built on the same plumbing: the diff between
//! two fixed commits behind `GET /projects/{pid}/git/diff`, and the session
//! branch listing behind `GET /projects/{pid}/git/session-branches` and the
//! `list_session_branches` MCP tool (`ARCHITECTURE.md`, "Git model", Diff;
//! `SPEC.md`, "Git"). `diff::diff` is reached through its module rather than
//! re-exported here, so that the name of the operation and the name of the
//! module cannot be mistaken for each other at a call site.
//!
//! [`history`] is the third read-only query: an integration head's
//! first-parent history and the commit ranges it is attributed to tasks over,
//! behind `GET /projects/{pid}/git/history` (`ARCHITECTURE.md`, "Git model",
//! History).
//!
//! [`service`] is what the rest of the orchestrator calls. [`GitService`]
//! composes the primitives above into the operations the REST routes, the MCP
//! tools and the session endpoints expose: it takes the project git lock once,
//! syncs the participating sessions, resolves every ref to a fixed commit,
//! obtains the credential and the commit identity for the [`GitActor`] that
//! asked, runs the primitive and records the outcome as a `git` event on the
//! sessions that took part (ADR 0007; `ARCHITECTURE.md`, "Git model";
//! `SPEC.md`, "AgentEvent"). [`HandoffVerifier`] is the one thing it does not
//! decide for itself: whether a task's hand-off is current and approved, which
//! "Code hand-offs and review" answers under the lock the task merge holds.

pub mod command;
pub mod credentials;
pub mod diff;
pub mod error;
pub mod history;
pub mod integrate;
pub mod lock;
pub mod mirror;
pub mod paths;
pub mod push;
pub mod refs;
pub mod service;
pub mod session;
pub mod tempclone;

pub use command::{GitCommand, GitOutput};
pub use credentials::{
    CommitIdentity, CredentialConfig, GitActor, GitCredential, GitCredentialProvider,
    PatCredentialProvider,
};
pub use diff::{MAX_PATCH_BYTES, session_branches};
pub use error::GitError;
pub use integrate::{
    MergeOutcome, RebaseOutcome, WorkTreeOutcome, merge, rebase, requested_by, requested_by_trailer,
};
pub use lock::{ProjectGitGuard, ProjectGitLocks};
pub use mirror::{
    FetchOutcome, InitOutcome, fetch_project, fetch_upstream, init_project_repo, list_branches,
    remove_project_repo, set_default_branch, verify_default_branch,
};
pub use paths::DataPaths;
pub use push::{ComparePage, PushOutcome, github_compare_url, push};
pub use refs::{GitRef, RefEntry, ResolvedRef};
pub use service::{
    ApprovedHandoff, DiffSelector, GitService, HandoffVerifier, NoHandoffs, PinnedSource,
};
pub use session::{
    FetchedBack, create_work_clone, fetch_back, fetch_back_ended, remove_work_clone, resolve_base,
    session_branch,
};
pub use tempclone::TempClone;

/// Real repositories for tests: `CLAUDE.md`, "Testing expectations" — git is
/// never mocked, so both the unit tests here and the integration tests under
/// `tests/` build fixtures with [`testutil`].
#[cfg(any(test, feature = "integration-tests"))]
pub mod testutil;

#[cfg(feature = "integration-tests")]
pub mod mock;
