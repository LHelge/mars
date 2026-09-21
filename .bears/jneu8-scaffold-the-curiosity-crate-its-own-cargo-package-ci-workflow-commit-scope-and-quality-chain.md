---
id: jneu8
title: "Scaffold the curiosity crate: its own Cargo package, CI workflow, commit scope and quality chain"
status: open
priority: P2
created: "2026-09-21T12:18:04.047673Z"
updated: "2026-09-21T20:26:14.783028543Z"
tags:
  - curiosity
  - infra
  - ci
  - docs
parent: w9nsq
---

## Summary
Create the home of Mars's own coding agent: a top-level Cargo package `curiosity/` with a binary `curiosity`, independent of `orchestrator/` (own `Cargo.toml`, own `Cargo.lock`, no workspace), and everything the repository's conventions need to know about it.

## Documents
- `CLAUDE.md`: "Project overview" (one sentence on `curiosity/`), "Git workflow" (add scope `curiosity`), "Code quality" (its chain), a short "Curiosity conventions" section: edition 2024, `cargo add` for crates, `Result` everywhere, `tracing` to **stderr only** (stdout will be a protocol channel), no real credentials in tests or fixtures (rule 3).
- `README.md`: "Development" (how to build and run it), the repository layout wherever it is listed. `ARCHITECTURE.md`, "Components": one entry.
- The provenance note lives in `curiosity/README.md`: copied from `github.com/LHelge/midgaard` (MIT, same author) at commit `0684a87`, no dependency between the projects, Mars supersedes it.

## Acceptance criteria
- [ ] `curiosity/Cargo.toml` (edition 2024, `publish = false`, licence as the repository's), `curiosity/rust-toolchain.toml` identical to the orchestrator's, `src/main.rs` + `src/lib.rs` (thin binary over a library, so integration tests reach the code), `clap` with `--version`.
- [ ] `.github/workflows/curiosity.yml`: path-filtered to `curiosity/**`, runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo nextest run` (or `cargo test` — choose and say why in the workflow comment) and doctests; modelled on `orchestrator.yml`.
- [ ] The quality chain in `CLAUDE.md` is one copy-pasteable command, as the two existing ones are.
- [ ] `.dockerignore` and `.gitignore` cover `curiosity/target`.
- [ ] `task-implementer` agent definition and the `implement-epic` skill mention the third chain if they enumerate chains (`.claude/agents/task-implementer.md`, `.claude/skills/implement-epic/`).

## Implementation notes
- No code is copied in this task; the binary prints its version and exits.
- The orchestrator workflow's path filters must not start running for `curiosity/**` changes, and vice versa.

## Testing
- The new workflow's commands pass locally.