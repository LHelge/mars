---
id: bj3xv
title: Reject bare non-level words in RUST_LOG instead of silencing the orchestrator
status: open
priority: P3
created: "2026-09-17T05:33:40.860822059Z"
updated: "2026-09-17T05:33:40.860822059Z"
tags:
  - orchestrator
  - core
depends_on:
  - au4vs
parent: sywed
---

## Summary
`init_tracing` (`src/prelude/telemetry.rs`, task au4vs) falls back to `info` only when `EnvFilter::try_new` rejects the string. `EnvFilter` parses a bare word such as `RUST_LOG=verbose` as a *target* directive, so a typo in `RUST_LOG` silences all logging instead of falling back to `info` as `README.md` "Configuration" implies.

## Documents
- `README.md` "Configuration": `RUST_LOG` log filter, `info` by default.
- `ARCHITECTURE.md` "Orchestrator internals", crates: `tracing-subscriber` (`env-filter`).

## Acceptance criteria
- [ ] A filter consisting only of bare words that are not level names (`trace`, `debug`, `info`, `warn`, `error`, `off`) and contain no `=` or `,` falls back to `info` with the same `warn!` line as an unparseable filter.
- [ ] Legitimate target directives (`mars_orchestrator=debug`, `sqlx=warn,info`) still apply unchanged.
- [ ] Unit test for `RUST_LOG=verbose` asserting the fallback decision.

## Discovered while
Implementing au4vs (AppState and tracing initialisation).