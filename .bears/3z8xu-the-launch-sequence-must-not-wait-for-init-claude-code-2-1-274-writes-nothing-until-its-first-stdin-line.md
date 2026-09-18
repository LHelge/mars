---
id: "3z8xu"
title: "The launch sequence must not wait for init: Claude Code 2.1.274 writes nothing until its first stdin line"
status: done
priority: P1
created: "2026-09-18T18:08:12.814321683Z"
updated: "2026-09-18T18:37:03.293736590Z"
tags:
  - orchestrator
  - agent
  - images
  - docs
parent: "8vnwy"
attempts: 1
---

## Summary
Found while running the live probe (task u3eta): under `--print --input-format stream-json` the pinned CLI writes no output at all, `system`/`init` included, until the first line arrives on stdin. A probe that waited for `init` before writing saw zero lines for 120 s; writing first produced `init` within a second (`orchestrator/tests/fixtures/claude/2.1.274/NOTES.md`, `stdin_shape`). `ARCHITECTURE.md` "Launch sequence" has the owner wait for `system/init {session_id}` before it sets the session `running`, stores `cli_session_id` and flushes queued input, which would deadlock against the real CLI: the owner waits for `init`, the CLI waits for input. A conversational session created without a first message would never leave `creating`. The stub hides this because it writes its first `init` before reading any input (task ngr69 chose that to satisfy the documented sequence).

## Documents
- `ARCHITECTURE.md` "Launch sequence" (diagram and text), "Session image" (stub bullets), "Durability and recovery" if it relies on `init` for adoption.
- `SPEC.md` session states (`creating` to `running`), `cli_session_id` on the session DTO.
- `images/claude/VERIFY.md` "Observed on 2.1.274"; `orchestrator/tests/fixtures/claude/2.1.274/NOTES.md`.

## Acceptance criteria
- [ ] `ARCHITECTURE.md` "Launch sequence" no longer orders `init` before the first stdin write: the session becomes `running` when the container has started and stdin is attached, queued input is flushed immediately, and `cli_session_id` is stored when the first `init` event arrives (nullable until then; a session with no message yet has none). An ADR records the decision if a real alternative is rejected (for example sending a no-op first line to force `init`).
- [ ] Resume is unaffected (`cli_session_id` is already known); the text says so. Ephemeral launches are unaffected (`-p` writes `init` with no stdin).
- [ ] The stub matches the real CLI: no `init` before the first stdin line under `--input-format stream-json`; `images/smoke-test.sh`, the stub unit tests, `images/stub/fixtures/README.md` and `ARCHITECTURE.md` "Session image" change in the same commit.
- [ ] The Session lifecycle epic tasks that implement the launcher and owner cite the corrected sequence (update their bodies, or note which need it).

## Notes
- The translator already emits one `init` event per process (task gukce), so nothing changes there.