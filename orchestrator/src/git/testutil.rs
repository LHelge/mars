//! Real git repositories for tests.
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"): everything that
//! touches a repository is tested against a real one in a `tempfile`
//! directory. This module builds those repositories so that the unit tests in
//! `src/` and the integration tests under `tests/` — mirror initialization,
//! session clone, merge, rebase, push, diff and the git routes — share one
//! definition of "an upstream with some history in it" instead of each writing
//! its own.
//!
//! It is compiled under `cfg(test)` and behind the `integration-tests`
//! feature, and it is the one place in the crate that may `expect`: a helper
//! that cannot build its fixture has nothing useful to return, and a panic
//! naming the failed git command is what a test author needs to see.
//!
//! The identity below is deliberately fake (`CLAUDE.md` rule 3); `.invalid` is
//! the reserved TLD that can never resolve.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{CommitIdentity, GitCommand};
// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

/// The name every fixture commit is authored and committed by.
pub const TEST_AUTHOR_NAME: &str = "Test User";
/// The address every fixture commit carries. `.invalid` never resolves.
pub const TEST_AUTHOR_EMAIL: &str = "test@example.invalid";

/// The fixed identity fixture commits are made under.
pub fn test_identity() -> CommitIdentity {
    CommitIdentity {
        name: TEST_AUTHOR_NAME.to_string(),
        email: TEST_AUTHOR_EMAIL.to_string(),
    }
}

/// Run `git <args>` in `cwd` and return its standard output, panicking with
/// the failure if it did not exit 0.
///
/// Through [`GitCommand`] like everything else, so the fixture repositories
/// are built in the same cleared environment the orchestrator uses and a test
/// cannot accidentally depend on the developer's own git configuration.
pub async fn run_git(cwd: &Path, args: &[&str]) -> String {
    GitCommand::new()
        .args(args)
        .cwd(cwd)
        .identity(&test_identity())
        .run_ok()
        .await
        .unwrap_or_else(|err| panic!("git {} failed in {}: {err}", args.join(" "), cwd.display()))
        .stdout
}

/// A bare repository standing in for a project's upstream remote, with a
/// throwaway work clone beside it that the helpers commit through.
///
/// Both live in `dir`, which removes them when the value is dropped, so a test
/// holds the [`TestUpstream`] for as long as it needs the repository.
pub struct TestUpstream {
    /// The temporary directory holding `upstream.git` and `work`.
    pub dir: TempDir,
    /// The bare repository: what a project's `remote_url` would point at.
    pub path: PathBuf,
}

impl TestUpstream {
    /// A bare repository with a `main` branch holding two commits.
    pub async fn create() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let root = dir.path().to_path_buf();

        run_git(
            &root,
            &[
                "init",
                "--bare",
                "--quiet",
                "--initial-branch=main",
                "upstream.git",
            ],
        )
        .await;
        run_git(&root, &["clone", "--quiet", "upstream.git", "work"]).await;
        // A clone of an empty repository leaves HEAD on whatever the local
        // `init.defaultBranch` says; name it explicitly so the first commit
        // lands on `main`.
        run_git(
            &root.join("work"),
            &["symbolic-ref", "HEAD", "refs/heads/main"],
        )
        .await;

        let upstream = Self {
            path: root.join("upstream.git"),
            dir,
        };

        upstream
            .commit_file("main", "README.md", "# fixture\n", "chore: add a readme")
            .await;
        upstream
            .commit_file(
                "main",
                "src/lib.rs",
                "pub fn answer() -> u32 {\n    42\n}\n",
                "feat: add the answer",
            )
            .await;

        upstream
    }

    /// The throwaway non-bare clone the commits are made in.
    fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    /// Write `content` to `path` on `branch`, commit it and push it to the
    /// upstream. Returns the new commit's full object id.
    ///
    /// The branch is created if the upstream does not have it yet: off the
    /// current HEAD when there is one, as an unborn branch otherwise.
    pub async fn commit_file(
        &self,
        branch: &str,
        path: &str,
        content: &str,
        message: &str,
    ) -> String {
        let work = self.work();

        let known_upstream = !run_git(&work, &["ls-remote", "--heads", "origin", branch])
            .await
            .trim()
            .is_empty();

        if known_upstream {
            let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");
            run_git(&work, &["fetch", "--quiet", "origin", &refspec]).await;
            let start = format!("refs/remotes/origin/{branch}");
            run_git(&work, &["checkout", "--quiet", "-B", branch, &start]).await;
        } else if head_is_born(&work).await {
            run_git(&work, &["checkout", "--quiet", "-B", branch]).await;
        } else {
            let full = format!("refs/heads/{branch}");
            run_git(&work, &["symbolic-ref", "HEAD", &full]).await;
        }

        let file = work.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).expect("the parent directory is created");
        }
        std::fs::write(&file, content).expect("the fixture file is written");

        run_git(&work, &["add", "--", path]).await;
        run_git(&work, &["commit", "--quiet", "-m", message]).await;
        let push_spec = format!("HEAD:refs/heads/{branch}");
        run_git(&work, &["push", "--quiet", "origin", &push_spec]).await;

        run_git(&work, &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_string()
    }

    /// Create a tag in the upstream at `target`, annotated or lightweight.
    ///
    /// The distinction matters to the ref plumbing: an annotated tag's ref
    /// points at a tag object, which has to be peeled to reach a commit.
    pub async fn tag(&self, name: &str, target: &str, annotated: bool) {
        if annotated {
            let message = format!("release {name}");
            run_git(
                &self.path,
                &["tag", "--annotate", "--message", &message, name, target],
            )
            .await;
        } else {
            run_git(&self.path, &["tag", name, target]).await;
        }
    }
}

impl TestUpstream {
    /// Delete `branch` from the upstream.
    ///
    /// What a fetch with `--prune` has to notice: the upstream-tracking ref
    /// goes, Mars's integration head of the same name stays
    /// (`ARCHITECTURE.md`, "Git model", Ref ownership).
    pub async fn delete_branch(&self, branch: &str) {
        let full = format!("refs/heads/{branch}");
        run_git(&self.path, &["update-ref", "-d", "--end-of-options", &full]).await;
    }

    /// Delete `tag` from the upstream, for the same reason.
    pub async fn delete_tag(&self, tag: &str) {
        let full = format!("refs/tags/{tag}");
        run_git(&self.path, &["update-ref", "-d", "--end-of-options", &full]).await;
    }
}

/// Does `work` have a commit on HEAD yet?
///
/// A non-zero exit is the answer, so this goes through [`GitCommand::run`]
/// rather than [`run_git`], which would panic on an unborn branch.
async fn head_is_born(work: &Path) -> bool {
    GitCommand::new()
        .args(["rev-parse", "--verify", "HEAD"])
        .cwd(work)
        .run()
        .await
        .is_ok_and(|output| output.status == 0)
}
