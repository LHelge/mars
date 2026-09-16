---
id: "3mmrq"
title: Document both session images, the tag convention and the SESSION_IMAGE_DEFAULT default
status: open
priority: P1
created: "2026-09-16T20:30:10.375619282Z"
updated: "2026-09-16T20:30:10.375619282Z"
tags:
  - images
  - docs
depends_on:
  - "64jkc"
  - nfz7m
parent: deex5
---

## Summary
Write back what the image tasks decided: the build commands for both images, the `mars-session-claude:<cli version>` tag convention, the default value of `SESSION_IMAGE_DEFAULT` (aligned across `README.md`, `.env.example` and `Config::from_env()`), the stub's fixture path and environment knobs, and the `images/` layout. This is the epic's "README build instructions verified; `SESSION_IMAGE_DEFAULT` default documented" acceptance item (CLAUDE.md rule 1: documentation is part of the change).

## Documents
- `README.md` "Configuration" (`SESSION_IMAGE_DEFAULT` row has no default today), "Development" (layout lists `images/ session container images (claude/)`), "Session image" (only the claude build command), "CI" (Images row)
- `ARCHITECTURE.md` "Session image" (add the tag convention sentence and the stub's contract details), "Claude Code invocation" ("the CLI version is pinned by the adapter task once one is tested end to end, and is recorded in the session image tag")
- `.env.example` (same variables as the README table, per `CLAUDE.md` "Backend conventions")
- `CLAUDE.md` rule 1

## Acceptance criteria
- [ ] `README.md` "Session image" shows both builds and the tag convention:
  ```bash
  podman build -t mars-session-claude:$(sed -n 's/^ARG CLAUDE_CODE_VERSION=//p' images/claude/Dockerfile) -t mars-session-claude:latest images/claude
  podman build -t mars-session-stub:latest images/stub
  ```
  plus one sentence: the claude tag is the pinned CLI version and `latest` points at it; the stub is for tests and needs no credentials; `ENGINE=podman images/smoke-test.sh` checks both.
- [ ] `README.md` "Configuration" row `SESSION_IMAGE_DEFAULT` reads "Image used by the default profile of new projects and by the startup probe (default `mars-session-claude:latest`)"; `.env.example` carries `SESSION_IMAGE_DEFAULT=mars-session-claude:latest` (commented as optional); `Config::from_env()` in `orchestrator/src/prelude/config.rs` defaults the variable to the same string instead of requiring it, if the scaffolding epic made it required.
- [ ] `README.md` "Development" layout line reads `images/          session container images (claude/, stub/)`.
- [ ] `ARCHITECTURE.md` "Session image" gains: the tag convention (`mars-session-claude:<CLAUDE_CODE_VERSION>`, `latest` alias); the stub's install path (`/usr/local/bin/claude`), fixture location (`/opt/mars-stub/fixtures/default.jsonl`, overridable with `MARS_STUB_FIXTURE`), the knobs `MARS_STUB_LINE_DELAY_MS`, `MARS_STUB_EXIT_AFTER_TURNS`, `MARS_STUB_EXIT_CODE`, its `init` line (`mcp_servers` mirrors the `--mcp-config` file), its turn rule (one turn per stdin line, synthesised replies after exhaustion, whole file under `-p`), and its exit codes (0 on EOF or SIGINT with a trailing `result`, 143 on SIGTERM). Keep it to one paragraph and a short list; no new document.
- [ ] `ARCHITECTURE.md` "Claude Code invocation" sentence about pinning is adjusted to say the version is pinned in `images/claude/Dockerfile` (`ARG CLAUDE_CODE_VERSION`) and recorded in the tag; the adapter's live probe confirms or bumps it.
- [ ] `README.md` "CI" Images row is verified against the workflow (triggers on `images/**`; builds both images on Docker and Podman; runs the smoke test) and corrected if it drifted.
- [ ] No open-questions entry is deleted by this task (item 7 belongs to the engine adapter epic).
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` still pass if `config.rs` was touched.

## Implementation notes
- Files: `README.md`, `ARCHITECTURE.md`, `.env.example`, possibly `orchestrator/src/prelude/config.rs` (one default value) and its unit test for defaults.
- The default profile of a new project uses `SESSION_IMAGE_DEFAULT` (`SPEC.md` "Agent profiles": "the built-in Claude image"); the engine adapter's startup probe pulls the same image. Say both in the README row so operators know the image must exist before first start.
- Do not describe the stub knobs anywhere else (no `images/README.md`); the rule lives in the main document.
- Keep wording consistent with the existing documents: "session image", "stub", "pinned CLI version".

## Edge cases
- If the scaffolding epic's `Config` already defaults `SESSION_IMAGE_DEFAULT`, align the README and `.env.example` to that value only if it is a sane image name; otherwise change all three together in this task.
- If the claude Dockerfile task chose a different tag scheme, this task follows the Dockerfile, not the text above.

## Testing
- `git grep SESSION_IMAGE_DEFAULT` shows the same default in `README.md`, `.env.example` and `config.rs`.
- Run the README build commands verbatim on a clean checkout with Podman; run `ENGINE=podman images/smoke-test.sh`.
- Backend chain if `config.rs` changed.

## Documentation
- This task is the documentation change: `README.md`, `ARCHITECTURE.md`, `.env.example`.

## Assumes from other epics
- "Repository scaffolding, tooling and CI" delivers `Config::from_env()` reading `SESSION_IMAGE_DEFAULT` and `.env.example`.