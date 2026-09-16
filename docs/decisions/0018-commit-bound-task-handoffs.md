# 0018. Task hand-offs retain a commit; review approval belongs to that commit

Status: accepted. Extends ADRs 0016 and 0017 without changing project-defined task states or reference clones.

## Context

Moving a task from implementation to review changes who should work on it, but does not identify the code to review. The producing conversational session may remain running or parked, so its branch may not yet be synced. A new reviewer session previously defaulted to the project's integration branch. Comments and session links alone cannot reliably identify the intended revision, and a moving branch can gain unreviewed commits after approval.

Options considered:

1. Rely on comments to name branches and commits. Rejected: each agent must reconstruct the hand-off and the launcher cannot select the right revision reliably.
2. Store a source branch only and always use its latest tip. Rejected: later edits silently change what a review or approval covers.
3. Publish a retained commit with source provenance and a required comment; launch the next session from it and bind approval to it. Chosen.

## Decision

- A state update may publish a revision hand-off or forward the current one. Planning-only state moves need no code; state names remain user-defined and do not imply approval.
- Revision publication supplies a source session, full commit id and comment. MCP derives the source session from its bearer token. The orchestrator syncs the branch, checks the expected commit, and pins it at `refs/handoffs/<id>` before publishing the database change.
- The hand-off record, comment, task state, lease release, current-hand-off pointer and events commit together. Git preparation happens first, under the project git lock; stale task state or failed sync leaves the task unchanged. Orphan refs from an interrupted publication can be cleaned up later.
- Forwarding retains the original source session, branch and commit. An explicit review records `approved` or `changes_requested` with its actor and time. A new revision always starts `unreviewed`; forwarding without a new decision preserves existing review attribution.
- Task-bound launch defaults to the current hand-off's commit, selected atomically with the task claim. An explicit base override remains possible and is visible. An existing session claiming a task receives the hand-off but must explicitly fetch/inspect it; the orchestrator does not reset its checkout.
- The task form of merge takes the task and current hand-off ids, requires approval, and merges exactly the pinned commit. Later source-branch edits do not enter that merge. Including them requires publishing and reviewing another revision. Generic branch operations remain explicit user/profile-authorized actions and do not create task approval.
- Hand-off history and retained refs outlive the source session; they are removed with the task or project.

## Consequences

- Reviewers and subsequent implementers start from the code passed to them rather than reconstructing it from comments.
- Review-only sessions can forward implementation work without substituting their own branches.
- A review/rejection/fix cycle keeps previous revisions and decisions inspectable. Approval of commit A does not approve commit B, including a rebased version of A.
- The schema gains `task_handoffs`, a current pointer on tasks and a launch pointer on sessions. The API and MCP share one hand-off contract.
- Implementation acceptance covers publication while the source session remains alive, source-branch advancement after approval, new-revision approval reset, rejection and retry, stale forwarding, failed sync, a crash between git and database writes, and source-session deletion without loss of handed-over code.
