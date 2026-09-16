---
id: "2yeth"
title: "Write hand-off E2E specs: revision hand-off, review approval, changes requested, task merge and revision diff"
status: open
priority: P1
created: "2026-09-16T20:47:33.563517604Z"
updated: "2026-09-16T20:47:33.563517604Z"
tags:
  - frontend
  - tracker
  - git
  - tests
depends_on:
  - sey9x
  - nvjt5
parent: "6s8j7"
---

## Summary
Cover "Code hand-offs and review" from the user's side: an implementer session commits (host-side commit into its work clone), the user publishes a revision hand-off from the task drawer with the exact commit and a comment moving the task to `review`; a reviewer opens the task in a session that defaults to the hand-off commit, forwards it to `merge` with `approved`; the task's merge action, enabled only for an approved current hand-off, merges the pinned commit even after the session branch moved on; a `changes_requested` path sends the task back; a new revision resets review; the revision diff is viewed by `handoff_id`.

## Documents
- `README.md` "Start" (hand-offs keep session, branch, commit and comment together; next session defaults to the handed-over commit; approval belongs to a commit; the merge action merges the approved revision even if the branch changed).
- `SPEC.md` "Code hand-offs and review" (`HandoffInput` revision `{kind: "revision", source_session_id, commit, comment}` and forward `{kind: "forward", handoff_id, comment, review?}`; requires a different target `state` in the same `PUT` (400 otherwise) and a non-empty comment; REST revision requires `source_session_id` of the project; `commit` is a full object id and sync must produce that tip or 409; forwarding requires `handoff_id` to equal the current hand-off (409); a new revision resets `review_status` to `unreviewed`; `Handoff = { id, task_id, source_session_id, source_branch, commit, comment_id, review_status, reviewed_by_user_id, reviewed_by_session_id, reviewed_at, created_by_user_id, created_by_session_id, created_at }`; example flow A → review → approved → merge uses A even if the branch is at B).
- `SPEC.md` "Git" (`POST .../merge` with `{task_id, handoff_id, target}` → `{commit}`; 409 for a stale or unapproved hand-off; `GET .../diff?handoff_id=&base=` selects the retained commit without syncing; `head` and `handoff_id` are mutually exclusive, 400).
- `SPEC.md` "Sessions" (with `task_id` and no `base_ref`, `handoff_id` and the full commit are recorded on the session; the generated message includes the hand-off id, source branch, commit, review status and comment, also when an explicit base overrides it).
- `SPEC.md` "Frontend", "Hand-off controls" (current source session/branch, pinned commit, comment, review status, history; revision form selects source session and exact commit and requires a comment; review actions forward the current id with `approved`/`changes_requested`; neither silently selects the reviewer's own branch; "Open in session" and "Run once" default to the hand-off commit and disclose base overrides; merge action sends `task_id` and `handoff_id` and is enabled only for an approved current hand-off; approval is labelled with the commit; new revisions appear unreviewed).
- `ARCHITECTURE.md` "Git model" (`refs/handoffs/<id>` retains the commit; task merge under the project git lock verifies current and approved hand-off).
- ADR 0018.

## Acceptance criteria
- [ ] `frontend/tests/handoffs.spec.ts`; user U1 (implementer), user U2 (reviewer), bare repo, ready project, task `Add greeting` in `ready`; `test.setTimeout(240_000)`; sessions ended in `afterEach`.
- [ ] `publish a revision hand-off to review`: U1 opens the task in a session S1 (claims it); `A = commitInSessionWorkClone(S1, { "greeting.txt": "hello\n" }, "feat: greeting")`; in the drawer's hand-off form choose source session S1, commit `A` (the form may offer the synced tip after Sync; the test enters the full id), comment `Ready for review`, target `review`; submit → task in `review`, drawer shows source S1, branch `session/<S1>`, commit `A` (short and full), review status `unreviewed`, the comment in the comments list; the lease is cleared; `git -C mirror rev-parse refs/handoffs/<handoff id>` equals `A`; a hand-off without a comment or without a state change shows the 400 error.
- [ ] `reviewer session defaults to the hand-off commit`: U2 (second context) opens the task → "Open in session" shows base `A` with the hand-off summary and no override notice; launching gives a session S2 whose header shows `base_ref` = `A` and whose transcript's generated first message contains the hand-off id, `session/<S1>`, `A`, `unreviewed` and `Ready for review`; choosing base `main` instead shows the override disclosure and the message says so.
- [ ] `approve and forward to merge`: in U2's drawer click Approve with comment `LGTM` → task in `merge`, review status `approved` labelled with commit `A`, `reviewed_by_user_id` U2 shown by name; the merge action is now enabled.
- [ ] `task merge uses the pinned commit even after the branch advanced`: `B = commitInSessionWorkClone(S1, { "greeting.txt": "hello again\n" }, "more")`; sync S1 (so `refs/sessions/<S1>` is at `B`); click the task's merge action (target `main`) → success; `gitIsAncestor(mirror, A, "main")` is true and `gitIsAncestor(mirror, B, "main")` is false; the task's drawer shows the merge outcome and the task can be moved to `done`.
- [ ] `merge is disabled without approval and refused when stale`: new task with an unreviewed revision → merge control disabled; publish a second revision `C` on that task (form again) → review status `unreviewed`, history lists both; forward with `approved` using a stale id through the helper API (`PUT` with the first hand-off's id) → 409 shown/returned; the UI's approve uses the current id and succeeds.
- [ ] `changes requested sends the task back`: reviewer forwards with `changes_requested`, comment `Please rename`, target `ready` → task in `ready`, status `changes_requested` on commit `A`; "Open in session" still defaults to `A`; a new revision `D` from a new implementer session resets status to `unreviewed`.
- [ ] `revision diff by handoff_id`: the drawer's revision view fetches `diff?handoff_id=<id>` and lists `greeting.txt` (added, +1); the transcript of S1 shows no additional `git` event from that view (no sync).
- [ ] `sync mismatch is rejected`: enter a commit id that is not the work clone's tip (a made-up 40-hex) → 409 message; the task stays in `ready` with its lease.

## Implementation notes
- Files: `frontend/tests/handoffs.spec.ts`.
- Two users require two contexts (`newLoggedInPage`); the task board's live refresh means U2's drawer reflects U1's hand-off without reload, which the test also asserts once.
- Commit ids are obtained from `commitInSessionWorkClone`; the hand-off form may run Sync itself (the SPEC says sync must produce the exact tip); either way the mirror's `refs/sessions/<S1>` after publication equals the entered commit.
- The `Requested-By` trailer on the merge commit is asserted in the git spec; here only ancestry.
- Actor names in the drawer come from `GET /users/{id}`; assert by username.

## Edge cases
- Approve with an empty comment must be refused by the form (the API requires a non-empty comment); assert the inline validation.
- The merge control must remain disabled after a new revision on an approved task (status reset to `unreviewed`).
- Session S1 remains running while U1's task is in `review` (the lease was cleared by the hand-off, the session is not ended); `afterEach` ends it.

## Testing
- The spec file; run twice against one stack.
- Command: `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:e2e`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Frontend task board, task detail and hand-off controls": hand-off form, review actions, merge action gating, revision diff view, base-override disclosure in the launch dialog.
- "Code hand-offs and review": `handoff` on `PUT /tasks/{id}`, `refs/handoffs/<id>`, task-form merge and `diff?handoff_id=`; "Session lifecycle": hand-off default base and generated message.