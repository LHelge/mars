---
id: nzchj
title: Implement push with an explicit refspec, non-fast-forward as conflict and explicit force
status: done
priority: P1
created: "2026-09-16T20:31:47.287660741Z"
updated: "2026-09-18T01:18:18.771525812Z"
tags:
  - orchestrator
  - git
depends_on:
  - vztkg
  - "7u3pu"
parent: z4u4e
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Implement the push primitive: send exactly one integration head or session ref to `refs/heads/<remote_branch>` upstream with an explicit refspec and the project credential, never a mirror push. A non-fast-forward rejection becomes `GitError::NonFastForward` (HTTP 409 / MCP `conflict`) and leaves every local ref untouched; `--force` is used only when the caller explicitly requested it. Behind `POST /projects/{pid}/git/push` and the MCP `push` tool.

## Documents
- `ARCHITECTURE.md` "Git model" -> "Merge, rebase, push" (push sends exactly the selected ref; non-fast-forward is a conflict; force only when requested; a failed push never rolls back a local merge), "Credentials"
- `SPEC.md` "Git" (`POST .../push {ref, remote_branch?, force?: boolean = false}` -> `{remote_branch, commit}`; only integration heads or session refs may be pushed (400 otherwise); non-fast-forward -> 409; REST force requires `force: true`)
- `SPEC.md` "MCP tool contracts" -> `push` (session refs pushed as `refs/heads/session/<id>` by default)
- `SPEC.md` "User-facing features" -> "Git operations" (after a push to a GitHub remote the UI links to the compare page)
- ADR 0002, ADR 0007, ADR 0017

## Acceptance criteria
- [ ] `git::push::push(guard, paths, project_id, r: &ResolvedRef, remote_branch: Option<&str>, force: bool, credential: Option<&GitCredential>) -> Result<PushOutcome { remote_branch: String, commit: String }, GitError>`; `r.git_ref` must satisfy `is_push_source()` (`Head` or `Session`), else `GitError::InvalidRef` (400).
- [ ] Default `remote_branch`: `<name>` for `Head(name)`, `session/<sid>` for `Session(sid)`; a supplied `remote_branch` is validated as a branch name (same rules as `GitRef::parse` heads; no `refs/` prefix accepted).
- [ ] Command: `git -C <repo> push --porcelain [--force] origin <full_local_ref>:refs/heads/<remote_branch>` with the credential config attached when present; never `--mirror`, never `--all`, never `--tags`.
- [ ] `--porcelain` output is parsed: a line starting with `!` whose summary contains `[rejected]` with `(non-fast-forward)`, `(fetch first)` or `(stale info)` -> `GitError::NonFastForward { remote_branch }`; any other `!` line or non-zero exit -> `GitError::Command` (500, generic message); the `commit` returned is `r.commit`.
- [ ] After a successful push git updates `refs/remotes/origin/<remote_branch>` itself (the configured fetch refspec matches); the primitive asserts this in tests but does not write refs.
- [ ] `PushOutcome` also carries `compare_url: Option<String>` computed for GitHub remotes: for `https://github.com/<owner>/<repo>(.git)` produce `https://github.com/<owner>/<repo>/compare/<default_branch>...<remote_branch>?expand=1`; `None` for other hosts. (The REST body stays `{remote_branch, commit}` as specified; the URL goes into the `git` event detail so the UI can link it.)
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- File: `orchestrator/src/git/push.rs`.
- Porcelain push output format: `<flag>\t<from>:<to>\t<summary> (<reason>)`; flag `!` means rejected. Parse only these markers; do not match on human-readable prose elsewhere.
- Force semantics: `force: true` adds `--force` (not `--force-with-lease`; lease semantics would need the caller to supply an expected remote commit, which the API does not expose). Document this in the doc comment.
- The credential config is created just before the command and dropped right after; the `secret_uses` row is written by the provider when the service obtains the credential (`GitActor::User` for REST, `GitActor::Session` for MCP).
- Compare URL helper `github_compare_url(remote_url, base, head) -> Option<String>` lives here and is unit-tested; the `default_branch` is passed in by the service.

## Edge cases
- Remote branch protected / permission denied: git prints `! [remote rejected] ... (protected branch hook declined)` -> not a non-fast-forward -> `GitError::Command`, mapped 500 generic with stderr logged (stderr contains no credential).
- Pushing a session ref whose commit is already on the remote branch: "up to date", exit 0, `=` flag line -> success with the same commit.
- Deleting remote branches is not supported (no empty `<from>`); reject at validation.
- Upstream unreachable: `GitError::Command`.

## Testing
- Real repositories: upstream bare repo as `origin`; after a merge into `main`, `push(Head(main))` moves upstream `main` and `refs/remotes/origin/main` to the same commit. `push(Session(A))` creates upstream `session/<A>`; with `remote_branch = "feature/x"` creates that branch instead.
- Non-fast-forward: advance upstream `main` by a commit the mirror lacks, push `main` -> `NonFastForward`, mirror `refs/heads/main` and `refs/remotes/origin/main` unchanged; `force: true` then succeeds and upstream `main` equals the mirror head.
- Invalid: `push(Upstream(main))` -> `InvalidRef`; `remote_branch = "refs/heads/x"` -> `InvalidRef`.
- Credential: with a fake `GitCredential` the temp config file exists during the command (observe from a test hook or assert absence afterwards) and is absent afterwards, including when the push fails (unreachable remote path).
- `github_compare_url` unit tests for `.git` and non-`.git` URLs and a non-GitHub host.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written (the compare URL is exposed in the `git` event detail, documented by the service task).

## Assumes from other epics
- none.