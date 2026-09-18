---
id: "8vnwy"
title: Claude Code agent backend and event translation
type: epic
status: done
priority: P1
created: "2026-09-16T20:13:04.044156603Z"
updated: "2026-09-18T19:12:29.652006024Z"
tags:
  - orchestrator
  - agent
depends_on:
  - deex5
  - p5tsd
---

## Scope

`agent/`: the `AgentBackend` trait and its Claude Code implementation (ADRs 0003, 0008), including the live verification that closes `docs/open-questions.md`.

- `AgentEvent` and `TranslateState` types in `events/`, serialised exactly as `SPEC.md` "AgentEvent", `_`-prefixed internal fields stripped on read.
- `launch_command` for conversational (long-lived `--print --input-format stream-json ... [--resume]`) and ephemeral (`-p "<prompt>"`) launches with every documented flag, `--model`, `--append-system-prompt`, `--include-partial-messages` by profile flag, `--mcp-config /session/mcp.json`.
- `translate` rules: `init`, `permission_denied`, assistant blocks to `text`/`thinking`/`tool_call` (+ `subagent_start` on the subagent tool name constant matching `Task` and `Agent`), CLI user messages to `tool_result` (+ `subagent_end`), echo suppression by content hash, `stream_event` to `text_delta`, `result` with `cost_usd`, `raw` fallback, `parent_tool_use_id` propagation, 256 KiB tool-result truncation, authentication failure to fatal `error` naming the injected secret.
- `encode_input` for `message` and `answer`.
- Fixture-based tests: recorded native output under `tests/fixtures/claude/<version>/` with expected event sequences; never edit old fixtures on a version bump.
- Live probe test against the pinned CLI (runs only with credentials present): stdin message shape, mid-turn message behaviour, whether `prompt` events can occur, `--strict-mcp-config` interaction, per-turn vs cumulative `result` cost, `init` fields, multi-turn/subagent/prompt-snapshot flags. Write each answer back into `ARCHITECTURE.md`/`SPEC.md` and delete the open-question entries; remove `prompt`/`answer` from the spec if they cannot occur.

## Documents

`ARCHITECTURE.md` "Agent process model", "Claude Code invocation", "Input encoding", "Cost accounting"; `SPEC.md` "AgentEvent"; `docs/open-questions.md` items 1-6, 9; ADRs 0003, 0008.

## Acceptance criteria

- [ ] Fixture tests cover every translation rule and pass in CI without credentials.
- [ ] The live probe passes against the pinned version and its findings are recorded in the documents, with the open-questions entries deleted.
- [ ] The pinned CLI version matches the session image tag.

## Out of scope

Tailing, offsets and persistence (Session lifecycle epic); a second backend (non-goal).