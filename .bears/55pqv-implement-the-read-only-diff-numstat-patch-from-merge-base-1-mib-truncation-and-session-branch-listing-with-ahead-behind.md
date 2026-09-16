---
id: "55pqv"
title: Implement the read-only diff (numstat, patch from merge-base, 1 MiB truncation) and session-branch listing with ahead/behind
status: open
priority: P1
created: "2026-09-16T20:32:13.054796135Z"
updated: "2026-09-16T20:32:13.054796135Z"
tags:
  - orchestrator
  - git
depends_on:
  - vztkg
parent: z4u4e
---

## Summary
Implement the two read-only queries the UI's "Changes" panel, the task revision view and `GET /projects/{pid}/git/session-branches` need: a diff between fixed commits computed directly against the project repository (`--name-status`/`--numstat` plus the unified patch from `merge-base(base, head)` to `head`, truncated above 1 MiB), and the list of `refs/sessions/*` with ahead/behind counts against the integration head named by `default_branch`. Both take already-resolved commits so the service can capture ref resolutions under the lock and run the git commands outside it.

## Documents
- `ARCHITECTURE.md` "Git model" -> "Diff" (numstat and patch from merge-base directly against the mirror; session head synced first without a `git` event; `handoff_id` selects the retained commit; 1 MiB truncation), "Serialization" (ref resolutions for read-only diffs captured under the lock)
- `SPEC.md` "Git" (`GET .../diff?head=&base=` or `?handoff_id=&base=` -> `Diff = { base, head, merge_base, files: {path, status, additions, deletions}[], patch, truncated }`; `base` defaults to the project's default branch; `SessionBranch = { session_id, ref, commit, ahead, behind, base: default_branch, updated_at }`; ahead/behind measured against the integration head named by `default_branch`)
- `SPEC.md` "MCP tool contracts" -> `list_session_branches` (same shape)
- `SPEC.md` "Frontend" (refresh-on-git-event rule for the Changes panel)

## Acceptance criteria
- [ ] `git::diff::diff(paths, project_id, base: &ResolvedRef, head: &ResolvedRef, limit: usize /* 1 MiB */) -> Result<Diff, GitError>`: `merge_base = git merge-base <base.commit> <head.commit>` (if none, `GitError::UnknownRef` with message "no merge base"); files from `git diff --name-status -M --no-renames? ` — use `git diff --numstat --no-color <mb> <head>` for additions/deletions and `git diff --name-status --no-color <mb> <head>` for status, joined by path; patch from `git diff --no-color --no-ext-diff <mb> <head>`; `truncated = patch.len() > limit`, in which case `patch` is cut at the last char boundary at or before `limit`.
- [ ] `Diff` (in `models/git.rs`) serialises `{ base: <api name>, head: <api name>, merge_base: <sha>, files: [{ path, status: "A"|"M"|"D"|"R"|"C"|"T", additions: u32, deletions: u32 }], patch: String, truncated: bool }`; binary files report `additions: 0, deletions: 0` (numstat prints `-`).
- [ ] `git::diff::session_branches(paths, project_id, default_branch: &str) -> Result<Vec<SessionBranch>, GitError>`: for each `refs/sessions/<sid>` (from `git::refs::list`) compute `git rev-list --left-right --count refs/heads/<default>...refs/sessions/<sid>` -> `behind` (left) and `ahead` (right); `updated_at` is the tip's committer date; `base` is `default_branch`; `ref` is `refs/sessions/<sid>`; sorted by `updated_at` descending.
- [ ] Both functions run with `--no-ext-diff` / never invoke external diff or textconv helpers configured in the repository (the mirror is orchestrator-owned, but keep the flag).
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/git/diff.rs`; `Diff`, `DiffFile`, `SessionBranch` in `orchestrator/src/models/git.rs`.
- Renames: run `--name-status` without `-M` so paths match `--numstat` one-to-one (renames appear as `D` + `A`); note the choice in the doc comment (the frontend shows files, not rename arrows).
- `-z` output for `--name-status` and `--numstat` avoids quoting of unusual paths; parse NUL-separated.
- When `refs/heads/<default>` is missing (should not happen for a `ready` project), `session_branches` returns `UnknownRef`.
- The service performs the session fetch-back (silent) before resolving `head` when it is a `Session`; this function never syncs.

## Edge cases
- `head == base` or empty diff: `files: []`, `patch: ""`, `truncated: false`.
- Unrelated histories (no merge base): `UnknownRef("no merge base")` -> 400 at the route.
- Patch exactly at the limit: not truncated.
- Non-UTF-8 patch bytes: lossy conversion before truncation (`String::from_utf8_lossy`).
- Very large repositories: `git diff` streams; the wrapper reads stdout fully (bounded by the repository, not by the limit); acceptable in v1.

## Testing
- Real repositories: session A adds `a.txt`, modifies `README.md`, deletes `old.txt`; `diff(Head(main), Session(A))` -> three files with the right statuses and counts, `merge_base` equals the branch point, patch contains `diff --git a/a.txt`. Advance `main` after branching and assert the patch still shows only A's changes (merge-base based).
- Truncation: commit a 2 MiB text file; `truncated: true`, `patch.len() <= 1 MiB`, valid UTF-8.
- Binary file: `additions == 0 && deletions == 0`.
- `session_branches`: two sessions, one with 2 commits ahead and `main` advanced by 1 -> `ahead: 2, behind: 1`; ordering by `updated_at`; `base == "main"`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- none.