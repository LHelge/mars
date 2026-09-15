# 0011. Shell out to the `git` binary

Status: accepted

## Context

Rust has two mature options for git: `git2` (libgit2 bindings) and `gitoxide`. Both avoid process spawning and give typed APIs. Both also lag the `git` binary on features (alternates handling, credential helpers, partial clone, `fetch --prune` edge cases, `http.extraHeader`) and libgit2 has a history of subtle behavioural differences in merges and rebases.

## Decision

All git operations are `tokio::process::Command` invocations of the `git` binary, wrapped in one module (`orchestrator/src/git/`) that builds argument vectors, sets the working directory and environment explicitly, captures stdout and stderr, and maps exit codes to typed errors. Credentials are passed with `-c http.extraHeader=Authorization: ...` (for tokens) or `GIT_ASKPASS` pointing at a one-shot helper; never in the remote URL and never in argv where they would show in `ps`.

## Consequences

- Behaviour matches what users get on the command line; debugging is `git` debugging.
- Every operation is testable against a real repository in a temp directory; the test suite creates bare "upstream" repositories on disk instead of mocking.
- The orchestrator image must ship `git`; the version is pinned in the Dockerfile.
- Output parsing is limited to porcelain formats (`--porcelain`, `for-each-ref --format`, `rev-parse`), never human-readable output.
