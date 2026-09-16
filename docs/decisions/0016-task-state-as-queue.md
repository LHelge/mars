# 0016. Task state is a project-defined queue; leases follow session liveness

Status: accepted. Supersedes 0009.

## Context

The tracker is the way agents hand work to each other: a planner breaks a backlog item down, an implementer builds it, a reviewer checks it, a merger lands it, and a rejected change goes back to the implementer. ADR 0009 fixed the task states as an enum (`blocked`, `ready`, `in_progress`, `needs_human`, `done`), routed tasks to agents through a separate `role` column, and expired leases on a timer that the holder had to keep resetting with tracker calls. Three problems followed. Two routing keys (state and role) made it ambiguous which one decides who picks a task up. `in_progress` erased the queue a task came from, so "being reviewed" and "being implemented" looked the same. And a conversational session that spent an hour talking with a person lost its lease without anyone being at fault.

Options considered:

1. Keep the enum, add `review` and `merge` values, keep `role`. Rejected: every new role or pipeline shape is a migration, and the two-key ambiguity stays.
2. A fixed enum of pipeline states with per-profile "served states" and no `role`. Simpler than what was chosen, but the pipeline is the thing the operator is least sure about, and enum values can be added but never removed or renamed.
3. Project-defined states as rows, typed by what the orchestrator needs to know (`queue`, `human`, `terminal`), with per-profile served states and no `role`. Chosen.
4. Back the tracker with a synced store as Beads does with Dolt. Rejected: Beads syncs because each agent owns a checkout; here every agent talks to one orchestrator and Postgres already serialises claims.

## Decision

- A task's state is the queue it waits in. States are rows in `task_states`, per project, each with a `kind`: `queue` (agents claim from it), `human` (exactly one per project; escalations land here; agents never claim from it), `terminal` (closes the task, satisfies dependencies). Every project starts with `backlog`, `ready`, `review`, `merge`, `needs_human`, `done`, `cancelled`.
- A profile serves a set of `queue` states. The `ready` and `claim` tools see only those. Served states plus the system prompt are the role; there is no `role` column.
- There is no in-progress state. The lease holder is the worker, in whatever state the task is in. A hand-off is a state change by the holder and clears the lease atomically. `blocked` is a stored boolean, not a state, and excludes a task from claiming in every state.
- The atomic claim statement from ADR 0009 is kept. The time-based lease TTL is not. A lease is valid while its holder is `creating`, `running` or `parked` and is released by the stuck-task reaper when the holder is `done` or `failed`. An ephemeral session silent for longer than its profile's idle timeout is failed as `stalled`, which releases its tasks through the same path.
- `attempts` counts claims since the last state change. A release (by agent or reaper) at the project's `max_attempts` moves the task to the `human` state instead of back into its queue.
- A session can be launched for one task; the launch claims it in the same transaction. This is how v1 puts a conversational agent on a specific task. Automatic launching is deferred (`ARCHITECTURE.md`, "Task tracker", "After v1").
- Any terminal state satisfies dependencies, `cancelled` included, following Beads.

## Consequences

- Changing the pipeline is data entry on the project page, not a migration. Renaming a state is a rename everywhere because tasks and profiles reference states by id.
- The board has one column per state and shows work in progress as a held card, not as a separate column. The `Task` DTO carries the holder and `attempts` so the UI can render that.
- A parked conversational session keeps its tasks indefinitely; a person releases them from the UI if that is wrong. The reaper never guesses about a live session.
- Agents cannot forget to extend a lease, and cannot hold one after dying. The tool descriptions no longer need "keep touching the task".
- The orchestrator must know, for every state operation, whether the target state is terminal or human; the kind column makes that one lookup. Validation that a served state is a `queue` state and that the human state is unique lives in the repository.
- A dispatcher and scheduled agents fit without touching the task tables: they are profile columns plus one job that calls the existing launch path.
