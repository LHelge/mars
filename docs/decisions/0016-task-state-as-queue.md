# 0016. Task state is a project-defined queue; leases follow session liveness

Status: accepted. Supersedes 0009.

## Context

ADR 0009 fixed task states as an enum, routed tasks through a separate `role` column, and expired leases on a timer. Two routing keys made it ambiguous who picks a task up, `in_progress` erased the queue a task came from, and a conversational session talking with a person for an hour lost its lease through nobody's fault.

Options considered:

1. Keep the enum, add `review` and `merge`, keep `role`. Rejected: every new pipeline shape is a migration.
2. A fixed enum of pipeline states with per-profile served states. Rejected: the pipeline is what the operator is least sure about, and enum values can never be removed.
3. Project-defined states as rows, typed by what the orchestrator needs to know, with per-profile served states. Chosen.
4. A synced store as Beads does with Dolt. Rejected: Postgres already serialises claims.

## Decision

- A task's state is the queue it waits in. States are per-project rows with a `kind`: `queue`, `human` (one per project) or `terminal` (satisfies dependencies, `cancelled` included).
- A profile serves a set of `queue` states; served states plus the system prompt are the role.
- There is no in-progress state. The lease holder is the worker; a hand-off is a state change that clears the lease atomically.
- The atomic claim from ADR 0009 stays; the time-based TTL goes. A lease is valid while its holder session is alive and is released by the reaper when the holder finishes or fails.

## Consequences

- Changing the pipeline is data entry, not a migration.
- A parked conversational session keeps its tasks indefinitely; a person releases them from the UI.
- Agents cannot forget to extend a lease.
