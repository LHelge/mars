# 0028. Issue notifications within event transactions

Status: accepted. Supersedes the notification transaction-ordering requirement in ADR 0005.

## Context

ADR 0005 says notifications must be issued after commit to prevent readers seeing unfinished writes. PostgreSQL already defers delivery of notifications issued inside a transaction until it commits, and discards them on rollback. Issuing a separate notification after commit instead leaves a gap where the writer can crash after persisting an event but before notifying listeners.

## Decision

Execute `pg_notify` with bound parameters on the same transaction that inserts events or changes session state. Commit the rows and notification together. For an event batch, one notification per affected stream may carry the highest sequence written by that transaction.

The shared Postgres listener forwards delivered notifications to the existing in-process broadcast channels. Writers do not broadcast before commit or issue a second database notification afterwards. Notifications contain only the existing identifiers and cursors/state, never event content.

## Consequences

- Rollback publishes neither the rows nor their notifications. A successful commit does not require a separate notification write from the application.
- Notifications remain best-effort wake signals for connected listeners. Cursor-based reads, replay, deduplication and the periodic safety read remain unchanged.
- Acceptance covers no notification before commit, delivery after commit, rollback without delivery, batch notification followed by reading every new event, and recovery through the safety read after a lost notification.

Reference: [PostgreSQL 18 NOTIFY documentation](https://www.postgresql.org/docs/18/sql-notify.html).
