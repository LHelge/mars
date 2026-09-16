# 0030. Record tracker changes, not read-only tool calls

Status: accepted.

## Context

The architecture says every MCP call produces an event, but the tracker schema describes changes and provides no event kinds for reads. Recording task reads as work would also populate session-task history with sessions that only inspected the task.

## Decision

Successful tracker changes write their events and session-task links in the same transaction. Read-only calls such as `ready` and `get_task` do not write tracker events, create links or advance touch timestamps. Rejected operations and updates with no effective change have no tracker-history side effects.

The read-only `list_session_branches` tool emits no session `git` event. Existing git-operation outcome events remain separate from tracker history. Backend-reported tool calls and results may still appear in the session transcript; this decision does not suppress those records.

Do not introduce a separate persistent audit log for MCP reads in v1.

## Consequences

Task history reflects changes and actual session involvement, rather than inspection traffic. Repeated reads do not trigger board refreshes or make a task look recently worked on. Acceptance covers repeated reads with unchanged event counts and touch timestamps, rejected/no-op updates, and successful mutations committing their events and links together.
