---
id: j6xxw
title: "Curiosity reads the repository's instruction files: AGENTS.md, then CLAUDE.md"
status: open
priority: P2
created: "2026-09-21T12:23:04.937328Z"
updated: "2026-09-21T12:23:04.937328Z"
tags:
  - curiosity
  - backlog
  - agent-backends
depends_on:
  - pfs5r
---

## Summary
Mars splits responsibilities: the repository owns "how we work here" through its instruction file, the profile owns "what this agent's job is" through its system prompt (`ARCHITECTURE.md`, "Claude Code invocation", the `--bare` paragraph). Claude Code loads `CLAUDE.md` itself; Curiosity must do the equivalent or the seeded role prompts, which defer to the repository's instructions, defer to nothing.

## Acceptance criteria
- [ ] At session start, from the working directory upward to the repository root: `AGENTS.md` if present, otherwise `CLAUDE.md`; an `AGENTS.md` that only points at `CLAUDE.md` (as this repository's does) results in both being read — follow relative Markdown links one level deep, or document a simpler rule that still handles that case.
- [ ] Placed after the built-in prompt and before the profile's appended system prompt; size-capped with a visible truncation note; re-read on `session/load`.
- [ ] `curiosity/README.md` documents the lookup. `ARCHITECTURE.md`, "Curiosity invocation" states the split holds for this backend.

## Testing
- Unit tests over `tempfile` trees; one mock-script integration test asserting the text reached the model request.