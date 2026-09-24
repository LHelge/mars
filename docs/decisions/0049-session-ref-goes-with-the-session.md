# 0049. A session's branch ref is deleted with the session; no separate branch-delete action

Status: accepted (Bears task `g3qdk`, epic `kc8k3`).

## Context

`DELETE /sessions/{id}` removed the session directory, the CLI transcript and the row, but left `refs/sessions/<sid>` in the project repository. Nothing else removed it, so every session that was started, ended and deleted left a branch in `GET /projects/{pid}/git/session-branches` whose `session_id` named nothing, and the Branches tab listed it for good with no way to get rid of it.

## Decision

The session ref belongs to the session and goes with it. Deleting a session takes the project git lock before any database lock, deletes `refs/sessions/<sid>` with `git update-ref -d`, then deletes the row in a transaction that commits before the lock is released. A ref that is not there — a session that never synced, a project with no repository yet — is not an error. Any other git failure fails the delete with the row still in place, so the next delete retries, as a failed directory removal already did.

The hourly orphan cleanup job sweeps `refs/sessions/*` under each project's git lock, beside `refs/handoffs/*`, and removes a session ref with no session row of that project at once. That removes the refs of sessions deleted before this change, a ref whose row delete failed after the ref went, and a ref written back by a sync that read the row before the deletion and wrote after it. Unlike a hand-off ref it needs no grace or second sighting: a session's row is committed before anything can write its ref, so a ref without a row is always a deleted session's.

The session delete confirmation warns, without refusing, when the branch has commits not on the default branch (`SessionBranch.ahead > 0`), naming the count and the base. It reads this from the project's existing session-branch list, the cached read the Branches tab already shares, rather than from a field on the delete endpoint: the list already carries `ahead` measured against the default branch, the confirmation is the only reader, and a delete endpoint answering a warning would need either a second request shape (a dry run) or a 409 that is not a refusal.

Branches pushed upstream are not touched. Mars only pushes, and never deletes a remote branch as a side effect.

Rejected: **keeping the ref and adding an explicit "delete branch" action** on the Branches tab. The ref is named by a session that no longer exists, and deleting a session is already the explicit, final action, offered only on `done` or `failed`. Work worth keeping has other homes by then: an integration head, an upstream push, or a hand-off, whose `refs/handoffs/<id>` holds its commit independently of the session ref (ADR 0018). A second action would leave every existing leftover in place until someone clicked it, and would add a mutation on a namespace whose owner is already gone.

## Consequences

- `refs/sessions/<sid>` lives exactly as long as its session (`ARCHITECTURE.md`, "Git model", Ref ownership), and `GET /projects/{pid}/git/session-branches` names only sessions that exist, once the first cleanup run has removed older leftovers.
- A deleted session's commits stay in the repository's object store (`gc.auto=0`); they are reachable afterwards only through a hand-off ref, an integration head that merged them, or an id someone kept.
- Deleting a session now waits for any git operation in progress on its project.
