---
id: jeyrc
title: "Create the orchestrator crate skeleton: toolchain, crates, feature flag and module tree"
status: done
priority: P0
created: "2026-09-16T20:25:24.141710023Z"
updated: "2026-09-17T05:19:37.268816146Z"
tags:
  - orchestrator
  - core
  - infra
parent: sywed
assignee: claude-opus-subagent
attempts: 1
---

## Summary
Create `orchestrator/` as a compiling Rust crate with the pinned toolchain, edition 2024, every crate from the `ARCHITECTURE.md` "Orchestrator internals" table added through `cargo add`, the `integration-tests` cargo feature, and the binding module layout as empty modules that each `use crate::prelude::*`. This is the root of the whole backend; every other backend epic adds files into this tree, so the names and the feature flag must match the documents exactly.

## Documents
- `ARCHITECTURE.md` "Orchestrator internals": the directory tree (binding names) and the **Crates** table.
- `CLAUDE.md` "Backend conventions": Rust stable, edition 2024, `rust-toolchain.toml`, `cargo add` only, git is never a crate (ADR 0011), every module does `use crate::prelude::*`.
- `SPEC.md` "Test-only routes": the `integration-tests` cargo feature, never compiled into a release build.
- `README.md` "Development": intended repository layout.

## Acceptance criteria
- [ ] `orchestrator/rust-toolchain.toml` pins `channel = "stable"` (a concrete stable version string is acceptable) with components `rustfmt` and `clippy`.
- [ ] `orchestrator/Cargo.toml`: package `mars-orchestrator`, `edition = "2024"`, a `[lib]` plus a `[[bin]]` named `mars-orchestrator` (the binary name is referenced by `ARCHITECTURE.md` "Secrets" for `mars-orchestrator rotate-secrets`), and `[features] integration-tests = []`.
- [ ] Every crate in the table is present with the listed features, added with `cargo add` (no hand-edited version numbers): `axum` (`ws`), `axum-extra` (`cookie`, `typed-header`), `tower-http` (`trace`, `cors`), `sqlx` (`postgres`, `runtime-tokio`, `uuid`, `chrono`, `json`), `bollard`, `rmcp` (server + Streamable HTTP transport features as named by the current rmcp release), `tokio` (`full`), `tokio-stream`, `futures-util`, `jsonwebtoken`, `argon2`, `sha2`, `aes-gcm`, `rand`, `zeroize`, `base64`, `serde` (`derive`), `serde_json`, `thiserror`, `tracing`, `tracing-subscriber` (`env-filter`), `reqwest` (`json`, rustls TLS rather than native TLS so the release image needs no OpenSSL), `uuid` (`v4`, `serde`), `chrono` (`serde`), `dotenvy`; dev-dependencies `axum-test`, `testcontainers-modules` (`postgres`), `tempfile`. No `git2`/`gitoxide`.
- [ ] `src/lib.rs` declares `pub mod prelude; pub mod models; pub mod repositories; pub mod routes; pub mod ws; pub mod sse; pub mod mcp; pub mod engine; pub mod agent; pub mod session; pub mod git; pub mod secrets; pub mod events; pub mod email; pub mod cron;` and each is a directory module (`src/<name>/mod.rs`) containing `#![allow(unused_imports)]`-free `use crate::prelude::*;` (the prelude must export at least one item so the import is not unused; see notes).
- [ ] `src/prelude/mod.rs` exists and re-exports the items later tasks add (`Config`, `Error`, `Result`, `AppState`); in this task it may export only a `pub use` of `tracing::{debug, error, info, warn}` and `crate::prelude::Result` placeholder so the module imports compile without warnings.
- [ ] `src/main.rs` is a minimal `#[tokio::main] async fn main()` that returns `Ok(())` (real startup lands in the startup task).
- [ ] `orchestrator/migrations/` exists (empty, with `.gitkeep`) so `sqlx::migrate!()` can be wired in the startup task; `orchestrator/tests/` exists with `common/mod.rs` left for the Database epic (do not create `TestApp`).
- [ ] The repository root `.gitignore` already ignores `target/`; add `orchestrator/.sqlx/` is **not** ignored (it must be committed by the Database epic).
- [ ] `cd orchestrator && cargo fmt --check && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test` pass.

## Implementation notes
- Files: `orchestrator/rust-toolchain.toml`, `orchestrator/Cargo.toml`, `orchestrator/Cargo.lock` (committed; it is a binary crate), `orchestrator/src/lib.rs`, `orchestrator/src/main.rs`, `orchestrator/src/{prelude,models,repositories,routes,ws,sse,mcp,engine,agent,session,git,secrets,events,email,cron}/mod.rs`, `orchestrator/migrations/.gitkeep`.
- Feature flag name is exactly `integration-tests` (with a hyphen); `CLAUDE.md` and `SPEC.md` both spell it that way. Mocks and test-only routes in later epics are gated with `#[cfg(feature = "integration-tests")]`.
- `use crate::prelude::*` in an otherwise empty module triggers `unused_imports` under `-D warnings`. Do not silence it with `allow`; instead have the prelude export something every module legitimately uses (the `Result` alias and tracing macros are the natural choice) and give each empty module one line that uses it, e.g. a `pub(crate) fn module_name() -> &'static str` is *not* wanted. Preferred: leave `mod.rs` with `use crate::prelude::*;` plus a doc comment, and put `#![allow(unused_imports)]` only if the alternative is a fake symbol; record the choice in the PR so the first real file in each module removes it.
- `bollard`, `rmcp`, `sqlx` and `axum` major versions must be mutually compatible on `tokio` 1 and `hyper` 1; check `cargo tree -d` shows no duplicated `hyper`/`http` majors.
- `reqwest` with `default-features = false, features = ["json", "rustls-tls"]` keeps the future orchestrator image OpenSSL-free (Deployment packaging epic).
- Keep `[profile.release]` default; no LTO tuning in this task.

## Edge cases
- `cargo clippy -- -D warnings` must be run with `--all-targets --features integration-tests` as well as without the feature, because CI and `CLAUDE.md` use both forms.
- `sqlx` macros require `DATABASE_URL` or `SQLX_OFFLINE=true`; this task introduces no `query!` calls, so the crate must build with neither set.
- Do not add crates outside the table (for example `anyhow`, `once_cell`, `dotenv`); if one is genuinely needed, add it to the `ARCHITECTURE.md` table in the same commit (rule 1).

## Testing
- `cargo test` runs zero tests successfully; add one `#[test]` in `src/lib.rs` (`fn crate_compiles()`) is unnecessary; the clean build is the test.
- Command that must pass: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented layout as written. If a crate name or feature in the `ARCHITECTURE.md` table does not exist on crates.io under that name, correct the table in the same commit.

## Assumes from other epics
- none.