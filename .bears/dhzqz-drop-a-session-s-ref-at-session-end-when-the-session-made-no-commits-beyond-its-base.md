---
id: dhzqz
title: Drop a session's ref at session end when the session made no commits beyond its base
status: open
priority: P2
created: "2026-09-24T08:17:00.445343Z"
updated: "2026-09-24T08:17:00.445343Z"
tags:
  - orchestrator
  - frontend
  - git
  - docs
depends_on:
  - g3qdk
parent: kc8k3
---

Part of epic kc8k3; read its decision first. A planner, or a reviewer that forwards a hand-off without committing, ends with `session/<sid>` exactly at the commit it was cloned from, and the end-of-session fetch-back still writes `refs/sessions/<sid>` there (`orchestrator/src/git/session.rs`, `fetch_back`; called from `session/service.rs` `fetch_back` on `end` and from `session/owner.rs` `fetch_back` when an ephemeral session's `result` arrives). That ref holds nothing and should not exist.

## Scope

1. **Know the base commit.** `sessions.base_ref` holds a branch name unless the launch came from a hand-off (docs/data-model.md, `sessions`), and a branch may have moved since, so the base *commit* is not recorded today. Record it: preferably a `base_commit` column set from `resolve_base` at launch (a migration, `docs/data-model.md`, `.sqlx/`, and the `Session` shape in SPEC.md if exposed). Reading it from the work clone's reflog is the alternative; it doesn't survive a missing reflog, so choose it only with a reason written down. Rows launched before the migration have no value, and for them the rule below does nothing.
2. **End-of-session rule.** In the fetch-back that ends a session (`done`, or `failed`), under the project git lock it already holds: if the work clone's `session/<sid>` tip equals the session's base commit, do not create `refs/sessions/<sid>`, and delete it (`update-ref -d`, the helper g3qdk adds) if an earlier sync left one. Mid-session syncs (`POST /sessions/{id}/sync`, the diff's silent sync, hand-off publication) are unchanged: a live session keeps its ref whatever it holds.
3. **Nothing is lost.** The work clone stays until the session is deleted, and a later sync (a REST hand-off naming this session, a retry of a `failed` conversational session) fetches back from it again. A resumed session gets its ref back on its next fetch-back. Check that `session has no synced branch yet` is still the right answer for a REST hand-off naming such a session, or that it syncs first; document which.
4. **Readers of a missing ref.** Check every reader of a session ref for an ended session that now has none: the session view's "Changes" panel (diff with a session `head`), `Session.branch`, the branch and session-branch listings, and a launch with that session as `base_ref`. Each gives a defined answer (an empty diff against the base, the branch omitted from lists, a 400/404 with an exact message for the base) rather than a 500. The frontend shows it as "no changes" and not as a read failure.

## Documentation (rule 1)

- ARCHITECTURE.md: "Git model", Ref ownership (the session ref's lifetime) and the fetch-back on ending a session ("Session lifecycle", the paragraph on ending).
- SPEC.md: the `POST /sessions/{id}/end` answer and the `SessionBranch`/`Branch` listings where they describe which sessions appear; the `Session` shape if `base_commit` is exposed.
- docs/data-model.md: the new column.
- New ADR in `docs/decisions/`: session refs stay one per session and are kept only while they hold work no other retained ref holds; branches one per task rejected, with the reasons in epic kc8k3's decision. This task writes the ADR for the whole epic's rule, and the next task implements its second half.

## Tests

- Integration (real bare repositories, git never mocked): a session that ends without committing leaves no `refs/sessions/<sid>`; one that committed keeps it; one synced mid-session and then ended without further commits past its base loses the ref; a session whose base branch moved after launch is judged against its recorded base commit, not the branch's current tip.
- The diff and listing readers answer as documented for an ended session with no ref.
- `tests/session_e2e.rs` if the end path on real containers needs it.