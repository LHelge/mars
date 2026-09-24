# 0050. Session refs stay one per session and are kept only while they hold work no other retained ref holds

Status: accepted (epic `kc8k3`; the first half in Bears task `dhzqz`, the second in `ahv3c`). Builds on ADR 0049, which ties the ref's longest lifetime to its session's.

## Context

Every session that ended left `refs/sessions/<sid>` in the project repository, written by the fetch-back that ends it. A task that goes planner → implementer → reviewer left three: the planner's at the commit it was cloned from, holding nothing it did; the implementer's with the work; and the reviewer's at the hand-off commit it forwarded. Only the hand-off (`refs/handoffs/<id>`, ADR 0018) is what review and the task merge use. The other two were clutter in `GET /projects/{pid}/git/session-branches`, the Branches tab and every branch picker, and ADR 0049's deletion only removes them when somebody deletes the session.

Two ways out were considered.

1. **One branch per task**, `refs/tasks/<n>` or similar, written by every session that works on the task. Rejected:
   - A session ref is one writer's working ref. A task has several writers over its life — an implementer, a reviewer, a second implementer after `changes_requested`, a retry — and fetch-back force-updates the ref so that an agent that rewrote its own history is followed rather than refused. A ref shared by the task's writers would let a later writer overwrite the work that was reviewed: exactly the moving branch behind an approval that option 2 of ADR 0018 rejected.
   - Sessions with no task (scheduled runs, ad-hoc launches) and sessions holding several tasks have no single task to name a branch after, so session refs would have to stay beside task refs anyway.
   - The per-task branch already exists: it is the hand-off chain. Each revision is pinned under its own `refs/handoffs/<id>`, and the next session for the task launches from the current one by default (`ARCHITECTURE.md`, "Task tracker"). A task-named ref would be a second, mutable copy of it.
2. **Keep one ref per session and shorten its life.** Chosen.

## Decision

Branches stay one per session. A session ref is kept only while it holds commits no other retained ref holds, and it goes when its session goes (ADR 0049).

- **A live session keeps its ref whatever it holds.** Syncs while it runs — `POST /sessions/{id}/sync`, the diff's silent sync, hand-off publication — are unchanged.
- **At the end, a session with no commits beyond its base keeps no ref** (this task). The launch records the commit the work clone was created at as `sessions.base_commit`, because `base_ref` is a name and a branch may have moved since. The fetch-back of a session that is ending, or has ended (`done` or `failed`), compares the work clone's `session/<sid>` tip with that commit under the project git lock; when they are equal it writes no `refs/sessions/<sid>` and deletes one an earlier sync left. A row launched before the column has no value, and for it the rule does nothing.
- **At the end, a session whose tip another retained ref already holds keeps no ref** (task `ahv3c`): a tip contained in — equal to, or an ancestor of — a hand-off ref or an integration head is kept by that ref, since hand-off refs go only with their task or project and integration heads only move forward. It is one `git for-each-ref --contains` over `refs/heads/` and `refs/handoffs/`, under the same lock, before the fetch. Upstream-tracking refs and tags do not count: Mars does not own them and a fetch can prune them (ADR 0017). A session with no recorded base commit is judged by this half alone.
- **A kept ref is not chased.** A ref kept at the end because it held unique work is not re-examined when that work later lands on a head or in a hand-off: the hourly orphan cleanup stays a job about orphans (refs with no session row, ADR 0049) and does not apply this rule to the refs of ended sessions. Extending it was considered and rejected: it would add a containment query per ended session per project every hour to reclaim refs that go with their session anyway, and it would make a session's ref vanish while someone may be using it by name. A later fetch-back of the ended session — an explicit sync, the diff, a hand-off naming it — does judge it again, because every fetch-back of an ended session applies the rule.
- **Nothing is lost.** The work clone stays until the session is deleted. Every later fetch-back reads it again: an ended session's is judged by the same rule, and a `failed` conversational session that is retried is live again, so its next fetch-back writes the ref. The fetch-back answers the branch's tip whether or not a ref is kept, and the readers that need the commit take that answer — the diff, and a revision hand-off naming the session, which pins that commit under its own ref instead of refusing with `session has no synced branch yet`. Readers that look the ref up by name (the two listings, a launch or merge naming the session) see no ref, as for any session that never synced.

Rejected alongside option 1: **reading the base from the work clone's reflog** instead of recording it. The reflog is agent-controlled, expires, and is absent after anything that recreated the branch, so the rule would silently stop applying; a column written by the orchestrator at the moment it made the clone does not.

Task-named branches for people, if they are wanted later, belong to the upstream push — pushing a hand-off to something like `mars/task-<n>` — and not to the internal ref model.

## Consequences

- `GET /projects/{pid}/git/session-branches`, `GET /projects/{pid}/branches` and the Branches tab list the sessions that are live or that ended holding their own work, not every session that ever ran.
- `sessions` gains `base_commit` (`docs/data-model.md`). It is not part of the `Session` API shape.
- An ended session with no ref answers a diff at its work clone's tip — empty against the branch it started from when it made no commits, its work when a hand-off holds it — is absent from both listings, and is a 400 `base_ref does not resolve` as a launch's base. The session view's Changes panel says in one line that it has no branch of its own; its work is reached through the hand-off or the head that holds it.
- A sync of such a session answers `{ref, commit}` with `ref` naming a ref that does not exist; the `git` event says the same. A nullable `ref` was not introduced, because every client reads `commit` and the case is one nobody acts on.
