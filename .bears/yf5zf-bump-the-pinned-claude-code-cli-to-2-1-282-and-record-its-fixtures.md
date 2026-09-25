---
id: yf5zf
title: Bump the pinned Claude Code CLI to 2.1.282 and record its fixtures
status: done
priority: P2
created: "2026-09-25T11:57:14.603723106Z"
updated: "2026-09-25T12:20:11.710382Z"
attempts: 1
---

Move the session image pin (`images/claude/Dockerfile`, `ARG CLAUDE_CODE_VERSION`) and the adapter pin (`src/agent/claude/mod.rs`, `CLAUDE_CLI_VERSION`) from 2.1.274 to 2.1.282. The dev image inherits the pin (`ARCHITECTURE.md`, "Session image").

## References
- `ARCHITECTURE.md` "Session image", "Claude Code invocation"; `CLAUDE.md` "Testing expectations" (a CLI bump adds fixtures, never edits old ones)
- `orchestrator/tests/fixtures/claude/README.md`; `images/claude/VERIFY.md` "Recording the MCP conformance fixtures"

## Acceptance
- [ ] Both pins read 2.1.282; `the_pinned_version_matches_the_image` passes
- [ ] `tests/fixtures/claude/2.1.282/` recorded by `tests/claude_probe.rs`, expected files reviewed by hand, synthetic scenarios carried over
- [ ] `tests/fixtures/mcp/2.1.282/` recorded by `scripts/mcp-record/record.sh`
- [ ] Any change in native shape against 2.1.274 is reflected in the translator and in `SPEC.md`/`ARCHITECTURE.md`; the documents' "pinned version" references name 2.1.282
- [ ] Base and dev images build under the new tag