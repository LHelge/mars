# 0015. Project-scoped shared directories and a per-project CLI state directory

Status: accepted

## Context

Every session has its own clone, its own `HOME` and its own CLI state directory. Two costs follow. Build output is duplicated per session: a Rust project's `target/` is several gigabytes, so ten sessions on one project fill a disk that would comfortably hold one. And nothing an agent learns survives its session: the CLI keeps auto memory, installed skills and plugins under its state directory, keyed by the working directory, and each session starts from an empty one.

Options considered:

1. Keep everything per session. Simple and maximally isolated; the disk cost and the lost memory remain.
2. Share one CLI state directory across all sessions of the deployment. Rejected: every session's working directory is `/session/work`, so the CLI files everything under the same encoded-path subdirectory, and memory written for one project would be presented to agents working on every other project.
3. Share build output through a compilation cache such as `sccache`. Saves compile time, not disk: every session still keeps its own full `target/`. Complementary, not sufficient.
4. Share the CLI state directory per project, and let a project declare additional directories that are mounted read-write into every session of that project. Which paths make sense is ecosystem knowledge (Cargo's `target/` takes a file lock and can be shared; `node_modules` cannot), so the mechanism is generic and the defaults are documentation.

## Decision

Option 4.

- The CLI state directory lives at `/data/projects/<id>/claude/`, mounted at the same path in every session container of the project and selected with `CLAUDE_CONFIG_DIR`. Transcripts, memory, skills and plugins are therefore shared by all sessions of a project and by no session of another project. Running several CLI processes against one state directory is the normal single-machine situation and is supported by the CLI.
- A project holds a list of shared directories, each a name and an absolute container path. The orchestrator creates `/data/projects/<id>/shared/<name>` on first use and mounts it read-write at the container path in every session container of the project. The container path may lie inside the work tree (`/session/work/target` is the expected first use), which relies on nested bind mounts and is verified on both engines.
- Shared directories are opt-in per project, empty by default, and removed with the project. A user can empty one from the UI while no session of the project is running.

## Consequences

- ADR 0012 said a session cannot reach other sessions. That now reads: a session cannot reach sessions of other projects; sessions of one project share the CLI state directory and whatever directories the project declares. A file planted in a shared build directory by one session can be executed by another session's build. This is accepted because those sessions already work on the same repository with the same trust, and because the sharing is opt-in.
- Concurrent Cargo builds from several sessions serialise on Cargo's build-directory lock rather than corrupt each other; the same-path convention (`/session/work` everywhere) means fingerprints match and artifacts are reused across sessions. Other ecosystems' build output inside the checkout is not shareable; their download caches are. The guidance lives in `README.md`, "Operating notes".
- Deleting a session no longer removes its transcript by deleting its directory. The owner deletes the transcript by CLI session id from the project's state directory instead.
- A shared directory grows across branches and feature sets. Disk pressure is handled by the "clear" action, not by an automatic job, because the orchestrator cannot know which artifacts a running build still needs.
- A second agent backend gets a sibling state directory under `/data/projects/<id>/` and its own environment variable.
