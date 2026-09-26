---
id: juk2a
title: Move the JSON-error Query and Path extractor wrappers from routes/secrets.rs to routes/extractors.rs and use them in every route module
status: in_progress
priority: P3
created: "2026-09-17T14:33:24.941354444Z"
updated: "2026-09-26T17:16:40.857960137Z"
tags:
  - orchestrator
  - api
  - refactor
depends_on:
  - mqf98
attempts: 1
---

## Summary
`routes/secrets.rs` (secrets manager epic, task mqf98) wraps `axum::extract::Query` and `axum::extract::Path` so that a bad query string or a malformed UUID in the path answers the documented `{status, error}` JSON body instead of axum's plain-text rejection. The older route modules (`routes/users.rs`, `routes/auth.rs`) still use the bare extractors, so `GET /api/users/not-a-uuid` answers plain text while `PUT /api/secrets/not-a-uuid` answers JSON. Make the wrappers shared and use them everywhere.

## Documents
- `SPEC.md` "REST API" (errors are `{ "status", "error" }`)
- `CLAUDE.md` "API conventions"

## Acceptance criteria
- [ ] The `Query<T>` and `Path<T>` wrappers move to `orchestrator/src/routes/extractors.rs` and are re-exported from `routes/mod.rs` next to `CurrentUser`; `routes/secrets.rs` imports them instead of defining them.
- [ ] Every handler in `routes/users.rs` and `routes/auth.rs` that takes a `Path` or `Query` uses the wrappers.
- [ ] One integration test per affected module asserts that a malformed UUID in the path answers 400 with the JSON error shape.
- [ ] No behaviour change other than the error body format for malformed paths and query strings.

## Documentation
- none if `SPEC.md` "REST API" already states that every error is JSON; otherwise add the sentence that path and query parsing failures follow the same shape.