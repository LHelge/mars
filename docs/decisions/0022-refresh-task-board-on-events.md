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
- Every new `TaskEvent` is a refresh signal, not an authoritative replacement. At most one refresh runs per board; responses belonging to an obsolete view are discarded and coalesced into another refresh. No raw event queue.
- Refresh after every reconnection and successful local mutation, keeping the prior snapshot meanwhile.
- Persist each task event's original task UUID without a foreign key to the live task.

## Revision: a dirtied response is installed, not discarded

Originally a response dirtied by an event that arrived while it was in flight was thrown away. Under events arriving faster than a round trip — an ordinary working agent — every response is dirtied, so the board installed nothing at all and sat at its refreshing marker for as long as the work lasted, and every local mutation cost two reads of which the first was discarded.

Options considered:

1. Keep discarding, and make the marker honest. Rejected: the board is then a snapshot that stops updating exactly when it is being watched.
2. Buffer the events and reconcile them against the snapshot. Rejected again, for the reason option 1 of the original decision was rejected: that is the snapshot-cursor protocol this ADR exists not to build.
3. Install the dirtied response and run the follow-up it owes. Chosen. It is only sound because a refresh now cancels any outstanding request for the same key before making its own: with one refresh at a time and no joining of an older request, a settling response is always strictly newer than the snapshot it replaces. A dirtied one is behind the event, never behind the screen, and the follow-up catches it up.

The same pass made a first connection ask for `?after=latest` rather than replaying a project's whole history into a client that is about to read a REST snapshot anyway (`SPEC.md`, "SSE: task stream"), and coalesced the stream's query invalidations into one flush per tick.

## Consequences

- Renames, deletes, dependency effects and updates share one refresh path. A late event cannot restore an old card.
- Historical event ids stay meaningful after task deletion without keeping deleted tasks alive.
- A board under sustained events shows a snapshot that is at most one round trip behind, instead of the last one it managed to install before the events started.
- Cost stops growing with a project's age: opening a board is one REST snapshot whatever the length of its event history.
