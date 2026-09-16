# 0018. Task hand-offs retain a commit; review approval belongs to that commit

Status: accepted. Extends ADRs 0016 and 0017 without changing project-defined task states or reference clones.

## Context

Moving a task to review changes who works on it but does not identify the code to review. The producing session may still be running with an unsynced branch, and a moving branch can gain unreviewed commits after approval.

Options considered:

1. Name branches and commits in comments. Rejected: the launcher cannot select the revision reliably.
2. Store a source branch and always use its latest tip. Rejected: later edits silently change what an approval covers.
3. Publish a retained commit with provenance and a required comment; launch the next session from it and bind approval to it. Chosen.

## Decision

- A state update may publish a revision hand-off or forward the current one. Planning-only moves need no code; state names do not imply approval.
- Publication names a source session, commit and comment. The orchestrator pins the commit at `refs/handoffs/<id>` under the project git lock, then writes the hand-off, state, lease release and events in one database transaction.
- A review records `approved` or `changes_requested` against that commit; a new revision starts `unreviewed`.
- Task-bound launch defaults to the current hand-off's commit. Task merge requires approval and merges exactly the pinned commit; later branch edits need a new revision and review.
- Hand-off refs outlive the source session and are removed with the task or project.

## Consequences

- Reviewers and later implementers start from the code passed to them.
- Approval of commit A does not approve commit B, including a rebased A. Previous revisions stay inspectable.
- The schema gains `task_handoffs`, a current pointer on tasks and a launch pointer on sessions.
