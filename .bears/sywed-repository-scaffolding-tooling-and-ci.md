---
id: sywed
title: Repository scaffolding, tooling and CI
type: epic
status: open
priority: P0
created: "2026-09-16T20:11:32.579396406Z"
updated: "2026-09-16T20:11:32.579396406Z"
tags:
  - infra
  - orchestrator
  - frontend
---

## Scope

Turn the documentation-only repository into a buildable skeleton that every other epic extends.

- `orchestrator/` Rust crate: `rust-toolchain.toml`, edition 2024, the crate list from `ARCHITECTURE.md`, "Orchestrator internals" added with `cargo add`, the module layout (`prelude/`, `models/`, `repositories/`, `routes/`, `ws/`, `sse/`, `mcp/`, `engine/`, `agent/`, `session/`, `git/`, `secrets/`, `events/`, `email/`, `cron/`) as empty modules with `use crate::prelude::*`.
- `prelude/`: `Config::from_env()` reading every variable in `README.md`, "Configuration" and failing fast naming missing required ones; the `Error` enum contract and `IntoResponse` mapping to `{status, error}`; `Result<T>`; `AppState` shape; `tracing` setup with `env-filter`.
- `main.rs` startup order (config, pool, migrations, listeners) with `GET /api/health`.
- `.env.example` carrying the same variables as the README table.
- `frontend/` Vite + React 19 + TypeScript strict + Tailwind 4 + React Router 7 + TanStack Query + Zustand + ESLint + Playwright skeleton, `npm run dev` proxying `/api` and `/ws`.
- GitHub Actions workflows from `README.md`, "CI": Orchestrator CI (fmt, clippy `-D warnings`, tests with `SQLX_OFFLINE=true`), Frontend CI (lint, typecheck, build), plus placeholders for E2E and Images that later epics fill in.

## Documents

`README.md` "Development", "Configuration", "CI"; `ARCHITECTURE.md` "Orchestrator internals"; `CLAUDE.md` "Backend conventions", "Frontend conventions".

## Acceptance criteria

- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test` pass on an empty crate that starts, serves `/api/health` and exits cleanly.
- [ ] `Config::from_env()` names each missing required variable; `.env.example` and the README table list the same variables.
- [ ] `npm run lint && npx tsc -b && npm run build` pass on the frontend skeleton.
- [ ] Orchestrator CI and Frontend CI workflows are green on `main`.

## Out of scope

Compose, nginx and orchestrator container images (Deployment packaging epic); migrations and repositories (Database schema epic).