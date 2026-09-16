# 0009. Single-writer task claiming with leases

Status: superseded by 0016. The atomic claim statement is retained there; the time-based lease TTL and the fixed state enum are not.

## Context

Several agents and humans work the same tracker. Two agents must not both work one task, and a task whose agent died must not stay locked forever. Options: optimistic updates with `updated_at` version checks, advisory locks held by a connection, or an explicit lease column with expiry.

Optimistic checks prevent lost updates but do not express "this task is being worked on". Connection-held locks vanish when the orchestrator restarts and cannot outlive a request.

## Decision

A task is claimed with one atomic statement: `UPDATE tasks SET lease_holder_session_id = $s, lease_expires_at = NOW() + $ttl, state = 'in_progress' WHERE id = $id AND state = 'ready' AND lease_holder_session_id IS NULL RETURNING *`. Zero rows means somebody else has it. The holding session extends its lease implicitly on every `update` or `comment` it makes. A reaper releases expired leases: the task returns to `ready`, `assignee_session_id` is kept for context, and a `TaskEvent` announces the release. Only the lease holder may move a task to `done` or `needs_human` through MCP; humans may do anything through the UI.

Default lease TTL is 30 minutes; the `claim` tool accepts a shorter or longer one up to 4 hours.

## Consequences

- No two sessions ever hold one task; the database enforces it, not application logic.
- A crashed agent's task becomes visible again within one TTL.
- Agents must keep touching tasks they hold; the tool descriptions say so ("claim before you start, comment before you finish").
- Humans can override anything; the UI shows who holds a lease and lets a user release it.
