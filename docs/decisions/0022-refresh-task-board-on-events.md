# 0022. Refresh the task board on events and retain deleted task identities

Status: accepted. Supersedes the task board's event-payload folding strategy in the initial frontend design; session transcript streaming is unchanged.

## Context

The task board loads states and tasks over REST and receives changes over SSE. Applying live event payloads while a REST load is in flight allows an older response to overwrite a newer event. Replayed historical payloads can also temporarily restore obsolete task or state values after a fresh load. Separately, `task_events.task_id` originally referenced the live task with `ON DELETE SET NULL`, losing the identifier needed to interpret a deletion event.

Options considered:

1. Add a snapshot cursor protocol and buffer/reconcile events against each snapshot. Deferred: unnecessary synchronization machinery for the initial board.
2. Subscribe before loading and treat task events as invalidations of authoritative REST data. Chosen for v1 simplicity. A refresh dirtied by another event is repeated.
3. Retain the existing task foreign key and add a separate deletion identifier. Rejected: event identity should survive deletion for all historical events, not just the final one.

## Decision

- The server subscribes to project notifications before opening the SSE response, then uses the existing replay and safety-read behavior. The browser installs event handlers and waits for the stream to open before loading the board.
- In v1, the board uses every new `TaskEvent` as a refresh signal, not as an authoritative replacement task or column list. Session transcript reducers are unchanged. Task event payloads remain available for history and other consumers.
- At most one refresh runs at a time for a mounted board. Fetch states and tasks, then replace both together. Track an event generation and a connection/view generation; discard responses dirtied by events or belonging to an obsolete connection/view, and coalesce changes into another refresh. Do not retain a raw event queue.
- Refresh after every stream reconnection and successful local board mutation. While loading or disconnected, retain any prior snapshot with a refreshing/reconnecting indication; a failed load is not an empty board and is not considered clean.
- Persist each task event's original task UUID without a foreign key to the live task. Only project-wide events such as `states_changed` have null `task_id`. Deleting a task writes its `deleted` event in the same project-locked transaction and leaves its historical event identities intact. Deleting the project still cascades its event history.

## Consequences

- Renames, deletes, dependency effects and ordinary updates share the same board refresh path. A late event cannot restore an old card from its payload.
- v1 trades additional REST reads for simpler consistency rules. Changes received during one load are coalesced; different project views keep separate state.
- Historical event IDs remain meaningful after task deletion without keeping deleted tasks alive.
- Acceptance covers a mutation during initial load, a rename or deletion during refresh, replayed older events, reconnection, a late response after changing projects, failed loads and deletion-event replay after the task row is gone.
