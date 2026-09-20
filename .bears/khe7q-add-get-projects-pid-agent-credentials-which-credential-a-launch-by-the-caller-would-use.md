---
id: khe7q
title: "Add GET /projects/{pid}/agent-credentials: which credential a launch by the caller would use"
status: open
priority: P1
created: "2026-09-20T22:22:37.187539463Z"
updated: "2026-09-20T22:22:37.187539463Z"
tags:
  - orchestrator
  - secrets
  - api
depends_on:
  - a8bga
parent: rdkmk
---

## Summary
The preflight that lets the UI say "authenticates with your subscription token" or "no agent credential" before anything is launched. It runs the same selection as the launch resolver, over metadata only.

## Documents
- `SPEC.md` "Secrets": the `GET /projects/{pid}/agent-credentials` row and the `AgentCredentialStatus` paragraph
- `ARCHITECTURE.md` "Secrets", Agent credentials (last sentence)
- ADR 0036

## Acceptance criteria
- [ ] `GET /api/projects/{pid}/agent-credentials` (JWT) → `AgentCredentialStatus[]`, one entry per `Backend` value in enum order: `{ backend, credential: { secret_id, name, scope } | null }`.
- [ ] The credential is the row at the most specific of the caller's `user` scope, the project's scope and `global` among the backend's credential names — the same rule as `resolve_for_launch`, and literally the same selection code, so the two cannot disagree.
- [ ] Nothing is decrypted, no `secret_uses` row is written, `last_used_at` does not move, and no value or ciphertext is in the response.
- [ ] 401 unauthenticated; 404 unknown project; a caller with `must_change_password` gets the usual 403.
- [ ] An `orchestrator_only` credential row (older than the write rule) at the winning scope yields `credential: null`, matching what a launch would do.
- [ ] Both clippy invocations and the test suite pass; `.sqlx/` regenerated.

## Implementation notes
- Files: `orchestrator/src/secrets/resolve.rs` (extract the selection into a function over metadata rows that both `resolve_for_launch` and this endpoint call), the secrets repository (one query: rows with `name = ANY($1)` at the three scopes, scope in the `WHERE`), `orchestrator/src/routes/projects.rs` or `routes/secrets.rs` — put the handler where the path's router lives and keep the DTO private to the module.
- The user scope is the *caller*, not a session's `created_by`: this answers "if I launch now".

## Edge cases
- An admin gets their own answer, not everyone's; there is no `?user_id=`.
- A project that is still `cloning` answers normally — credentials do not depend on the mirror.

## Testing
- New `tests/agent_credentials_api.rs`: none → null; global only; project beats global; caller's user scope beats project while another user's user-scoped credential is invisible; different names at different scopes; unauthenticated; unknown project; assertion that `secret_uses` is unchanged afterwards (through `GET /secrets/{id}/uses`).
