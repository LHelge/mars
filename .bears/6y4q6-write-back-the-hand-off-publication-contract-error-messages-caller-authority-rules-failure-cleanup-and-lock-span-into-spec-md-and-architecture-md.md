---
id: "6y4q6"
title: "Write back the hand-off publication contract: error messages, caller authority rules, failure cleanup and lock span into SPEC.md and ARCHITECTURE.md"
status: open
priority: P3
created: "2026-09-16T20:44:51.876909612Z"
updated: "2026-09-16T20:44:51.876909612Z"
tags:
  - docs
  - tracker
  - git
depends_on:
  - dgxzq
  - x55fb
  - rz6bj
  - bq5mf
parent: xjaah
---

## Summary
Once the code paths exist, reconcile the documents with what was implemented so the next reader (and the MCP and Frontend epics) can rely on exact strings and rules: the 400/409 messages of hand-off publication and task merge, that REST users are not lease-bound while MCP callers must hold the lease, that the git lock is held through the tracker commit, what happens to a pinned ref when the database step fails, and which sessions get `task_sessions` links on a revision versus a forward. Only sections already describing these behaviours are edited; no new rules are invented here.

## Documents
- `SPEC.md` "Code hand-offs and review" (messages and authority rules), "Git" (task-merge 409 messages and default merge message), "MCP tool contracts" -> `update` and `merge` (cross-reference to the same messages as `conflict` / `invalid_argument` text).
- `ARCHITECTURE.md` "Task tracker" -> "Code hand-offs" (lock span through commit; discard of the pinned ref on database failure; orphan cleanup as backstop), "Git model" -> "Serialization" (hand-off publication holds the lock through the tracker commit).
- `docs/data-model.md` `task_sessions` (which session is linked on revision by a user, revision by a session, forward by a session), `task_handoffs` (carried decision without reviewer after deletion).
- `CLAUDE.md` rule 1 (documentation is part of the change) and `docs/open-questions.md` (confirm no hand-off entry remains; none exists today).

## Acceptance criteria
- [ ] `SPEC.md` "Code hand-offs and review" lists the exact 400 messages (`handoff requires a different target state`, `handoff comment must not be empty` or the model's `EmptyComment` text, `revision hand-off requires source_session_id`, `source_session_id is derived from the calling session`, `source_session_id must name a session of this project`, `commit must be a full commit id`, `review applies to forward hand-offs only`) and 409 messages (`session branch tip <tip> does not match commit <commit>`, `handoff_id is not the task's current hand-off`, `task is not held by the calling session`, `task changed during hand-off publication; re-read it and retry`, `commit is not present in the project repository`, `project is not ready`), matching the code byte for byte (a test in the API suite asserts at least the two 409 texts).
- [ ] `SPEC.md` "Git" lists `handoff_id is not the task's current hand-off` and `hand-off is not approved` for the task merge form and the default message `Merge handoff <id> (<source_branch>) into <target>`.
- [ ] `SPEC.md` states in one sentence each: a REST user may publish for any task regardless of lease and name any project session with a synced branch as the source; an MCP caller must hold the lease and is always the source.
- [ ] `ARCHITECTURE.md` "Code hand-offs" states that the git lock is held from preparation through the tracker transaction's commit and that a failed database step removes the just-pinned ref (best effort), with orphan cleanup as the backstop.
- [ ] `docs/data-model.md` `task_sessions` names the linked sessions per hand-off kind; `task_handoffs` notes the carried-decision-without-reviewer case if the model rule was relaxed.
- [ ] No behaviour changes in this task; if a mismatch between code and document is found, the document is corrected when the code follows the epic's contract, otherwise a new task is filed under this epic and linked here.

## Implementation notes
- Edit only the sections named above; keep the ADR untouched (ADR 0018 records the decision, not the messages).
- Keep the wording in the same register as the surrounding text (present tense, one sentence per rule).

## Edge cases
- If the tracker epic chose different unknown-state or lease messages, cite theirs rather than introducing variants.

## Testing
- No code; `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` still passes (the message-assertion test from the API suite must agree with the document).

## Documentation
- This task is the documentation change.

## Assumes from other epics
- none.