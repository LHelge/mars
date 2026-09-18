//! [`GitError`], the single failure type every git operation returns.
//!
//! `ARCHITECTURE.md`, "Orchestrator internals" (Errors): the crate-wide
//! [`Error`] carries this and delegates the HTTP status to
//! [`GitError::status`]. Nothing outside `git/` constructs these; the command
//! wrapper and the operations built on it translate git's own exit codes and
//! porcelain output into them, so no caller has to parse git (ADR 0011).
//!
//! **Secrets.** A credential never reaches a variant here. It is written into
//! a temporary mode-0600 config selected through `GIT_CONFIG_GLOBAL` and never
//! into argv (`ARCHITECTURE.md`, "Git model", Credentials), so neither the
//! argv [`GitError::Command`] carries nor the stderr git produced can contain
//! one (CLAUDE.md rule 3).

use axum::http::StatusCode;

// The crate convention (`CLAUDE.md`, "Backend conventions"); here for the doc
// links back to the crate-wide [`Error`] this widens into.
#[allow(unused_imports)]
use crate::prelude::*;

/// Anything a git operation fails with.
///
/// The variants are the distinctions a caller acts on. Everything the wrapper
/// cannot classify arrives as [`GitError::Command`] with the argv, the exit
/// code and git's own stderr, which is an internal fault: the caller gets
/// `internal error` and the detail goes to the log.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The `git` binary exited non-zero, or was killed before it could.
    ///
    /// `args` is the argv as run, without the leading `git`; `code` is `None`
    /// when a signal killed the child or the command hit its timeout.
    #[error(
        "git {} failed ({}): {stderr}",
        .args.join(" "),
        .code.map_or_else(|| "no exit code".to_string(), |code| format!("exit {code}"))
    )]
    Command {
        /// The argv as run, so a log line shows exactly what was invoked. It
        /// never holds a credential, because credentials go through the config
        /// file (`ARCHITECTURE.md`, "Git model", Credentials).
        args: Vec<String>,
        /// The process exit code, or `None` for a signal or a timeout.
        code: Option<i32>,
        /// What git wrote to stderr, lossily decoded.
        stderr: String,
    },
    /// The `git` binary could not be spawned, or a pipe broke while its output
    /// was being read.
    #[error("running the git binary failed: {0}")]
    Io(#[from] std::io::Error),
    /// A merge or rebase stopped on conflicting paths. The operation was
    /// aborted and the temporary clone deleted; nothing was written back
    /// (`ARCHITECTURE.md`, "Git model", Merge, rebase, push).
    ///
    /// This is the one variant the crate-wide [`Error`] does not carry as
    /// [`Error::Git`]: `From<GitError>` turns it into [`Error::GitConflict`],
    /// which is the 422 that puts `conflicts` in the body (`SPEC.md`, "REST
    /// API").
    #[error("the git operation stopped on {} conflicting path(s)", .paths.len())]
    Conflict {
        /// The conflicting paths, as git's porcelain output named them.
        paths: Vec<String>,
    },
    /// Upstream has advanced incompatibly, so a non-forced push was rejected.
    /// Every local ref is retained and the caller can fetch, integrate and
    /// retry (`SPEC.md`, "Git").
    #[error("the upstream branch {remote_branch} has moved on; push rejected as non-fast-forward")]
    NonFastForward {
        /// The upstream branch that was pushed to.
        remote_branch: String,
    },
    /// The name does not parse as a ref, or is the wrong kind for the
    /// operation: an upstream-tracking ref as a mutation target or push
    /// source, for instance (`SPEC.md`, "Git": 400).
    #[error("not a usable ref for this operation: {0}")]
    InvalidRef(String),
    /// The name parses, but nothing in the repository resolves to it.
    #[error("no such ref in this repository: {0}")]
    UnknownRef(String),
    /// The name resolves, but to an object that is not a commit, so it cannot
    /// be a base, a source or a merge target.
    #[error("not a commit: {0}")]
    NotACommit(String),
    /// Upstream advertises no branch at all: an empty repository, so there is
    /// nothing to seed an integration head from
    /// (`ARCHITECTURE.md`, "Git model", Project clone).
    #[error("the remote has no branches")]
    RemoteHasNoBranches,
    /// Upstream has branches but names none of them as its default: its `HEAD`
    /// is detached or points at a branch it does not have. Nothing is wrong
    /// with the remote; the project has to say which branch to use.
    ///
    /// Separate from [`GitError::RemoteHasNoBranches`] because the two are
    /// different things to do about it — push a branch upstream, or set
    /// `default_branch` — and the clone job reports them as different reasons
    /// (`SPEC.md`, "Projects": `status_message`).
    #[error("the remote does not name a default branch")]
    NoRemoteDefaultBranch,
    /// The work tree has uncommitted changes, so the operation would have
    /// discarded work: post-rebase checkout reconciliation is the case that
    /// reports this (`ARCHITECTURE.md`, "Git model", Merge, rebase, push).
    #[error("the work tree has uncommitted changes")]
    DirtyWorkTree,
    /// No usable credential for the project's upstream (ADR 0002). A state the
    /// caller fixes by storing one, not an internal fault.
    #[error("no git credential is available for this project")]
    CredentialUnavailable,
}

impl GitError {
    /// The HTTP status this failure maps to (`SPEC.md`, "REST API").
    ///
    /// The kinds the caller can act on answer 4xx with their own message;
    /// everything else is an internal fault, logged with its detail and
    /// answered `internal error`.
    pub fn status(&self) -> StatusCode {
        match self {
            // Carried as `Error::GitConflict` in practice, which is where the
            // `conflicts` array comes from; the code is the same either way.
            GitError::Conflict { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            // States the caller changes and retries: fetch and integrate,
            // commit or discard the work tree, store a credential, or give the
            // remote a branch to clone and run `retry-clone`.
            GitError::NonFastForward { .. }
            | GitError::DirtyWorkTree
            | GitError::CredentialUnavailable
            | GitError::RemoteHasNoBranches
            | GitError::NoRemoteDefaultBranch => StatusCode::CONFLICT,
            // The caller named something git cannot use; the message names it.
            GitError::InvalidRef(_) | GitError::UnknownRef(_) | GitError::NotACommit(_) => {
                StatusCode::BAD_REQUEST
            }
            GitError::Command { .. } | GitError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_variant_maps_to_its_documented_status() {
        for (error, expected) in [
            (
                GitError::Conflict {
                    paths: vec!["src/main.rs".into()],
                },
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                GitError::NonFastForward {
                    remote_branch: "main".into(),
                },
                StatusCode::CONFLICT,
            ),
            (GitError::DirtyWorkTree, StatusCode::CONFLICT),
            (GitError::CredentialUnavailable, StatusCode::CONFLICT),
            (GitError::RemoteHasNoBranches, StatusCode::CONFLICT),
            (GitError::NoRemoteDefaultBranch, StatusCode::CONFLICT),
            (
                GitError::InvalidRef("origin/main".into()),
                StatusCode::BAD_REQUEST,
            ),
            (
                GitError::UnknownRef("refs/heads/gone".into()),
                StatusCode::BAD_REQUEST,
            ),
            (
                GitError::NotACommit("v1.0.0^{tree}".into()),
                StatusCode::BAD_REQUEST,
            ),
            (
                GitError::Command {
                    args: vec!["status".into()],
                    code: Some(128),
                    stderr: "fatal: not a git repository".into(),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                GitError::Io(std::io::Error::other("broken pipe")),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ] {
            assert_eq!(error.status(), expected, "wrong status for {error}");
        }
    }

    #[test]
    fn a_failed_command_renders_its_argv_its_code_and_git_s_own_words() {
        let rendered = GitError::Command {
            args: vec!["rev-parse".into(), "--git-dir".into()],
            code: Some(128),
            stderr: "fatal: not a git repository".into(),
        }
        .to_string();

        assert!(rendered.contains("git rev-parse --git-dir"), "{rendered}");
        assert!(rendered.contains("exit 128"), "{rendered}");
        assert!(
            rendered.contains("fatal: not a git repository"),
            "{rendered}"
        );
    }

    #[test]
    fn a_command_with_no_exit_code_says_so_rather_than_inventing_one() {
        let rendered = GitError::Command {
            args: vec!["fetch".into()],
            code: None,
            stderr: "timed out".into(),
        }
        .to_string();

        assert!(rendered.contains("no exit code"), "{rendered}");
    }

    #[test]
    fn the_4xx_variants_name_what_the_caller_asked_for() {
        assert!(
            GitError::InvalidRef("origin/main".into())
                .to_string()
                .contains("origin/main")
        );
        assert!(
            GitError::UnknownRef("refs/heads/gone".into())
                .to_string()
                .contains("refs/heads/gone")
        );
        assert!(
            GitError::NotACommit("deadbeef".into())
                .to_string()
                .contains("deadbeef")
        );
        assert!(
            GitError::NonFastForward {
                remote_branch: "release/2".into(),
            }
            .to_string()
            .contains("release/2")
        );
    }
}
