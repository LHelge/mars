# 0015. Project-scoped shared directories and a per-project CLI state directory

Status: accepted

## Context

Every session had its own clone, `HOME` and CLI state directory. Build output was duplicated per session (a Rust `target/` is gigabytes), and nothing an agent learned survived its session.

Options considered:

1. Keep everything per session. Simple and isolated; both costs remain.
2. One CLI state directory for the whole deployment. Rejected: every session's cwd is `/session/work`, so one project's memory would reach agents on every other project.
3. A compilation cache such as `sccache`. Saves compile time, not disk; complementary, not sufficient.
4. Share the CLI state directory per project, and let a project declare directories mounted read-write into every session of that project. Chosen. Which paths are safe to share is ecosystem knowledge (Cargo's `target/` takes a lock; `node_modules` does not), so the mechanism is generic.

## Decision

- The CLI state directory is per project, mounted at the same path in every session container of the project and selected with `CLAUDE_CONFIG_DIR`.
- A project declares shared directories by name and container path, mounted read-write in every session of the project. The path may lie inside the work tree; `/session/work/target` is the expected first use.
- Shared directories are opt-in, removed with the project, and can be emptied from the UI while no session of the project runs.

## Consequences

- ADR 0012 now reads: a session cannot reach sessions of other projects. A file planted in a shared build directory can be executed by another session's build. Accepted: those sessions already share a repository and trust.
- Deleting a session removes its transcript by CLI session id, not by deleting a directory.
- Disk pressure is handled by the "clear" action, not an automatic job.
