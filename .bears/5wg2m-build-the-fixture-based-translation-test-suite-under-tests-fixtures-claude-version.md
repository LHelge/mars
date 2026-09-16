---
id: "5wg2m"
title: Build the fixture-based translation test suite under tests/fixtures/claude/<version>/
status: open
priority: P1
created: "2026-09-16T20:30:24.273141218Z"
updated: "2026-09-16T20:30:24.273141218Z"
tags:
  - orchestrator
  - agent
  - tests
depends_on:
  - eeswv
  - u3eta
parent: "8vnwy"
---

## Summary
Turn the recorded native output into the permanent, credential-free regression suite that proves every translation rule per pinned CLI version: a manifest per scenario with the `TranslateState` setup and the exact expected `AgentEvent` sequence, a runner that discovers every `tests/fixtures/claude/<version>/` directory, and synthetic scenarios for rules the live recordings cannot exercise deterministically (256 KiB truncation, echo suppression, denial dedupe, non-JSON lines). This suite is what CI runs on every change to the translator.

## Documents
- `SPEC.md` "AgentEvent" (every translation rule listed there must have at least one fixture assertion).
- `CLAUDE.md` "Testing expectations" ("Event translation tests are fixture-based ... A CLI version bump adds fixtures, never edits old ones").
- `ARCHITECTURE.md` "Claude Code invocation" (subagent tool name constant proven per version).
- ADR 0008 ("Translation is a pure function ... unit-tested against recorded fixtures per CLI version").

## Acceptance criteria
- [ ] Layout: `orchestrator/tests/fixtures/claude/<version>/<scenario>.jsonl` (native lines, from the recorder or hand-written for synthetic scenarios), `<scenario>.stdin.jsonl` (optional; lines the owner would have written, used to seed `record_sent_input`), `<scenario>.expected.json` (`{ "state": { "resumed": bool, "partial_messages": bool, "credential": { "name": "...", "scope": "..." } | null }, "events": [ AgentEvent, ... ] }`); `README.md` in `tests/fixtures/claude/` explains the layout and the never-edit rule.
- [ ] Runner `orchestrator/tests/agent_fixtures.rs`: for every version directory and every `*.jsonl` that has an `.expected.json`, builds the state, seeds sent-input hashes from the `.stdin.jsonl` text fields, translates line by line, and asserts the concatenated events equal `expected.events` (JSON equality after serialising through `AgentEvent`, so field names are checked against the spec); the failure message names version, scenario, first differing index and both events.
- [ ] A scenario listed in `.jsonl` without `.expected.json` fails the runner with `missing expected events for <version>/<scenario>` so recordings cannot be forgotten.
- [ ] For the pinned version, expected files exist for every recorded scenario from the probe task (`stdin_shape`, `multi_turn`, `mid_turn`, `prompt_kind`, `mcp_config`, `resume_prompt`, `subagent`, `ephemeral`, `auth_failure`), and the `subagent` scenario's expected events contain `subagent_start` and `subagent_end` (proving `SUBAGENT_TOOL_NAMES` for this version).
- [ ] Synthetic scenarios under the same version directory (hand-written, prefixed `synthetic_`) cover: `synthetic_truncation` (a tool_result over 256 KiB → `truncated: true` and exact byte length), `synthetic_echo` (a text-only `user` line matching a stdin line → no event; a non-matching one → `raw`), `synthetic_denials` (system `permission_denied` plus the same id in `result.permission_denials` → one event), `synthetic_raw` (non-JSON line, unknown type, unknown block type), `synthetic_partial` (`stream_event` deltas → `text_delta`, other stream events dropped), `synthetic_init_resumed` (`state.resumed = true` → `init.resumed = true`), `synthetic_auth_no_credential` (auth failure with `credential: null`).
- [ ] A test asserts that every `AgentEventBody` kind that the Claude translator can emit (`init`, `text_delta`, `text`, `thinking`, `tool_call`, `tool_result`, `permission_denied`, `subagent_start`, `subagent_end`, `result`, `error`, `raw`) appears at least once across the pinned version's expected files, so a rule cannot be silently uncovered.
- [ ] The suite runs in CI without credentials or Docker: `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/tests/agent_fixtures.rs`, `orchestrator/tests/fixtures/claude/README.md`, `orchestrator/tests/fixtures/claude/<version>/*.jsonl|*.expected.json`.
- Use `include_str!`-free directory walking with `std::fs::read_dir` from `env!("CARGO_MANIFEST_DIR")` so new versions are picked up without editing the runner.
- Expected events are written by hand from the recordings (an implementer reads the `.jsonl`, writes the expected sequence); do not generate them by running the translator and saving the output unreviewed, since that would only prove the translator agrees with itself. A helper binary or `MARS_WRITE_EXPECTED=1` mode that prints the translator's output as a starting point is acceptable if its result is reviewed line by line before commit.
- Recorded lines must already be free of credentials (probe recorder guarantee); add a test that greps every fixture file for `sk-ant-` other than the fake probe key and for `Bearer ` followed by anything other than `<token>`.

## Edge cases
- Recordings contain host-specific values (temp paths, session ids, timestamps, costs): expected files copy them verbatim; the fixtures are per version and per recording, not templates.
- Large synthetic fixtures: generate the 300 KiB tool result at test time from a small `.jsonl` marker rather than committing 300 KiB of text; document the convention in `README.md` (`{"__generate":"tool_result_bytes","n":307200,...}` expanded by the runner).
- Events from one native line are compared as a group; the runner must not reorder.

## Testing
- This task is the test suite; the acceptance commands above must pass. Add a negative test that a deliberately wrong expected file (in a temp dir, not under `fixtures/`) makes the runner report the first differing index.

## Documentation
- `CLAUDE.md` "Testing expectations" already states the convention; add the fixture directory `README.md` in this task. No other document changes.

## Assumes from other epics
- none.