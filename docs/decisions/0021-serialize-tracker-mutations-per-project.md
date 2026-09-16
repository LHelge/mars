# 0021. Serialize tracker mutations per project

Status: accepted. Extends ADR 0016's atomic claims and ADR 0018's hand-off transaction rules.

## Context

REST requests, MCP calls and background jobs can change related tasks concurrently. An atomic claim protects one task, but does not protect graph-wide rules. Two concurrent dependency insertions can each pass a cycle check and together form a cycle. Concurrent prerequisite changes can also leave a stored `blocked` flag stale. Task numbers and the project's event sequence need a common serialization point.

Options considered:

1. Fine-grained task locks plus special handling for dependency graphs, state configuration and event allocation. Rejected for v1 complexity.
2. Serializable transactions with retries of complete tracker operations. Rejected as the default v1 design because explicit serialization is easier to implement and reason about at the expected scale.
3. Lock the existing project row for each tracker mutation and perform validation, mutation and event insertion in one transaction. Chosen. No queue service, worker queue or additional schema is needed.

## Decision

- Every tracker mutation starts a database transaction and locks its project row with `SELECT id FROM projects WHERE id = $1 FOR UPDATE`. Authoritative state reads and validation happen after that statement completes, using subsequent statements under `READ COMMITTED`.
- This applies to task creation/editing/deletion, claims/releases, dependencies, comments, state configuration, profile served-state changes, hand-off publication and system changes from launch, session cleanup, reapers and parent closure. Deletions that cascade into tracker data use the same discipline.
- Keep task-number allocation, cycle checks, lease checks, dependency/parent recomputation, hand-off records, comments, session links and all resulting task events in the same transaction. A failed operation rolls all of them back. Allocate project event sequences while holding the project row lock.
- Related tasks in the project share this serialization point. Reads do not take the mutation lock, and different projects can mutate independently. Helpers reuse the caller's transaction instead of opening separate ones.
- For operations involving git, acquire the existing project git lock first, prepare the required git objects/refs, then acquire the database project lock and revalidate before publishing. Never acquire a git lock while holding the database project lock. Do not hold a tracker transaction open for model, engine, email or git work. Existing git/Postgres crash limitations remain unchanged.
- Session events use a separate, analogous rule: lock the session row before allocating event sequences and append the complete event batch in one transaction. When an operation changes tracker and session data together, acquire the database project lock before session row locks; an event-only transaction never acquires a project lock afterwards.

## Consequences

- Concurrent reciprocal dependency insertions serialize: after the first commits, the second sees it and fails validation. The tracker never publishes half of a multi-task change.
- `MAX(seq)+1` is safe within the locked stream; primary keys remain an invariant check, not a normal collision/retry mechanism. The existing project task-number counter remains monotonic and is not replaced with `MAX(number)+1`.
- Throughput is bounded to one tracker mutation transaction per project at a time. This is an intentional v1 simplicity trade-off; no distributed orchestration is introduced.
- Acceptance tests cover simultaneous claims, reciprocal dependency insertion, prerequisite changes and claiming, distinct task numbers, ordered project events, rollback without partial events, and session event batches from concurrent writers.
