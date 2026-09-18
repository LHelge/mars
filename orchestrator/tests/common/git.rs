//! Real bare repositories for the integration tests (`CLAUDE.md`, "Testing
//! expectations": git is never mocked).
//!
//! [`BareFixture`] is the upstream a lifecycle test points a project at: a
//! `tempfile` directory holding `origin.git`, initialised bare on `main` with
//! one commit made through a throwaway work clone. Beyond building history it
//! can make the remote *disappear* and come back, which is what a test of the
//! `error` → `retry-clone` → `ready` path needs and what
//! `mars_orchestrator::git::testutil::TestUpstream` does not offer.
//!
//! Everything goes through the `git` binary with argv arrays and never a
//! shell, and every command carries the fixture identity and an emptied
//! configuration lookup, so a test can never pick up the developer's own git
//! configuration. The calls are synchronous [`std::process::Command`]s: a
//! fixture is arrangement, not the thing under test, and a plain `new()` is
//! easier to call from a test body than an awaited one.
//!
//! This module may `expect`: a fixture that cannot build its repository has
//! nothing to return, and a panic naming the failed git command is what a test
//! author needs to see. Every identity here is obviously fake (rule 3);
//! `.invalid` is the reserved TLD that can never resolve.

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// The name every fixture commit is authored and committed by.
pub const FIXTURE_AUTHOR_NAME: &str = "Mars Test Fixture";
/// The address every fixture commit carries. `.invalid` never resolves.
pub const FIXTURE_AUTHOR_EMAIL: &str = "fixture@example.invalid";

/// The bare repository's directory name inside the fixture's temporary
/// directory.
const BARE: &str = "origin.git";

/// The throwaway non-bare clone commits are made in, beside the bare one.
const WORK: &str = "work";

/// The branch a fresh fixture is born on.
const INITIAL_BRANCH: &str = "main";

/// A bare repository standing in for a project's upstream remote.
///
/// The temporary directory is owned by the value: dropping a [`BareFixture`]
/// removes the repository, so a test holds it for as long as the project under
/// test needs its remote. Parallel tests each get a directory of their own, so
/// two fixtures never share a path.
pub struct BareFixture {
    /// The temporary directory holding `origin.git`.
    dir: TempDir,
    /// `origin.git` itself: what a project's `remote_url` points at.
    path: PathBuf,
}

impl BareFixture {
    /// A bare repository on `main` with one commit and `HEAD` naming `main`.
    pub fn new() -> BareFixture {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join(BARE);
        let fixture = BareFixture { dir, path };

        fixture.init();
        fixture
    }

    /// The `file://` URL of the bare repository, absolute because the
    /// temporary directory is.
    ///
    /// `file://` is the remote form `RemoteUrl::parse` accepts only under the
    /// `integration-tests` feature, which is the feature these tests run
    /// under.
    pub fn url(&self) -> String {
        format!("file://{}", self.path.display())
    }

    /// The bare repository's path on disk.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Branch `name` off `main`, with one commit of its own.
    ///
    /// A branch with no commit of its own would share `main`'s tip, which
    /// makes "the head moved" assertions unreadable.
    pub fn add_branch(&self, name: &str) -> String {
        self.add_commit(name, &format!("{}.txt", name.replace('/', "-")))
    }

    /// Commit `filename` on `branch` and push it, returning the new commit's
    /// full object id.
    ///
    /// The branch is created off the current `HEAD` if the repository does not
    /// have it yet, which is what [`BareFixture::add_branch`] relies on.
    pub fn add_commit(&self, branch: &str, filename: &str) -> String {
        let work = self.fresh_work_clone();

        let tracking = format!("refs/remotes/origin/{branch}");
        if self.git_succeeds(&work, &["rev-parse", "--verify", "--quiet", &tracking]) {
            self.git(&work, &["checkout", "--quiet", "-B", branch, &tracking]);
        } else if self.git_succeeds(&work, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
            self.git(&work, &["checkout", "--quiet", "-B", branch]);
        } else {
            // An unborn HEAD cannot be checked out, only renamed.
            let full = format!("refs/heads/{branch}");
            self.git(&work, &["symbolic-ref", "HEAD", &full]);
        }

        let file = work.join(filename);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).expect("the parent directory is created");
        }
        std::fs::write(&file, format!("{filename} on {branch}\n"))
            .expect("the fixture file is written");

        self.git(&work, &["add", "--", filename]);
        let message = format!("feat: add {filename} on {branch}");
        self.git(&work, &["commit", "--quiet", "-m", &message]);
        let refspec = format!("HEAD:refs/heads/{branch}");
        self.git(&work, &["push", "--quiet", "origin", &refspec]);

        let commit = self.git(&work, &["rev-parse", "HEAD"]).trim().to_string();
        std::fs::remove_dir_all(&work).expect("the throwaway work clone is removed");

        commit
    }

    /// The object id `rev` names in the bare repository.
    pub fn commit_of(&self, rev: &str) -> String {
        self.git(&self.path, &["rev-parse", rev]).trim().to_string()
    }

    /// Delete the bare repository, so the remote is unreachable.
    ///
    /// The mistyped or not-yet-created remote: a clone against this URL fails
    /// at once rather than hanging on a network timeout, which is what makes
    /// the `error` path cheap to test.
    pub fn remove(&self) {
        std::fs::remove_dir_all(&self.path).expect("the bare repository is removed");
    }

    /// Build the repository again at the same path, as
    /// [`BareFixture::new`] first did.
    ///
    /// The remote the operator finally created: the same URL now answers, so a
    /// retry of the failed clone succeeds.
    pub fn recreate(&self) {
        assert!(
            !self.path.exists(),
            "the fixture is still there; call remove() first"
        );
        self.init();
    }

    /// `git init --bare` plus the first commit on `main`.
    fn init(&self) {
        let root = self.dir.path();

        self.git(
            root,
            &["init", "--bare", "--quiet", "--initial-branch=main", BARE],
        );
        // `--initial-branch` already points the bare `HEAD` at `main`; naming
        // it again keeps the fixture true to its documentation whatever the
        // git version decides to default to.
        let full = format!("refs/heads/{INITIAL_BRANCH}");
        self.git(&self.path, &["symbolic-ref", "HEAD", &full]);

        self.add_commit(INITIAL_BRANCH, "README.md");
    }

    /// A clone of the bare repository at `work`, replacing any earlier one.
    fn fresh_work_clone(&self) -> PathBuf {
        let root = self.dir.path();
        let work = root.join(WORK);

        if work.exists() {
            std::fs::remove_dir_all(&work).expect("the previous work clone is removed");
        }
        self.git(root, &["clone", "--quiet", BARE, WORK]);
        // A clone of an empty repository leaves `HEAD` on whatever the local
        // `init.defaultBranch` says; name it so the first commit lands on
        // `main`.
        let full = format!("refs/heads/{INITIAL_BRANCH}");
        if !self.git_succeeds(&work, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
            self.git(&work, &["symbolic-ref", "HEAD", &full]);
        }

        work
    }

    /// Run `git <args>` in `cwd`, panicking with the failure if it did not
    /// exit 0, and return its standard output.
    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let output = self
            .command(cwd, args)
            .output()
            .unwrap_or_else(|err| panic!("git {} could not start: {err}", args.join(" ")));

        assert!(
            output.status.success(),
            "git {} failed in {}: {}",
            args.join(" "),
            cwd.display(),
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8(output.stdout).expect("git wrote utf-8")
    }

    /// Did `git <args>` exit 0? The question form of [`BareFixture::git`], for
    /// the `rev-parse --verify` probes, where a non-zero exit is the answer
    /// rather than a failure.
    fn git_succeeds(&self, cwd: &Path, args: &[&str]) -> bool {
        self.command(cwd, args)
            .output()
            .is_ok_and(|output| output.status.success())
    }

    /// `git` with the fixture identity and an emptied configuration lookup.
    fn command(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut command = Command::new("git");

        command
            .current_dir(cwd)
            // The identity per command rather than per repository, so a
            // repository built here carries no configuration of its own.
            .args(["-c", &format!("user.name={FIXTURE_AUTHOR_NAME}")])
            .args(["-c", &format!("user.email={FIXTURE_AUTHOR_EMAIL}")])
            .args(["-c", "commit.gpgsign=false"])
            .args(["-c", "protocol.file.allow=always"])
            .args(args)
            // No system, global or per-user configuration, and no prompt: a
            // fixture must build the same way on every machine, and a git that
            // asks for a password would hang the suite.
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");

        command
    }
}

impl Default for BareFixture {
    fn default() -> Self {
        Self::new()
    }
}
