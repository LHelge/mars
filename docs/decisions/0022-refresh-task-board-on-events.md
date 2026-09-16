# 0022. Refresh the task board on events and retain deleted task identities

Status: accepted. Supersedes the task board's event-payload folding strategy in the initial frontend design; session transcript streaming is unchanged.

## Context

The board loads states and tasks over REST and receives changes over SSE. Applying live payloads while a REST load is in flight lets an older response overwrite a newer event, and replayed historical payloads can briefly restore obsolete values. Separately, `task_events.task_id` referenced the live task with `ON DELETE SET NULL`, losing the identifier needed to interpret a deletion event.

Options considered:

1. A snapshot cursor protocol with events buffered and reconciled per snapshot. Deferred: unnecessary machinery for the initial board.
2. Subscribe before loading and treat task events as invalidations of authoritative REST data. Chosen for v1 simplicity.
3. Keep the foreign key and add a separate deletion identifier. Rejected: event identity should survive deletion for all historical events, not just the last one.

## Decision

- The server subscribes to project notifications before opening the SSE response; the browser waits for the stream to open before loading the board.
- Every new `TaskEvent` is a refresh signal, not an authoritative replacement. At most one refresh runs per board; responses dirtied by later events or belonging to an obsolete view are discarded and coalesced into another refresh. No raw event queue.
- Refresh after every reconnection and successful local mutation, keeping the prior snapshot meanwhile.
- Persist each task event's original task UUID without a foreign key to the live task.

## Consequences

- Renames, deletes, dependency effects and updates share one refresh path. A late event cannot restore an old card.
- Historical event ids stay meaningful after task deletion without keeping deleted tasks alive.
