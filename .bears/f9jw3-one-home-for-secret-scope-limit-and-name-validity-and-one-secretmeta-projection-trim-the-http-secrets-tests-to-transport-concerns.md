---
id: f9jw3
title: One home for secret scope, limit and name validity and one SecretMeta projection; trim the HTTP secrets tests to transport concerns
status: open
priority: P3
created: "2026-09-17T20:04:04.631008504Z"
updated: "2026-09-17T20:04:04.631008504Z"
tags:
  - orchestrator
  - secrets
  - architecture
  - tests
depends_on:
  - psybz
parent: zeccj
---

## Summary
Scope validity is checked in `service.rs` (create and list), `ScopeRef::new`, the database `CHECK`, and deliberately not in the repository; the `uses` limit is clamped in the service and again in the repository; secret-name validity is `SecretName::parse`, `agent_profile::is_secret_name`, `validate_profile_secrets` and a re-parse in `resolve.rs`; `SecretMeta` is hand-built in `service.rs::meta_of` and SQL-projected in the repository, which is why `create` returns a local meta while every other write does a second read. `tests/secrets_api.rs` (803 lines) re-asserts about ten rules `tests/secrets_service.rs` (1018 lines) already proves. Give each rule one home and each test one seam.

## Documents
- `SPEC.md` "Secrets (`/api/secrets`)" (scopes, name pattern, `uses` limit default and maximum, `SecretMeta` shape).
- `docs/data-model.md` `secrets` (`CHECK` constraints), `secret_uses`.
- ADR 0006.

## Acceptance criteria
- [ ] `ScopeRef::new` is the only place the scope/scope_id pairing is validated; the service constructs a `ScopeRef` once and passes it down; the list filter takes a `ScopeRef` or an enum, not loose fields.
- [ ] The `uses` limit is validated once (in the service, with `DEFAULT_USES_LIMIT` and `MAX_USES_LIMIT`); the repository takes a validated `u32` and does not clamp.
- [ ] `SecretName::parse` is the only name rule; `agent_profile.rs` calls it; `resolve.rs` receives `SecretName`s from the profile and does not re-parse.
- [ ] `SecretMeta` has one constructor (`From<&Secret>` or the SQL projection); every write returns the row it wrote through that constructor, and the extra read in `service.rs::meta` is gone.
- [ ] `tests/secrets_api.rs` keeps: authentication and admin gating per endpoint, request-body shape errors, the response shape of each endpoint, and one end-to-end create → list → delete; every rule already asserted in `tests/secrets_service.rs` (bad name, empty value, impossible scope, conflict, unknown id, limit range) is removed from the HTTP file.

## Implementation notes
- Files: `orchestrator/src/secrets/{service,resolve}.rs`, `orchestrator/src/models/{secret,agent_profile}.rs`, `orchestrator/src/repositories/secrets.rs`, `orchestrator/tests/{secrets_api,secrets_service}.rs`.
- Keep the route handlers at their current three to eight lines; this task does not merge the route and service layers (the service earns its place: scope defaulting, `authorize`, transaction ownership).

## Testing
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes with fewer tests and the same coverage of rules.

## Documentation
- None: `SPEC.md` already states each rule once; verify the `uses` limit wording (default, maximum, out-of-range answer) matches the single validation and correct `SPEC.md` if the code and the document disagree.