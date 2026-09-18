---
id: g6vk2
title: Add ref naming, resolution and ref plumbing (GitRef, rev-parse, for-each-ref, update-ref, hand-off refs)
status: done
priority: P0
created: "2026-09-16T20:28:05.514988652Z"
updated: "2026-09-17T23:58:14.183728715Z"
tags:
  - orchestrator
  - git
depends_on:
  - bv7a5
parent: z4u4e
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Define the typed ref model the whole epic shares: how API names (`main`, `origin/main`, a session id, a tag, a commit id, a fully qualified ref) map to the namespaces owned by the project repository (`refs/heads/*`, `refs/remotes/origin/*`, `refs/tags/*`, `refs/sessions/*`, `refs/handoffs/*`), which kinds may be mutation targets or push sources, and the plumbing helpers (`rev-parse --verify`, `for-each-ref --format`, `update-ref`, `update-ref -d`) that every later operation uses. Also delivers the hand-off ref retention and removal primitives that the Code hand-offs epic and the orphan-cleanup job call.

## Documents
- `ARCHITECTURE.md` "Git model" -> "Ref ownership" (namespaces, read-only upstream refs, only integration heads and session refs are mutation targets or push sources, `refs/handoffs/<id>` never a mutation target or push source)
- `SPEC.md` "Git" (`source`/`head` accept session id, integration name or `origin/<name>`; `onto`/`base` accept integration or upstream names; `branch`/`ref` accept session id or integration name; `target` accepts an integration name; fully qualified refs accepted; upstream refs as targets/sources -> 400)
- `SPEC.md` "Projects" (`Branch = { name, kind: "head" | "upstream" | "session", commit, session_id? }`, names `main` vs `origin/main`)
- `docs/data-model.md` `task_handoffs` (`refs/handoffs/<id>`, refs removed on task/project deletion under the git lock, orphan cleanup)
- ADR 0017, ADR 0018

## Acceptance criteria
- [ ] `git::refs::GitRef` enum: `Head(String)`, `Upstream(String)`, `Tag(String)`, `Session(Uuid)`, `Handoff(Uuid)`, `Commit(String)` with `full_name() -> Option<String>` (`refs/heads/<b>`, `refs/remotes/origin/<b>`, `refs/tags/<t>`, `refs/sessions/<uuid>`, `refs/handoffs/<uuid>`, `None` for `Commit`), `api_name() -> String` (`main`, `origin/main`, `<tag>`, `<session uuid>`, `<handoff uuid>`, `<sha>`), and predicates `is_mutation_target()` (Head, Session), `is_push_source()` (Head, Session), `is_merge_source()` (Head, Upstream, Session, Handoff, Commit), `is_base()` (Head, Upstream).
- [ ] `GitRef::parse(input: &str) -> Result<GitRef, GitError::InvalidRef>` with this precedence: fully qualified `refs/heads/`, `refs/remotes/origin/`, `refs/tags/`, `refs/sessions/`, `refs/handoffs/` prefixes first; then a UUID -> `Session`; then 40 lowercase hex -> `Commit`; then `origin/<name>` -> `Upstream`; otherwise `Head`. Tags are only reachable by fully qualified name or by `resolve` fallback (below). Names must pass `git check-ref-format --branch`-equivalent rules (no `..`, no leading `-`, no control chars, no `@{`, no trailing `.lock`, no `refs/` re-nesting); reject with `InvalidRef`.
- [ ] `git::refs::resolve(mirror: &Path, r: &GitRef) -> Result<ResolvedRef { git_ref: GitRef, commit: String }>` runs `git rev-parse --verify --end-of-options <full_name>^{commit}` (or `<sha>^{commit}` for `Commit`); for a bare `Head` name that does not exist it retries as `Tag` before returning `UnknownRef`; every returned commit is a full 40-hex id.
- [ ] `git::refs::list(mirror, patterns: &[&str]) -> Vec<RefEntry { full_name, commit, committer_date: DateTime<Utc> }>` via `git for-each-ref --format='%(refname)%00%(objectname)%00%(committerdate:iso-strict)' <patterns>`; `to_branch(&RefEntry) -> Option<Branch>` maps the three API kinds (`head`, `upstream`, `session` with `session_id`) and skips tags and hand-off refs.
- [ ] `git::refs::update(mirror, full_name, new_commit, expected_old: Option<&str>)` wraps `git update-ref <name> <new> [<old>]`; `delete(mirror, full_name)` wraps `git update-ref -d <name>`; both refuse names outside the five namespaces.
- [ ] `git::refs::retain_handoff(mirror, handoff_id, commit)` verifies `commit` is a commit object present in the repository (`rev-parse --verify <sha>^{commit}` equal to `sha`) then `update-ref refs/handoffs/<id> <sha>`; `remove_handoff(mirror, handoff_id)` deletes it (idempotent: missing ref is `Ok`); `list_handoffs(mirror) -> Vec<(Uuid, String)>` for orphan cleanup.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- File: `orchestrator/src/git/refs.rs`. `Branch` DTO type lives in `orchestrator/src/models/git.rs` (serde, `kind` serialised as `head|upstream|session`) so routes in other epics can reuse it.
- Use `--end-of-options` on `rev-parse` so a name starting with `-` cannot become an option; `GitRef::parse` already rejects such names but defence in depth is cheap.
- `for-each-ref` output is NUL-separated per field and newline-separated per ref; parse strictly and skip malformed lines with a `warn!`.
- Ambiguity rule to document on `GitRef::parse` (doc comment): a session id always wins over a branch literally named like a UUID; a 40-hex string is a commit even if a branch has that name; fully qualified names disambiguate.
- `Commit` values are validated `^[0-9a-f]{40}$` (SHA-1 only in v1; the mirror is created by `git init --bare` with the default object format).

## Edge cases
- `origin/HEAD` is not a branch: `list` excludes `refs/remotes/origin/HEAD`.
- Refs pointing at non-commits (annotated tag objects): `resolve` peels with `^{commit}`; `list` reports the tag object id, which is why `Branch` excludes tags.
- A `Head` name that is also a tag name resolves as the head (documented precedence).
- `update` with `expected_old` failing (ref moved) returns `GitError::Command` with `code: Some(1)`; callers under the project lock treat it as an invariant failure, not a user conflict.

## Testing
- Unit tests for `parse`: each documented input form, fully qualified forms, invalid names (`..`, `-x`, `a.lock`, empty, `refs/foo`), UUID vs head precedence, 40-hex vs head precedence, `origin/` prefix.
- Repository tests with a real bare repository in a `tempfile::tempdir()` (helper `tests/common/git.rs` or a `git::testutil` module behind `cfg(test)`: create an upstream bare repo, commit into it with a throwaway work clone, and return its path): `resolve` for head, upstream, tag, session, hand-off and commit; `UnknownRef` for a missing name; `list` returns the three kinds with `session_id` set; `retain_handoff` refuses a non-commit sha and is idempotent on `remove_handoff`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- none beyond the command wrapper task in this epic.