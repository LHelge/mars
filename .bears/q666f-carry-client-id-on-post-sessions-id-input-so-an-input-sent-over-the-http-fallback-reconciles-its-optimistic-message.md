---
id: q666f
title: Carry client_id on POST /sessions/{id}/input so an input sent over the HTTP fallback reconciles its optimistic message
status: done
priority: P2
created: "2026-09-20T07:58:26.273980356Z"
updated: "2026-09-20T08:23:03.642074904Z"
tags:
  - orchestrator
  - frontend
  - sessions
  - realtime
depends_on:
  - zum7c
parent: cgdc2
attempts: 1
---

## Summary
Found while reviewing zum7c (`useSessionSocket`). When the socket is not open, `SessionSocket.send` falls back to `POST /sessions/{id}/input`. The REST body is a bare `SessionInput` with no `client_id` (`orchestrator/src/routes/sessions.rs`, the REST input handler records `client_id: None`), so the `user_message` event that later records the input carries none, `sessionStore`'s `replaceOptimistic` cannot match it, and the transcript keeps the optimistic message `pending` forever with the replayed `user_message` appended beside it as a duplicate.

## Documents
- `SPEC.md` "Sessions" table, `POST /sessions/{id}/input`; "WebSocket: session stream" (`client_id` is a client-generated string echoed back so optimistic UI can reconcile; the `user_message` follows with the same `client_id`); "AgentEvent" `user_message { client_id? }`; known v1 restart limitation (ADR 0020: `client_id` is not a durable idempotency key, which stays true).

## Acceptance criteria
- [ ] `POST /sessions/{id}/input` accepts an optional `client_id` beside the `SessionInput` fields (decide and document the body shape: `{kind, text, client_id?}` keeps old callers working) and the resulting `user_message` event carries it, exactly as the socket path does. Validation matches the socket's (length cap, string).
- [ ] `SPEC.md` "Sessions" and "WebSocket: session stream" say so in the same commit.
- [ ] `frontend/src/services/sessions.ts#sendInput(id, input, clientId?)` sends it and `SessionSocket.send` passes its generated `client_id` on the REST fallback.
- [ ] Backend integration test: REST input with `client_id` produces a `user_message` with that `client_id`; without it the event has none (unchanged). Frontend unit test in `useSessionSocket.test.ts`: fallback send, then the replayed `user_message` replaces the optimistic message (one user message, not pending).
- [ ] Both quality chains pass.

## Out of scope
Any delivery guarantee or idempotency (ADR 0020 stands).
