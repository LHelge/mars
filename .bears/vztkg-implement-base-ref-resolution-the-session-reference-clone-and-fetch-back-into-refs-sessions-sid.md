---
id: vztkg
title: Implement base-ref resolution, the session reference clone and fetch-back into refs/sessions/<sid>
status: done
priority: P0
created: "2026-09-16T20:30:17.561615227Z"
updated: "2026-09-18T00:53:57.465826821Z"
tags:
  - orchestrator
  - git
  - sessions
depends_on:
  - g6vk2
  - cfrb3
parent: z4u4e
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Implement the session side of the git model: resolve a `base_ref` of any supported kind to a commit in the project repository, create the session work clone with `git clone --reference --no-checkout`, fetch the selected named ref explicitly, create `session/<sid>` at the resolved commit with the launching user's identity, and provide the fetch-back that copies `session/<sid>` from the work clone into `refs/sessions/<sid>` in the mirror. The session launcher, `POST /sessions/{id}/end`, `POST /sessions/{id}/sync`, hand-off publication and every merge/rebase/push/diff involving a session call these.

## Documents
- `ARCHITECTURE.md` "Git model" -> "Session clone", "Fetch-back", "Serialization" (fresh launch holds the lock through base resolution and clone setup; fetch-back is under the lock)
- `ARCHITECTURE.md` "Storage" (`/data/sessions/<session_id>/work`; alternates path identical inside and outside the container), "Launch sequence"
- `SPEC.md` "Sessions" (`POST /projects/{pid}/sessions`: 400 if `base_ref` does not resolve; `POST /sessions/{id}/sync` -> `{ref, commit}`), "Projects" (default base is the integration head named by `default_branch`; explicit upstream ref, tag, session ref or commit id allowed)
- `docs/data-model.md` `sessions.base_ref`, `sessions.branch` (`session/<id>`)
- ADR 0001, ADR 0007, ADR 0017

## Acceptance criteria
- [ ] `git::session::resolve_base(guard, paths, project_id, base_ref: Option<&str>, default_branch: &str) -> Result<ResolvedRef, GitError>`: `None` resolves `GitRef::Head(default_branch)`; `Some` parses with `GitRef::parse` and resolves; `Handoff` refs are accepted (the Code hand-offs epic passes the pinned commit as a 40-hex `base_ref`, so this is only for explicit fully qualified input); returns `UnknownRef`/`InvalidRef` (both 400 at the sessions endpoint).
- [ ] `git::session::create_work_clone(guard, paths, project_id, session_id, base: &ResolvedRef, identity: &CommitIdentity) -> Result<(), GitError>` runs: `git clone --reference <repo> --no-checkout <repo> <work>`; if `base.git_ref.full_name()` is `Some(name)` and the kind is `Upstream`, `Tag`, `Session` or `Handoff`, `git -C <work> fetch origin <name>` (explicit refspec so the object and ref are present; integration heads are already tracked as `refs/remotes/origin/<b>` in the clone); `git -C <work> checkout -b session/<sid> <commit>`; `git -C <work> config user.name <name>` and `config user.email <email>`.
- [ ] The clone's `.git/objects/info/alternates` contains exactly `<DATA_DIR>/projects/<pid>/repo.git/objects` using the orchestrator's `DATA_DIR` (never `DATA_DIR_HOST`).
- [ ] The clone contains no credential: no `http.extraHeader`, no `GIT_CONFIG_GLOBAL` file referenced, `origin` URL is the mirror path.
- [ ] `git::session::fetch_back(guard, paths, project_id, session_id) -> Result<String /* commit */, GitError>` runs `git -C <repo> fetch <work> +session/<sid>:refs/sessions/<sid>` (forced refspec) and returns the new `refs/sessions/<sid>` commit; when the work directory is missing it returns `GitError::UnknownRef("session/<sid>")`; when the branch is missing in the work clone (agent deleted it) it returns the same error and leaves the mirror ref untouched.
- [ ] `git::session::remove_work_clone(paths, session_id)` removes `DATA_DIR/sessions/<sid>/work` (called by session deletion; no lock needed because the mirror is untouched).
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- File: `orchestrator/src/git/session.rs`.
- `create_work_clone` runs under the caller's project git guard (the launcher acquires it for base resolution and clone setup together); the work clone itself is not a lock subject afterwards.
- Because the mirror is bare and local, `git clone` without `--no-local` hardlinks nothing when `--reference` supplies alternates; pass `--no-hardlinks` explicitly anyway so the work tree never shares inode-level files with the mirror (mounted RO in the container).
- Do not pass the credential to any command in this task: nothing here touches upstream.
- Fetch-back is a plain command on the mirror: no temp clone, no credential. Log `session_id = %sid`, `commit = %sha` at `debug`.
- The `git` event (`op: "sync"`) for an explicit sync is emitted by the service task, not here; this function is silent so the diff path can reuse it without emitting.

## Edge cases
- `base_ref` naming a commit not present in the mirror: `rev-parse` fails -> `UnknownRef` -> 400.
- `session/<sid>` already exists in the work dir (relaunch after a failed `creating`): `create_work_clone` deletes an existing `work` directory first and clones afresh; the launcher only calls it for fresh launches.
- Agent has rewritten history on `session/<sid>`: the forced refspec accepts it (the session owns that ref); hand-off pinning (other epic) compares the fetched tip to the requested commit.
- Agent-controlled checkout (ADR 0019): `fetch` from the work path executes no hooks of that repository, but this remains the accepted v1 exposure; add the ADR reference in the doc comment.

## Testing
- Real repositories via the shared helper: initialise a mirror, then for each base kind (integration head, `origin/main`, tag, an existing session ref, a 40-hex commit id, fully qualified `refs/sessions/<id>`) create a work clone and assert `git -C work rev-parse HEAD` equals the resolved commit, the current branch is `session/<sid>`, `user.name`/`user.email` match, alternates file content, and `git -C work config --get http.extraHeader` is empty.
- Fetch-back: commit in the work clone, `fetch_back`, assert `refs/sessions/<sid>` equals the work tip; amend the commit, fetch again, assert the ref moved (forced); delete the work dir, assert `UnknownRef`.
- Unknown base -> `UnknownRef`; malformed base (`..`) -> `InvalidRef`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Session lifecycle": the launcher that acquires the lock, calls `resolve_base` + `create_work_clone`, and the end/sync endpoints that call `fetch_back` through the git service.
- "Authentication, users, invites and email": the launching user's `name`/`email` fields on `users` (use `username` for `name` if no display name column exists).