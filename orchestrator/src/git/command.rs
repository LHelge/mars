//! [`GitCommand`], the single wrapper every git operation goes through.
//!
//! Git is never a crate: every operation is an invocation of the `git` binary
//! with an explicit argv, working directory and environment, and parsing is
//! limited to porcelain formats (ADR 0011). Nothing in the orchestrator builds
//! a shell string, so no path, ref or message can ever be interpreted as
//! syntax.
//!
//! **Environment.** The child starts from nothing (`env_clear`) and gets an
//! allow-list back: `PATH` and `HOME` so git can be found and behave, plus the
//! four settings that make it non-interactive and machine-readable, plus
//! `GIT_CONFIG_GLOBAL`, which is either the caller's temporary credential
//! config or `/dev/null`. `GIT_DIR` and `GIT_WORK_TREE` are therefore never
//! inherited: the repository a command acts on is the one named by
//! [`GitCommand::cwd`] or by the argv, never one the orchestrator's own
//! environment happened to point at.
//!
//! **Secrets.** A credential is never an argument. It goes into a temporary
//! mode-0600 config selected through `GIT_CONFIG_GLOBAL`
//! (`ARCHITECTURE.md`, "Git model", Credentials), so the argv recorded in
//! [`GitError::Command`] and the `debug` line below are safe to log
//! (CLAUDE.md rule 3). Environment *values* are never logged, only the argv
//! and the working directory.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::{CommitIdentity, GitError};
use crate::prelude::*;

/// The binary, resolved through `PATH`. `main.rs` probes it at startup, and
/// the orchestrator image pins its version.
const GIT_BINARY: &str = "git";

/// The `GIT_CONFIG_GLOBAL` for a command with no credential config of its own.
///
/// Not "unset": an unset `GIT_CONFIG_GLOBAL` makes git read `~/.gitconfig`,
/// which on a developer machine is somebody's real configuration.
const NO_GLOBAL_CONFIG: &str = "/dev/null";

/// How long a command may run before it is killed.
///
/// Ten minutes is sized for the network operations — the first fetch of a
/// large upstream — not for the local ones. It exists so a hung remote cannot
/// pin a project git lock forever (`ARCHITECTURE.md`, "Git model",
/// Serialization).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// The status [`GitOutput`] reports when the child never got to exit: killed
/// by a signal, or stopped at the timeout. [`GitCommand::run_ok`] keeps the
/// distinction properly, as `code: None`.
const NO_EXIT_CODE: i32 = -1;

/// What a finished `git` invocation produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOutput {
    /// The exit code, or [`NO_EXIT_CODE`] when a signal or the timeout ended
    /// the child.
    pub status: i32,
    /// Standard output, lossily decoded: git emits path names in the
    /// filesystem's encoding, which is not always UTF-8.
    pub stdout: String,
    /// Standard error, lossily decoded.
    pub stderr: String,
}

/// One `git` invocation, built up and then run.
///
/// Consuming builder methods, so a command reads as one expression and cannot
/// be run twice by accident:
///
/// ```no_run
/// # use mars_orchestrator::git::GitCommand;
/// # async fn example() -> Result<(), mars_orchestrator::git::GitError> {
/// let head = GitCommand::new()
///     .args(["rev-parse", "HEAD"])
///     .cwd("/data/projects/p/repo.git")
///     .run_ok()
///     .await?;
/// # let _ = head;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct GitCommand {
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    config_global: Option<PathBuf>,
    timeout: Duration,
}

impl Default for GitCommand {
    fn default() -> Self {
        Self {
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            config_global: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl GitCommand {
    /// An empty command with the default timeout and no extra environment.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one argument. It is passed to the child verbatim; there is no
    /// shell, so nothing in it is ever interpreted.
    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    /// Append several arguments, in order.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|arg| arg.as_ref().to_os_string()));
        self
    }

    /// Run in this directory.
    ///
    /// The child's working directory rather than a `-C` argument: the effect
    /// is the same and the repository stays out of argv, so a log line stays
    /// readable when the path is long.
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Read this file as the global config instead of `/dev/null`.
    ///
    /// The credential path: the caller writes `http.extraHeader` into a
    /// temporary mode-0600 file and points the child at it here, so the header
    /// never appears in argv where `ps` would show it (`ARCHITECTURE.md`, "Git
    /// model", Credentials).
    pub fn config_global(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_global = Some(path.into());
        self
    }

    /// Attribute anything this command commits to `identity`.
    ///
    /// Through `GIT_AUTHOR_*`/`GIT_COMMITTER_*` rather than `user.name` in a
    /// config, so one temporary clone can make commits under different
    /// identities without rewriting a file between them
    /// (`ARCHITECTURE.md`, "Git model", Commit identity).
    pub fn identity(mut self, identity: &CommitIdentity) -> Self {
        for (key, value) in [
            ("GIT_AUTHOR_NAME", &identity.name),
            ("GIT_AUTHOR_EMAIL", &identity.email),
            ("GIT_COMMITTER_NAME", &identity.name),
            ("GIT_COMMITTER_EMAIL", &identity.email),
        ] {
            self.env.push((key.into(), value.into()));
        }
        self
    }

    /// Kill the command after `timeout` instead of after ten minutes.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Run the command and return what it produced, whatever it exited with.
    ///
    /// For the handful of commands whose non-zero exit is an answer rather
    /// than a failure — `diff --quiet`, `merge-base --is-ancestor`. Everything
    /// else uses [`GitCommand::run_ok`].
    pub async fn run(self) -> std::result::Result<GitOutput, GitError> {
        let (code, stdout, stderr) = self.collect_output().await?;

        Ok(GitOutput {
            status: code.unwrap_or(NO_EXIT_CODE),
            stdout,
            stderr,
        })
    }

    /// Run the command and fail unless it exited 0.
    ///
    /// The failure carries the argv, the exit code and git's stderr, which is
    /// everything a log line needs to reproduce the invocation by hand.
    pub async fn run_ok(self) -> std::result::Result<GitOutput, GitError> {
        let args = self.argv();
        let (code, stdout, stderr) = self.collect_output().await?;

        if code == Some(0) {
            return Ok(GitOutput {
                status: 0,
                stdout,
                stderr,
            });
        }

        Err(GitError::Command { args, code, stderr })
    }

    /// The argv as strings, for the error and the log line.
    fn argv(&self) -> Vec<String> {
        self.args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    /// The child's entire environment: the allow-list, then whatever
    /// [`GitCommand::identity`] added.
    fn environment(&self) -> Vec<(OsString, OsString)> {
        let mut env: Vec<(OsString, OsString)> = Vec::new();

        // The two the orchestrator's own environment supplies: `PATH` so the
        // binary is found at all, `HOME` because git still consults it for
        // things `GIT_CONFIG_GLOBAL` does not cover.
        if let Some(path) = std::env::var_os("PATH") {
            env.push(("PATH".into(), path));
        }
        if let Some(home) = std::env::var_os("HOME") {
            env.push(("HOME".into(), home));
        }

        // Never ask a human anything: there is no terminal to ask on, and a
        // prompt would hang until the timeout.
        env.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
        // `/etc/gitconfig` is the host's, and the orchestrator's behaviour must
        // not depend on it.
        env.push(("GIT_CONFIG_NOSYSTEM".into(), "1".into()));
        // Porcelain output is parsed, so the locale must not translate or
        // reorder anything (ADR 0011).
        env.push(("LC_ALL".into(), "C".into()));
        env.push((
            "GIT_CONFIG_GLOBAL".into(),
            self.config_global
                .clone()
                .unwrap_or_else(|| PathBuf::from(NO_GLOBAL_CONFIG))
                .into_os_string(),
        ));

        env.extend(self.env.iter().cloned());
        env
    }

    /// Spawn, read both pipes to the end, and wait — under the timeout.
    ///
    /// The two pipes are read concurrently: git writes a large diff to stdout
    /// and progress to stderr, and draining one after the other deadlocks as
    /// soon as the pipe the process is not reading fills its buffer.
    async fn collect_output(&self) -> std::result::Result<(Option<i32>, String, String), GitError> {
        let args = self.argv();
        let cwd = self.cwd.clone().unwrap_or_else(|| PathBuf::from("."));

        // `debug` and never higher, and the argv only: no environment value is
        // ever rendered here (CLAUDE.md rule 3).
        debug!(git.args = ?args, git.cwd = %cwd.display(), "running git");

        let mut command = Command::new(GIT_BINARY);
        command
            .args(&self.args)
            .current_dir(&cwd)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The timeout path kills the child explicitly; this covers every
            // other way this future can be dropped, such as a cancelled
            // request.
            .kill_on_drop(true);
        for (key, value) in self.environment() {
            command.env(key, value);
        }

        let mut child = command.spawn().map_err(GitError::Io)?;

        let (Some(mut stdout_pipe), Some(mut stderr_pipe)) =
            (child.stdout.take(), child.stderr.take())
        else {
            return Err(GitError::Io(std::io::Error::other(
                "git was spawned without captured stdout and stderr",
            )));
        };

        let collect = async {
            let mut stdout_buf = Vec::new();
            let mut stderr_buf = Vec::new();

            let (stdout_read, stderr_read) = tokio::join!(
                stdout_pipe.read_to_end(&mut stdout_buf),
                stderr_pipe.read_to_end(&mut stderr_buf),
            );
            stdout_read?;
            stderr_read?;

            let status = child.wait().await?;
            Ok::<_, std::io::Error>((status, stdout_buf, stderr_buf))
        };

        let finished = tokio::time::timeout(self.timeout, collect).await;

        let (status, stdout_buf, stderr_buf) = match finished {
            Ok(Ok(collected)) => collected,
            Ok(Err(err)) => return Err(GitError::Io(err)),
            Err(_elapsed) => {
                // The child still holds the project git lock's work; kill it
                // rather than leave it running behind the answer.
                let _ = child.kill().await;
                return Err(GitError::Command {
                    args,
                    code: None,
                    stderr: format!("git timed out after {} seconds", self.timeout.as_secs()),
                });
            }
        };

        Ok((
            status.code(),
            String::from_utf8_lossy(&stdout_buf).into_owned(),
            String::from_utf8_lossy(&stderr_buf).into_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// An obviously fake bearer value. It authenticates nowhere and never did
    /// (CLAUDE.md rule 3); the test only asserts where it does *not* appear.
    const FAKE_HEADER: &str = "Authorization: Basic FAKE-NOT-A-CREDENTIAL-0000";

    /// A repository with one commit, in a temporary directory.
    async fn temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temp dir");

        GitCommand::new()
            .args(["init", "--quiet", "--initial-branch", "main"])
            .cwd(dir.path())
            .run_ok()
            .await
            .expect("git init succeeds");

        dir
    }

    #[tokio::test]
    async fn the_version_probe_succeeds() {
        let output = GitCommand::new()
            .arg("--version")
            .run_ok()
            .await
            .expect("git is on PATH");

        assert_eq!(output.status, 0);
        assert!(output.stdout.starts_with("git version"), "{output:?}");
    }

    #[tokio::test]
    async fn an_unknown_subcommand_reports_the_exit_code_and_stderr() {
        let error = GitCommand::new()
            .arg("definitely-not-a-subcommand")
            .run_ok()
            .await
            .expect_err("an unknown subcommand fails");

        let GitError::Command { args, code, stderr } = error else {
            panic!("expected a command failure, got {error:?}");
        };

        assert_eq!(args, vec!["definitely-not-a-subcommand".to_string()]);
        assert!(code.is_some(), "git exited on a signal");
        assert_ne!(code, Some(0));
        assert!(!stderr.is_empty(), "git said nothing about the failure");
    }

    #[tokio::test]
    async fn run_returns_a_non_zero_exit_instead_of_failing() {
        // The other half of the pair: `run` is for the commands whose non-zero
        // exit is an answer.
        let output = GitCommand::new()
            .arg("definitely-not-a-subcommand")
            .run()
            .await
            .expect("spawning git succeeds even when git refuses");

        assert_ne!(output.status, 0);
        assert!(!output.stderr.is_empty());
    }

    #[tokio::test]
    async fn the_child_environment_is_the_allow_list_and_never_carries_git_dir() {
        let command = GitCommand::new().identity(&CommitIdentity {
            name: "Mars Test Bot".to_string(),
            email: "bot@example.test".to_string(),
        });

        let env = command.environment();
        let names: Vec<String> = env
            .iter()
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();

        for required in [
            "GIT_TERMINAL_PROMPT",
            "GIT_CONFIG_NOSYSTEM",
            "LC_ALL",
            "GIT_CONFIG_GLOBAL",
            "GIT_AUTHOR_NAME",
            "GIT_COMMITTER_EMAIL",
        ] {
            assert!(names.contains(&required.to_string()), "missing {required}");
        }
        for forbidden in ["GIT_DIR", "GIT_WORK_TREE"] {
            assert!(
                !names.contains(&forbidden.to_string()),
                "leaked {forbidden}"
            );
        }

        // Nothing but the allow-list: `PATH` and `HOME` are the only two taken
        // from the parent, and both may be absent there.
        assert!(names.len() <= 10, "unexpected environment: {names:?}");
    }

    #[tokio::test]
    async fn a_git_dir_in_the_parent_environment_does_not_reach_the_child() {
        let repo = temp_repo().await;

        // Test-only, and the reason `env_clear` exists: a `GIT_DIR` anywhere in
        // the orchestrator's own environment would otherwise silently redirect
        // every command at a repository nobody asked for.
        //
        // SAFETY: nothing else in this crate reads `GIT_DIR`; the wrapper
        // clears the child's environment rather than consulting it.
        unsafe { std::env::set_var("GIT_DIR", "/nonexistent/hijacked.git") };

        let output = GitCommand::new()
            .args(["rev-parse", "--git-dir"])
            .cwd(repo.path())
            .run_ok()
            .await;

        // SAFETY: as above, and restored before the assertions so a failure
        // does not leave it set for the rest of the process.
        unsafe { std::env::remove_var("GIT_DIR") };

        let output = output.expect("rev-parse resolves the repository at cwd");
        assert_eq!(output.stdout.trim(), ".git");
        assert!(!output.stdout.contains("hijacked"), "{output:?}");
    }

    #[tokio::test]
    async fn the_global_config_hook_is_honoured() {
        let repo = temp_repo().await;
        let config = repo.path().join("fake-global-config");
        fs::write(&config, "[user]\n\tname = Mars Fake Bot\n").expect("the config is written");

        let output = GitCommand::new()
            .args(["config", "--get", "user.name"])
            .cwd(repo.path())
            .config_global(&config)
            .run_ok()
            .await
            .expect("the configured name is readable");

        assert_eq!(output.stdout.trim(), "Mars Fake Bot");
    }

    #[tokio::test]
    async fn a_credential_lives_in_the_config_and_never_in_the_recorded_argv() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let config = dir.path().join("fake-credential-config");
        fs::write(&config, format!("[http]\n\textraHeader = {FAKE_HEADER}\n"))
            .expect("the config is written");

        // Not a repository, so git refuses and the failure records the argv.
        let error = GitCommand::new()
            .args(["rev-parse", "--git-dir"])
            .cwd(dir.path())
            .config_global(&config)
            .run_ok()
            .await
            .expect_err("rev-parse outside a repository fails");

        let GitError::Command { ref args, .. } = error else {
            panic!("expected a command failure, got {error:?}");
        };

        assert_eq!(
            args,
            &vec!["rev-parse".to_string(), "--git-dir".to_string()]
        );
        for rendered in [format!("{args:?}"), error.to_string(), format!("{error:?}")] {
            assert!(
                !rendered.contains("FAKE-NOT-A-CREDENTIAL"),
                "the credential reached a logged form: {rendered}"
            );
        }
    }

    #[tokio::test]
    async fn a_timed_out_command_is_killed_and_reports_no_exit_code() {
        let repo = temp_repo().await;

        // A `!` alias is a shell command git waits on, which is the hung-remote
        // shape without needing a remote.
        let error = GitCommand::new()
            .args(["-c", "alias.pause=!sleep 5", "pause"])
            .cwd(repo.path())
            .timeout(Duration::from_millis(250))
            .run_ok()
            .await
            .expect_err("a command past its timeout fails");

        let GitError::Command { code, stderr, .. } = error else {
            panic!("expected a command failure, got {error:?}");
        };

        assert_eq!(code, None, "a killed child has no exit code");
        assert!(stderr.contains("timed out"), "{stderr}");
    }
}
