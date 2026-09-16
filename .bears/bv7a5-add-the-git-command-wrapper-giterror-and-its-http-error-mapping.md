---
id: bv7a5
title: Add the git command wrapper, GitError and its HTTP error mapping
status: open
priority: P0
created: "2026-09-16T20:27:09.950042128Z"
updated: "2026-09-16T20:51:51.818386654Z"
tags:
  - orchestrator
  - git
depends_on:
  - t36d2
parent: z4u4e
---

## Summary
Create `orchestrator/src/git/` with the single wrapper every git operation goes through: a `tokio::process::Command` builder for the `git` binary that takes argv arrays, an explicit working directory and an explicit environment, captures stdout and stderr, and maps exit codes to a typed `GitError`. Also add the `Error` mapping so a `GitError` becomes the documented HTTP response (422 with `conflicts`, 409 for non-fast-forward, 400 for bad refs). Everything later in the epic builds on this module.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals" (module layout `src/git/`, `Error` enum contract with `#[from] GitError`)
- `ARCHITECTURE.md` "Git model" -> "Credentials" (no credential in argv; `GIT_CONFIG_GLOBAL` hook)
- `SPEC.md` "REST API" status table (400, 409, 422 with `conflicts: string[]`)
- ADR 0011 (shell out, porcelain-only parsing)

## Acceptance criteria
- [ ] `git::command::GitCommand` builds argv as a `Vec<OsString>` (never a shell string), sets `cwd`, sets `env_clear()` plus an explicit allow-list (`PATH`, `HOME`, `GIT_TERMINAL_PROMPT=0`, `GIT_CONFIG_NOSYSTEM=1`, `LC_ALL=C`, `GIT_CONFIG_GLOBAL=<path or /dev/null>`, optional `GIT_AUTHOR_*`/`GIT_COMMITTER_*`), and never inherits `GIT_DIR`/`GIT_WORK_TREE`.
- [ ] `GitCommand::run()` returns `GitOutput { status: i32, stdout: String, stderr: String }`; `run_ok()` returns `Err(GitError::Command { args, code, stderr })` on non-zero exit.
- [ ] `GitError` (thiserror) has at least: `Command { args: Vec<String>, code: Option<i32>, stderr: String }`, `Io(std::io::Error)`, `Conflict { paths: Vec<String> }`, `NonFastForward { remote_branch: String }`, `InvalidRef(String)` (name does not parse or wrong kind for the operation), `UnknownRef(String)` (name does not resolve in the repository), `NotACommit(String)`, `DirtyWorkTree`, `CredentialUnavailable`.
- [ ] `Error` gains the mapping: `Conflict{paths}` -> 422 `{ "status": 422, "error": "merge conflict", "conflicts": [..] }`; `NonFastForward` -> 409; `InvalidRef`/`UnknownRef`/`NotACommit` -> 400 with the name in the message; `CredentialUnavailable` -> 409; `Command`/`Io` -> 500 with a generic message and `tracing::error!` logging stderr (stderr never contains the credential because it is never in argv or output).
- [ ] The logged/serialised form of `GitError::Command.args` is the argv as run; a unit test asserts a credential header value never appears there because credentials go through the config file, not argv (see the credential task).
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/git/mod.rs` (re-exports), `orchestrator/src/git/command.rs`, `orchestrator/src/git/error.rs`; extend `orchestrator/src/prelude/error.rs` (`#[from] GitError` variant and the `IntoResponse` arm that adds `conflicts` to the JSON body).
- Builder sketch:
  ```rust
  pub struct GitCommand { args: Vec<OsString>, cwd: Option<PathBuf>, env: Vec<(OsString, OsString)>, config_global: Option<PathBuf> }
  impl GitCommand {
      pub fn new() -> Self; pub fn arg(self, a: impl AsRef<OsStr>) -> Self; pub fn args<I>(self, i: I) -> Self;
      pub fn cwd(self, p: impl Into<PathBuf>) -> Self;            // sets `-C` semantics via Command::current_dir
      pub fn config_global(self, p: PathBuf) -> Self;             // GIT_CONFIG_GLOBAL, used by the credential guard
      pub fn identity(self, id: &CommitIdentity) -> Self;         // GIT_AUTHOR_*/GIT_COMMITTER_* env
      pub async fn run(self) -> Result<GitOutput, GitError>; pub async fn run_ok(self) -> Result<GitOutput, GitError>;
  }
  ```
- Timeouts: wrap `run()` in `tokio::time::timeout` with a default of 10 minutes (network operations) so a hung remote cannot pin a project git lock forever; a timeout kills the child and returns `GitError::Command` with `code: None`.
- Output parsing helpers for porcelain formats only (`--porcelain`, `for-each-ref --format`, `rev-parse`); never parse human-readable messages except the two documented push rejection markers handled in the push task.
- Log at `debug` with structured fields (`git.args = ?args`, `git.cwd = %cwd`), never at `info` or above, never with env values.
- `git` must be on `PATH` at startup: add a startup probe `git --version` in `main.rs` that fails fast with a clear message (one line; the version is pinned in the orchestrator image by the Deployment packaging epic).

## Edge cases
- Non-UTF-8 output: use `String::from_utf8_lossy`.
- Very large stdout (diff patches): `run()` must read stdout and stderr concurrently (`tokio::join!`) to avoid pipe deadlock.
- Child killed by signal: `code: None`.
- `env_clear()` must still pass `PATH`; on the host `HOME` is needed by git for nothing else once `GIT_CONFIG_GLOBAL` is set explicitly, but keep it to be safe.

## Testing
- Unit tests in `command.rs`: `git --version` succeeds; a bad subcommand yields `GitError::Command` with the exit code and stderr; `env_clear` is effective (set `GIT_DIR` in the parent, run `rev-parse --git-dir` in a temp repo and assert it is not influenced).
- Unit tests in `prelude/error.rs`: each `GitError` variant maps to the documented status and `Conflict` serialises `conflicts`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written (`ARCHITECTURE.md` "Orchestrator internals" already lists `GitError` in the `Error` enum).

## Assumes from other epics
- "Database schema, models, repositories and test harness": `src/prelude/error.rs` exists with the `Error` enum and `IntoResponse` impl to extend.
- "Repository scaffolding, tooling and CI": the crate skeleton, `tokio`, `thiserror`, `tracing` dependencies.