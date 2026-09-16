---
id: "7q4qt"
title: Implement merge in a temporary shared clone with conflict reporting and explicit write-back
status: open
priority: P1
created: "2026-09-16T20:30:53.713122733Z"
updated: "2026-09-16T20:30:53.713122733Z"
tags:
  - orchestrator
  - git
depends_on:
  - vztkg
parent: z4u4e
---

## Summary
Implement the merge primitive: in a temporary `git clone --shared` under `DATA_DIR/tmp/`, explicitly fetch the selected source and target refs from the project repository into temporary local refs, merge with the bot identity and a `Requested-By` trailer, and on success write back only the target integration head with an explicit refspec. Conflicts abort, delete the temp clone and return the conflicting paths. This is the operation behind the branch form of `POST /projects/{pid}/git/merge`, the MCP `merge` tool and (with a pinned commit as source) the task-merge form wired by the Code hand-offs epic.

## Documents
- `ARCHITECTURE.md` "Git model" -> "Merge, rebase, push" (temp clone, explicit fetches, write back only the intended ref, conflicts abort and delete the clone), "Commit identity" (bot identity, `Requested-By: user:<id>` / `Requested-By: session:<id>` trailer), "Ref ownership" (`origin/main` into `main` integrates upstream; only integration heads are mutation targets), "Serialization"
- `SPEC.md` "Git" (`POST .../merge` -> `{commit}` or 422 `{status, error, conflicts}`; `source` accepts session id, integration or upstream name; `target` accepts an integration name; upstream refs as targets -> 400; `message?`)
- `SPEC.md` "MCP tool contracts" -> `merge` (never resolves conflicts)
- ADR 0007, ADR 0017

## Acceptance criteria
- [ ] `git::integrate::merge(guard, paths, project_id, source: &ResolvedRef, target: &ResolvedRef, message: Option<&str>, identity: &CommitIdentity, requested_by: &GitActor) -> Result<MergeOutcome { commit: String, fast_forward: bool }, GitError>`; `target.git_ref` must be `Head(_)` else `GitError::InvalidRef` (400); `source.git_ref` must satisfy `is_merge_source()`.
- [ ] Steps: `TempClone::create(paths, repo)` -> `git clone --shared --no-checkout <repo> <tmp>/<uuid>`; `git fetch origin +<target_full>:refs/tmp/target +<source_full or commit>:refs/tmp/source` (for a `Commit` source fetch by sha is unnecessary: it is readable via `--shared`, so use the sha directly); `git checkout -B work refs/tmp/target`; `git merge --no-edit -m <message> refs/tmp/source` with `GIT_AUTHOR_*`/`GIT_COMMITTER_*` set from `identity` (fast-forward is allowed; a merge commit is created only when needed); on success `git push origin work:refs/heads/<target>` (non-forced; must be a fast-forward of the target snapshot taken under the lock) and return the new tip; on conflict collect `git diff --name-only --diff-filter=U`, run `git merge --abort`, delete the temp clone, return `GitError::Conflict { paths }`.
- [ ] The default message is `Merge <source api name> into <target api name>`; the message always ends with a blank line and `Requested-By: user:<uuid>` or `Requested-By: session:<uuid>` (or `Requested-By: system` for `GitActor::System`), added with `git interpret-trailers`-equivalent formatting (a trailing `Key: value` line after a blank line); a user-supplied message keeps its text and gets the trailer appended.
- [ ] The temp clone is removed on every exit path (success, conflict, command failure, panic) via a `Drop` guard; the orphan-cleanup job remains the backstop.
- [ ] No upstream access and no credential in this operation.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/git/integrate.rs` (merge; rebase joins in the next task), `orchestrator/src/git/tempclone.rs` (`TempClone { path }` with `create`, `path()`, `Drop` removing the directory with `std::fs::remove_dir_all`, logging failures at `warn!`).
- Fetching into `refs/tmp/*` (not `refs/heads`) keeps the temp clone's branch namespace clean; `checkout -B work` gives a work tree for the merge (a merge needs an index and work tree; `--no-checkout` plus `checkout -B` populates it once).
- Write-back uses `push` to the local path so git validates the fast-forward; the project git lock guarantees the target did not move since it was resolved. If the push is rejected anyway, return `GitError::Command` (invariant failure), not `NonFastForward`.
- `fast_forward` is `true` when the returned commit equals the source commit and the target was an ancestor; it is informational for the `git` event detail.
- Trailer format: `\n\nRequested-By: session:<uuid>`; if the user message already ends with a trailer block, still append (git tolerates multiple trailers).

## Edge Cases
- Source equal to target or already merged (`git merge` prints "Already up to date", exit 0): return the current target commit with `fast_forward: false`; no write-back needed.
- Binary conflicts appear in `--diff-filter=U` like any other path.
- Modify/delete conflicts: `--diff-filter=U` lists them; fine.
- `message` longer than 10 KiB: reject with `Error::BadRequest` at the route; the primitive does not limit.
- Session source refs are synced by the service (fetch-back) before this primitive runs; the primitive itself never syncs.

## Testing
- Real repositories: mirror with `main`; session clone A commits a file; fetch-back; `merge(source = Session(A), target = Head(main))` -> `refs/heads/main` advanced, the merge commit's author/committer are the bot identity, message contains `Requested-By: user:<id>`, temp dir gone. Fast-forward case: `main` unchanged since A branched -> `fast_forward: true`, no merge commit.
- Conflict: two sessions edit the same line; merge the second after the first -> `GitError::Conflict { paths: ["file.txt"] }`, `refs/heads/main` unchanged, temp dir gone, `refs/tmp/*` never written to the mirror.
- Upstream integration: advance upstream `main`, fetch, `merge(source = Upstream(main), target = Head(main))` fast-forwards `refs/heads/main`.
- Invalid: `target = Upstream(main)` -> `InvalidRef`; `target = Session(x)` -> `InvalidRef` (targets are integration heads only).
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- none.