# 0052. The dispatcher waits for a task's author to land its work on the default branch

Status: accepted (Bears epic `4txx2`, task `zpsh4`).

## Context

A conversational planner session wrote planning documents and filed tasks against them over MCP, but its commits stayed on `session/<sid>`. The dispatcher launched implementers for those tasks from the default branch, which did not have the documents, and the work they produced, reviewed and auto-merged was built on the wrong base. Nothing in the tracker connected a task with the unmerged work it was written against, although `tasks.created_by_session_id` already records which session filed it.

## Decision

Before launching for a candidate whose `created_by_session_id` is set, the dispatcher asks whether that session's work has landed, and skips the task with the reason `author_work_unlanded` when it has not. The work has landed when the author column is NULL (no session filed the task, or it was deleted), when the session has no work tree and no ref, when its tip is its recorded `base_commit`, or when its tip is contained in the project's default integration head. A tip held only by a hand-off ref has not landed: a session launched from the default branch would not have it. The check is a silent fetch-back under the project git lock (`GitService::author_work_landed`), asked at most once per author per run, and a failure holds the task back and counts as a failure.

Only the dispatcher applies it. A person launching a task, an agent claiming one over MCP and the auto-merge job are not gated: a person decides for themselves, an agent claiming over MCP was pointed at the task by someone, and a merge is not a launch base.

Rejected:

- **Prompting only at session end** to merge the session's commits. Conversational sessions usually park rather than end, and dispatch happens while the planner is still open, so the prompt comes after the damage. (The End confirmation does say so as well, epic `4txx2`; it is a courtesy, not the guard.)
- **Holding auto-merge while other branches are ahead of the default branch.** Almost every live session is ahead of it, so the pipeline would stall; and it acts at the wrong point, because the damage is the launch base, not the merge.

## Consequences

- A planner's tasks sit in their queue until the planner's branch (or a hand-off of it) is merged into the default branch. The board is told why (epic `4txx2`).
- A plain branch merge of the author's session writes only a `git` event on that session, which does not wake the dispatcher, so such a task is picked up by the next timer run, within `DISPATCHER_INTERVAL_SECS`. A task merge writes task events and wakes it at once.
- A planner that keeps committing after its first merge holds its tasks back again until the new commits land too: the question is about the session's current tip.
