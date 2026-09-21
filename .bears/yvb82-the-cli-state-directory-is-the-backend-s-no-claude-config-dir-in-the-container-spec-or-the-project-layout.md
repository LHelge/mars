---
id: yvb82
title: "The CLI state directory is the backend's: no CLAUDE_CONFIG_DIR in the container spec or the project layout"
status: open
priority: P2
created: "2026-09-21T12:17:02.175Z"
updated: "2026-09-21T20:26:14.727456742Z"
tags:
  - orchestrator
  - agent
  - engine
  - projects
parent: fgbm3
---

## Summary
`engine/spec.rs` injects `CLAUDE_CONFIG_DIR` and binds `projects/<pid>/claude` for every session, and `projects/layout.rs` creates and names that directory, whatever the profile's backend. Let the backend declare its state directory — a directory name under the project's data directory and the environment variable that points the CLI at it — and have the launcher and the layout use that.

## Documents
- `ARCHITECTURE.md`, "Storage", "Claude Code invocation" (the `CLAUDE_CONFIG_DIR` paragraph), "Session container specification" (Environment and Binds rows); `SPEC.md`, "Shared directories" where reserved paths are listed; ADR 0015 is not changed (the placement per project stands).

## Acceptance criteria
- [ ] `AgentBackend::state_dir() -> Option<StateDir { dir_name, env_var }>`; Claude answers `claude` / `CLAUDE_CONFIG_DIR`, so every existing path, bind and variable is byte-identical and no data moves.
- [ ] `engine/spec.rs` builds the environment entry and the bind from it; a backend answering `None` gets neither.
- [ ] `ProjectLayout` creates the directory of every backend in `BACKENDS` (or lazily at first launch — choose one and document it) and deletes them with the project; `claude_dir()`-style accessors become backend-parameterised.
- [ ] The reserved-path validation for shared directories covers every backend's state directory.
- [ ] The transcript-path fallback for `--resume` stays inside the Claude adapter's knowledge (`CLI_PROJECTS`/`CLI_WORK_DIR` in `layout.rs` are Claude's).

## Implementation notes
- `engine/spec.rs` (~l.84, 233–258 and its tests), `projects/layout.rs` (~l.37–112, 168, 191), `projects/{create,delete}.rs`, `models/shared_dir.rs`, `session/launcher.rs`.
- The engine contract tests and `tests/engine.rs` assert the env and binds; they change only in how the expectation is derived.

## Edge cases
- Startup probe and the uid contract are unaffected; assert the new directory is created with the same ownership path as today.

## Testing
- Unit tests in `engine/spec.rs` and `projects/layout.rs`; `tests/engine_mock.rs`; with `DOCKER_HOST`, `tests/engine.rs` and `tests/session_e2e.rs`.