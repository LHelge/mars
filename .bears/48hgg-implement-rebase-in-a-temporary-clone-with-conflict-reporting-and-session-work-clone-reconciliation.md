---
id: "48hgg"
title: Implement rebase in a temporary clone with conflict reporting and session work-clone reconciliation
status: open
priority: P1
created: "2026-09-16T20:31:20.730476390Z"
updated: "2026-09-18T00:50:39.712211670Z"
tags:
  - orchestrator
  - git
depends_on:
  - vztkg
  - "7q4qt"
parent: z4u4e
---

## Summary
Implement the rebase primitive: rebase a session ref or integration head onto an integration or upstream ref inside a temporary shared clone, write back only the rewritten ref with an explicit (forced) refspec, report conflicts with paths, and, for a session branch, reconcile the session's work clone afterwards (`fetch` then `reset --hard` only when the work tree is clean, otherwise report that reconciliation is required). Behind `POST /projects/{pid}/git/rebase` and the MCP `rebase` tool.

## Documents
- `ARCHITECTURE.md` "Git model" -> "Merge, rebase, push" (temp clone; after a successful rebase of a session branch update the work clone: `git fetch origin refs/sessions/<sid>` then `git reset --hard` only if clean, otherwise the `git` outcome event reports that checkout reconciliation is required)
- `ARCHITECTURE.md` "Known v1 vulnerability" and ADR 0019 (commands on the agent-controlled checkout)
- `SPEC.md` "Git" (`POST .../rebase {branch, onto}` -> `{commit}` or 422; `branch` accepts session id or integration name; `onto` accepts integration or upstream names; an upstream ref may be `onto`, never the branch being rewritten -> 400)
- `SPEC.md` "MCP tool contracts" -> `rebase` ("If `branch` is the calling session's branch, the session work tree is updated afterwards when clean")
- ADR 0007 ("a `rebase` request brings a session branch up to date and the orchestrator fetches the result back into the session work tree")

## Acceptance criteria
- [ ] `git::integrate::rebase(guard, paths, project_id, branch: &ResolvedRef, onto: &ResolvedRef, identity: &CommitIdentity) -> Result<RebaseOutcome { commit: String, work_tree: WorkTreeOutcome }, GitError>` where `WorkTreeOutcome` is `Updated | ReconciliationRequired | NotApplicable`; `branch.git_ref` must be `Head(_)` or `Session(_)` (else `InvalidRef`), `onto.git_ref` must be `Head(_)` or `Upstream(_)` (else `InvalidRef`).
- [ ] Steps: temp clone (`TempClone` from the merge task); `git fetch origin +<branch_full>:refs/tmp/branch +<onto_full>:refs/tmp/onto`; `git checkout -B work refs/tmp/branch`; `git rebase refs/tmp/onto` with committer identity from `identity` (author preserved by git); on success `git push origin +work:<branch_full>` (forced: a rebase rewrites) and return the new tip; on conflict collect `git diff --name-only --diff-filter=U`, `git rebase --abort`, drop the temp clone, return `GitError::Conflict { paths }`.
- [ ] For `branch = Session(sid)` after a successful write-back: if `DATA_DIR/sessions/<sid>/work` is missing -> `NotApplicable`; else `git -C <work> fetch origin +refs/sessions/<sid>:refs/sessions/<sid>`; then `git -C <work> status --porcelain --untracked-files=no`; empty output and current branch `session/<sid>` -> `git -C <work> reset --hard refs/sessions/<sid>` -> `Updated`; otherwise `ReconciliationRequired` (no reset). For `Head(_)` -> `NotApplicable`.
- [ ] Reconciliation failures (fetch or reset error in the work clone) do not undo the mirror write-back; they yield `ReconciliationRequired` and a `warn!` with `session_id`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- File: `orchestrator/src/git/integrate.rs` (extend).
- Use `git rebase --no-autosquash --no-autostash` explicitly; interactive/`exec` features are never used; pass `-c rebase.backend=merge` is unnecessary (default).
- `GIT_COMMITTER_NAME/EMAIL` from the bot identity so rewritten commits are visibly re-committed by Mars while authorship stays with the user/agent.
- `status --porcelain --untracked-files=no`: untracked files do not block a reset (they survive `reset --hard`), which matches the intent of "clean"; document this choice in the doc comment.
- The work-clone commands run against an agent-controlled repository; put the ADR 0019 reference on the function.
- `RebaseOutcome.work_tree` becomes `detail.work_tree` (`"updated" | "reconciliation_required" | "not_applicable"`) in the `git` event emitted by the service task.

## Edge cases
- Already up to date (`branch` already based on `onto`): `git rebase` exits 0 without changes; return the unchanged tip with `work_tree: Updated` if applicable (the fetch is a no-op and the reset is harmless when clean).
- Rebasing an integration head that has an unpushed merge onto `origin/<b>`: allowed; the head is rewritten (this is the documented explicit way to linearise before push).
- `branch == onto` or `onto` is a descendant of `branch`: rebase fast-forwards `work` to `onto`; write back accordingly (git semantics), return the new tip.
- Work clone HEAD not on `session/<sid>` (agent checked out something else): `ReconciliationRequired`.
- A running container may be committing during reconciliation; the status check is a best-effort snapshot (documented; the lock does not cover the agent's checkout).

## Testing
- Real repositories: mirror `main` advanced by an upstream fetch after session A branched; session A has two commits; `rebase(Session(A), Upstream(main))` -> `refs/sessions/A` rewritten with `onto` as ancestor, both commit authors preserved, committer is bot; with a clean work clone `work_tree == Updated` and `git -C work rev-parse HEAD` equals the new tip; with a dirty work clone (uncommitted modification) `ReconciliationRequired` and the work HEAD unchanged; with the work dir removed `NotApplicable`.
- Conflict: `Conflict { paths }`, `refs/sessions/A` unchanged, temp dir gone, work clone untouched.
- Invalid: `branch = Upstream(main)` -> `InvalidRef`; `onto = Session(x)` -> `InvalidRef`.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- none.