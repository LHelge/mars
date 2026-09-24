---
id: kc8k3
title: "Session branches: a ref exists only while it holds work nothing else keeps, and branches get their own tab"
type: epic
status: done
priority: P2
created: "2026-09-24T08:16:43.130292Z"
updated: "2026-09-24T12:33:34.719051318Z"
tags:
  - orchestrator
  - frontend
  - git
  - docs
---

Every session that ends leaves `refs/sessions/<sid>` in the project repository (fetch-back in `orchestrator/src/git/session.rs`, `fetch_back`), and nothing ever removes one. A task that runs planner → implementer → reviewer leaves three: the planner's at its base commit with nothing on it, the implementer's with the work, and the reviewer's at the same hand-off commit. Only the hand-off (`refs/handoffs/<id>`, ADR 0018) is what the task review and merge use; the others are clutter in `GET /projects/{pid}/git/session-branches` and the UI's branch lists. That list also sits at the bottom of the project's Sessions tab, under the session table, which makes the tab hard to read.

## Decision (in discussion with the user, 2026-09-24)

Branches stay one per session, not one per task. A session ref is one writer's working ref: a task has several writers over its life (implementer, reviewer, a second implementer after `changes_requested`, a retry), fetch-back force-updates the ref so an agent that rewrote its history is followed, and a shared task ref would therefore let a later writer overwrite the reviewed work: option 2 ADR 0018 rejected, a moving branch behind an approval. Sessions with no task (scheduled runs, ad-hoc launches) and sessions holding several tasks have no single task to name a branch after. The per-task branch already exists as the hand-off chain: each revision pinned, and the next session launched from the current one by default (ARCHITECTURE.md, "Task tracker", "Launching a session for a task").

What changes instead is the session ref's lifetime: it is kept only while it holds commits that no other retained ref (an integration head, a hand-off ref) holds, and it goes when its session goes. Task-named branches for people, if wanted later, belong to the upstream push (pushing a hand-off to e.g. `mars/task-<n>`), not to the internal ref model. That is out of scope here.

Git work also gets its own place in the UI: a `Branches` tab on the project page, separate from the session list.

## Tasks

1. ebk7u: a Branches tab on the project page; the git panel moves off the Sessions tab. Frontend only.
2. vbrmg: push an integration head such as `main` from the console, on the new tab. Depends on ebk7u.
3. g3qdk: delete the session ref with the session, and sweep orphaned session refs. Depends on ebk7u, because both change `SessionsTab.tsx` and `tests/git.spec.ts`.
4. dhzqz: at the end of a session, drop a session ref that has no commits beyond its base. Depends on g3qdk.
5. ahv3c: at the end of a session, drop a session ref whose tip is already held by a hand-off ref or an integration head. Depends on dhzqz.

vbrmg and g3qdk can run in parallel once ebk7u lands. g3qdk, dhzqz and ahv3c run in sequence: all three change the same ARCHITECTURE.md sections ("Git model", Ref ownership, Fetch-back; "Background jobs") and each writes or amends an ADR, so running them in parallel would conflict on the ADR numbering and the prose.