# 0005. LISTEN/NOTIFY is a wake signal, rows are the truth

Status: accepted

## Context

WebSocket and SSE subscribers need to learn about new session events and task changes without polling. Postgres `LISTEN/NOTIFY` is available for free. But its payload is capped at 8000 bytes, notifications are dropped if the listener is disconnected, are coalesced when identical inside a transaction, and are not delivered to the sending backend's own uncommitted view. Using it as the transport would lose events.

The alternative, an in-process broadcast channel, works only while every subscriber shares the process with the writer, which is true today but not if the orchestrator is ever split.

## Decision

Every event is a row (`events`, `task_events`) with a per-stream monotonic `seq` derived from the table. After commit the writer sends `NOTIFY <channel>, '<id>:<seq>'`. Subscribers hold a cursor and, on each notification (or on a periodic timer as a fallback), read all rows with `seq` greater than their cursor. Notifications carry no event content and their loss is harmless.

Inside the orchestrator a `tokio::sync::broadcast` fan-out mirrors the notification so that in-process subscribers do not each hold a Postgres listener connection.

## Consequences

- At-least-once delivery to clients with dedupe on `seq`, at any scale of subscribers.
- Latency is one NOTIFY round trip plus one indexed read.
- A subscriber that reconnects simply resumes from its cursor; there is no replay buffer to size.
- The writer must send NOTIFY after commit, not inside the transaction, or a listener may read before the row is visible. The repository layer owns this ordering.
