---
id: r6yek
title: Write the probe findings back into ARCHITECTURE.md and SPEC.md, close open questions 1-6 and 9, and pin the CLI version to the image tag
status: open
priority: P1
created: "2026-09-16T20:30:57.001248535Z"
updated: "2026-09-16T20:31:02.096232577Z"
tags:
  - orchestrator
  - agent
  - docs
depends_on:
  - u3eta
  - "5wg2m"
parent: "8vnwy"
---

## Summary
Resolve the adapter's verification items with the evidence from the probe's `NOTES.md`: record each answer in the document section that depends on it, delete items 1-6 and 9 from `docs/open-questions.md`, remove the `prompt` kind and `answer` input from the spec and the code if the probe showed they cannot occur, set the cost accumulation rule, and make the pinned CLI version a checked invariant between `agent::claude::CLAUDE_CLI_VERSION` and the session image tag. This is the epic's documentation write-back; it is a mandatory part of the change (`CLAUDE.md` rule 1).

## Documents
- `docs/open-questions.md` items 1, 2, 3, 4, 5, 6, 9 (delete on resolution; each answer moves into the document that depends on it; an ADR only when a real alternative was rejected).
- `ARCHITECTURE.md` "Input encoding" (stdin shape, mid-turn behaviour, whether `prompt` occurs: "the observed behaviour is written back into this section"), "Cost accounting" (per-turn vs cumulative rule), "Claude Code invocation" (`--strict-mcp-config` interaction, `init` fields, pinned version statement, `--permission-prompts none` minimum version), "Session image" (version recorded in the image tag), "Event delivery" (answer/`reply_to` rejection rule, only if `answer` is removed).
- `SPEC.md` "AgentEvent" (`init` optional fields; `prompt` kind; translation rules confirmed), "WebSocket: session stream" (`SessionInput.answer`, `input_rejected` reason), "Sessions" (`POST /sessions/{id}/input` body).
- `CLAUDE.md` "Testing expectations" (fixture rule; unchanged unless the layout differs).

## Acceptance criteria
- [ ] `ARCHITECTURE.md` "Input encoding" states the verified stdin line shape (and whether `session_id`/`parent_tool_use_id` are required), whether a mid-turn message interrupts or queues, and whether `control_request`/`interrupt` exists, replacing the "is therefore a live probe" wording with the recorded facts and the probe's name (`tests/claude_probe.rs`).
- [ ] `ARCHITECTURE.md` "Cost accounting" states one rule: either "`result` reports per-turn values; the owner sums them" or "`result` reports cumulative values for the process; the owner adds the increase over the previous `result` of the same run", and the "one of the adapter's verification items" sentence is removed.
- [ ] `ARCHITECTURE.md` "Claude Code invocation" records the observed `init` field list, the `--strict-mcp-config` finding (whether repository `.mcp.json` servers are dropped), keeps `--strict-mcp-config` unused unless the finding justifies otherwise, and states the pinned version literally (`Claude Code <version>`), with the image tag convention (`mars-session-claude:<version>`) cross-referenced in "Session image".
- [ ] If the probe showed no prompt-like message can reach the host: `prompt` is removed from `SPEC.md` "AgentEvent", `answer` from `SessionInput` in "WebSocket: session stream", the `reply_to` field from `user_message`, the answer-rejection sentence from `ARCHITECTURE.md` "Event delivery", the `Prompt` variant and `SessionInput::Answer` from `events/` and `agent/claude/input.rs`, and a short ADR `docs/decisions/0032-no-interactive-prompts-in-v1.md` records the rejected alternative; if prompts can occur, `SPEC.md` documents the observed native shape and the translator gains the `prompt` rule as a new task linked to this one.
- [ ] `docs/open-questions.md` no longer lists items 1-6 and 9; items 7 and 8 stay for the engine epic; the intro paragraph is adjusted so it does not claim unresolved adapter items.
- [ ] `agent::claude::CLAUDE_CLI_VERSION` equals the version pinned in `images/claude/Dockerfile`, and a unit test in `agent/claude/mod.rs` reads `../images/claude/Dockerfile` (path relative to `CARGO_MANIFEST_DIR`) and asserts the pin line contains `CLAUDE_CLI_VERSION`, failing with both values when they differ; the Dockerfile's pin variable name is stated in `ARCHITECTURE.md` "Session image".
- [ ] Any translator behaviour the findings change (e.g. `encode_input` needing `session_id`, `init` lacking `tools`) is adjusted in code with its unit tests in the same commit, and the fixture suite still passes.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Source of truth for the answers: `orchestrator/tests/fixtures/claude/<version>/NOTES.md` and the recorded `.jsonl` files from the probe task; do not restate raw payloads in the documents, state the rule.
- Keep each document's rule in the main document; an ADR is only for a rejected alternative (removing `prompt`/`answer` qualifies; a cost rule does not).
- If the Dockerfile pin is an `ARG` (e.g. `ARG CLAUDE_CODE_VERSION=2.1.259`), the test parses `ARG <name>=<value>`; coordinate the name with the Session images epic and record it.
- The `sessions` DTO and owner accumulation code are the Session lifecycle epic's; this task only fixes the rule they implement.

## Edge cases
- If the probe could not answer an item (e.g. `control_request` undocumented and silently ignored), write the observed behaviour ("ignored without response on <version>") rather than leaving the item open; the item is still deleted, with the limitation noted in the section.
- Removing `Prompt`/`Answer` touches types other epics may already reference: grep `Prompt`, `Answer`, `reply_to`, `prompt_id` across `orchestrator/` and `frontend/src/types/` and fix every use in the same commit.
- Do not edit recorded fixtures while adjusting the translator; add synthetic fixtures if a new rule needs coverage.

## Testing
- The version-pin unit test above; the full fixture suite (`cargo test --features integration-tests --test agent_fixtures`); grep-based check that `docs/open-questions.md` contains no `1.`-`6.` or `9.` verification entries (manual, in the PR description).
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- `ARCHITECTURE.md`, `SPEC.md`, `docs/open-questions.md`, possibly a new ADR 0032 and `frontend/src/types/` type mirrors, all in the same commit as any code change.

## Assumes from other epics
- "Session container images: claude and stub": `images/claude/Dockerfile` pins the CLI version in a greppable `ARG`/`ENV` line and tags the image `mars-session-claude:<version>`.
- "Session lifecycle: launcher, owner, recovery and sessions API": implements the cost accumulation rule this task fixes and the input/answer handling as documented after this task.
- "Frontend project and session views": mirrors any `AgentEvent`/`SessionInput` change in `frontend/src/types/`.