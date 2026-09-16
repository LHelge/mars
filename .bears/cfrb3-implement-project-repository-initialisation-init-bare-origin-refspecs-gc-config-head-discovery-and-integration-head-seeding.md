---
id: cfrb3
title: "Implement project repository initialisation: init --bare, origin refspecs, gc config, HEAD discovery and integration-head seeding"
status: open
priority: P0
created: "2026-09-16T20:29:16.674237823Z"
updated: "2026-09-16T20:29:16.674237823Z"
tags:
  - orchestrator
  - git
  - projects
depends_on:
  - bv7a5
  - g6vk2
  - w2uhm
  - "6gvak"
parent: z4u4e
---

## Summary
Implement the git side of project creation: create the bare project repository at `DATA_DIR/projects/<id>/repo.git`, configure `origin` with the documented fetch refspecs and gc settings, discover the remote's default branch when the user did not supply one, run the first fetch, seed one Mars integration head per fetched upstream branch and point the repository's `HEAD` at the default integration branch. The Projects epic's background clone job calls this routine and moves the project to `ready` or `error`.

## Documents
- `ARCHITECTURE.md` "Git model" -> "Project clone" (exact commands and settings), "Ref ownership", "Serialization" (initialisation under the project git lock)
- `ARCHITECTURE.md` "Storage" (`/data/projects/<project_id>/repo.git`)
- `SPEC.md` "Projects" (`default_branch` supplied or discovered; must resolve to an integration head before `ready`)
- `docs/data-model.md` `projects` (`default_branch` nullable, CHECK on `ready`)
- ADR 0001 (gc settings), ADR 0017 (no mirror mode; integration heads seeded on import)

## Acceptance criteria
- [ ] `git::mirror::init_project_repo(guard: &ProjectGitGuard, paths: &DataPaths, remote_url: &str, requested_default: Option<&str>, credential: Option<&GitCredential>) -> Result<InitOutcome { default_branch: String, seeded: Vec<String> }, GitError>` performs, in order: `git init --bare <repo>`; `git remote add origin <remote_url>`; `git config --replace-all remote.origin.fetch '+refs/heads/*:refs/remotes/origin/*'` then `git config --add remote.origin.fetch '+refs/tags/*:refs/tags/*'`; `git config gc.auto 0`; `git config gc.pruneExpire never`; leaves `remote.origin.mirror` unset; `git ls-remote --symref origin HEAD` parsed for `ref: refs/heads/<name>\tHEAD` when `requested_default` is `None`; `git fetch --prune origin`; for each `refs/remotes/origin/<b>` create `refs/heads/<b>` at the same commit only if `refs/heads/<b>` does not already exist; `git symbolic-ref HEAD refs/heads/<default>`.
- [ ] `remote_url` is validated `https://` only and must not contain userinfo (`user:pass@`); otherwise `GitError::InvalidRef`-style validation error (`GitError::InvalidRemote(String)`, add the variant, mapped 400).
- [ ] If the default branch (requested or discovered) is not among the fetched upstream branches, the routine returns `GitError::UnknownRef("<name>")` after the fetch; the repository stays on disk so `retry-clone` can rerun the routine idempotently.
- [ ] The routine is idempotent: re-running on an existing `repo.git` re-applies config, fetches again and seeds only missing heads; existing integration heads are never moved.
- [ ] The credential, when present, is applied through `CredentialConfig` for `ls-remote` and `fetch` only; `remote_url` is stored and configured without any credential.
- [ ] `git::mirror::remove_project_repo(guard, paths)` deletes `repo.git` (used by project deletion in the Projects epic).
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/git/mirror.rs`; `orchestrator/src/git/paths.rs` with `DataPaths { data_dir: PathBuf }` and methods `project_repo(pid)`, `project_dir(pid)`, `session_work(sid)`, `session_dir(sid)`, `tmp()`, built from `Config.data_dir` (absolute).
- Symref parsing: `ls-remote --symref` first line `ref: refs/heads/main\tHEAD`; if absent (detached upstream HEAD or empty repository) return `GitError::UnknownRef("HEAD")` unless `requested_default` is given.
- Seeding uses `git::refs::list(repo, ["refs/remotes/origin/"])` and `git::refs::update(repo, "refs/heads/<b>", commit, None)` from the refs task; skip `origin/HEAD`.
- Also run `git config core.logAllRefUpdates true` so `refs/heads/*` and `refs/sessions/*` writes get reflogs (helps operator recovery; bare repos default it off). Note this in the doc comment; it is not a contract change.
- Empty upstream (no branches): treat as error `UnknownRef` for the default branch; the Projects epic maps this to `status: error` with `status_message`.

## Edge cases
- Upstream unreachable or credential rejected: `GitError::Command` from `ls-remote`/`fetch`; the caller sets `status = error` with a human-readable `status_message` that must not include stderr verbatim if stderr could contain the URL with userinfo (it cannot, since userinfo is rejected on input; still pass only the first line of stderr).
- Branch names containing `/` (`feature/x`) seed correctly as `refs/heads/feature/x`; a name that conflicts with an existing directory-style ref (`feature` vs `feature/x`) makes `update-ref` fail; surface as `GitError::Command`.
- Tags fetched via the second refspec must not be seeded as heads.

## Testing
- Real bare repositories in `tempfile` directories (extend the shared helper from the refs task): upstream with `main` and `feature/x` and an annotated tag; run `init_project_repo` with `None` and assert discovered `main`, `refs/heads/main` and `refs/heads/feature/x` equal their `refs/remotes/origin/*` counterparts, `HEAD` symref is `refs/heads/main`, `gc.auto` is `0`, `gc.pruneExpire` is `never`, `remote.origin.fetch` has exactly the two refspecs, `remote.origin.mirror` unset.
- Requested default branch present and absent (`UnknownRef`); `https://user:pw@host/x` rejected; `http://` rejected; idempotent re-run after advancing upstream `main` leaves `refs/heads/main` unchanged and updates `refs/remotes/origin/main`.
- Credential path: with the mock provider returning a fake credential, assert the config file is gone after the call (list `DATA_DIR/tmp`).
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Projects, agent profiles and shared directories": the background clone job that calls this routine, sets `default_branch`, `status` and `status_message`, and creates `DATA_DIR/projects/<id>/` siblings (`claude/`, `shared/`).