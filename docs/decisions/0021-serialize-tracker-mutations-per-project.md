# 0021. Serialize tracker mutations per project

Status: accepted. Extends ADR 0016's atomic claims and ADR 0018's hand-off transaction rules.

## Context

REST requests, MCP calls and background jobs change related tasks concurrently. An atomic claim protects one task, not graph-wide rules: two concurrent dependency insertions can each pass a cycle check and together form a cycle, and concurrent prerequisite changes can leave a stored `blocked` flag stale. Task numbers and the project's event sequence also need one serialization point.

Options considered:

1. Fine-grained task locks plus special handling for dependency graphs, state configuration and event allocation. Rejected for v1 complexity.
2. Serializable transactions with retries of whole operations. Rejected as the default: explicit serialization is easier to implement and reason about at the expected scale.
3. Lock the project row for each tracker mutation and do validation, mutation and event insertion in one transaction. Chosen; no queue service or extra schema.

## Decision

- Every tracker mutation locks its project row with `SELECT ... FOR UPDATE` before authoritative reads, validation, mutation and event allocation, all in one transaction. Helpers reuse the caller's transaction. Reads do not take the lock.
- Operations involving git take the project git lock first, prepare refs, then take the database lock and revalidate. Never the reverse, and never hold a tracker transaction open for model, engine, email or git work.
- Session events lock the session row before allocating sequences. Combined operations take the project lock before session row locks.

## Consequences

- Reciprocal dependency insertions serialize; the second fails validation. The tracker never publishes half a multi-task change.
- `MAX(seq)+1` is safe within the locked stream; primary keys remain an invariant check, not a retry mechanism.
- Throughput is one tracker mutation per project at a time, an intentional v1 trade-off.
