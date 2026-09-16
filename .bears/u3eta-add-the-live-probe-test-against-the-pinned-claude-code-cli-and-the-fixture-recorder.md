---
id: u3eta
title: Add the live probe test against the pinned Claude Code CLI and the fixture recorder
status: open
priority: P1
created: "2026-09-16T20:29:52.595200909Z"
updated: "2026-09-16T20:29:52.595200909Z"
tags:
  - orchestrator
  - agent
  - tests
depends_on:
  - m4vdn
  - eeswv
parent: "8vnwy"
---

## Summary
Write `orchestrator/tests/claude_probe.rs`: an opt-in integration test that launches the real, pinned Claude Code CLI with the adapter's own `launch_command` and `encode_input`, answers every verification item in `docs/open-questions.md` (1, 2, 3, 4, 5, 6, 9) by observation, and records the native output as fixture files under `tests/fixtures/claude/<version>/`. It is skipped (passes with a log line) when credentials are absent, so CI stays green; when enabled it fails on any mismatch with the pinned version or the documented contract. The write-back of its findings into the documents is the separate docs task; the expected-event files beside the recordings are the fixture-suite task.

## Documents
- `ARCHITECTURE.md` "Input encoding" (the probe is the first integration test of the adapter; what it must record), "Claude Code invocation" (`--permission-prompts none` requires 2.1.259+, version pinned by the adapter task and recorded in the image tag, `--strict-mcp-config` interaction, `--system-prompt-snapshot off`, subagent `parent_tool_use_id`), "Cost accounting" (per-turn vs cumulative).
- `docs/open-questions.md` items 1, 2, 3, 4, 5, 6, 9.
- `CLAUDE.md` "Testing expectations" (fixtures per pinned CLI version; a version bump adds fixtures, never edits old ones) and rule 3 (no real credentials in fixtures).
- ADR 0003 (fresh and resume must behave identically).

## Acceptance criteria
- [ ] The test runs only when `MARS_CLAUDE_PROBE=1` and one of `ANTHROPIC_API_KEY` / `CLAUDE_CODE_OAUTH_TOKEN` is set in the environment (never both; the test fails if both are set, mirroring the launcher rule); otherwise it returns early with `eprintln!("claude probe skipped: MARS_CLAUDE_PROBE unset")` and passes.
- [ ] When enabled, it runs the `claude` binary found on `PATH` (`MARS_CLAUDE_BIN` overrides the path) inside a `tempfile` working directory with `CLAUDE_CONFIG_DIR` pointing at a second temp directory, `HOME` unchanged, and asserts `claude --version` starts with `agent::claude::CLAUDE_CLI_VERSION`; a mismatch fails the test with both versions in the message.
- [ ] Scenario `stdin_shape` (item 1): launch the conversational argv from `build_argv` (with a temp `--mcp-config` file containing an `mcpServers` entry named `mars-orchestrator` pointing at `http://127.0.0.1:1/mcp`), write `encode_input(Message { text: "Reply with exactly the word PONG." })`, and assert an `init` line, at least one `assistant` line and a `result` line arrive within 120 s; record whether `session_id` or `parent_tool_use_id` had to be added to the stdin line (try the bare shape first; only if the CLI rejects it, retry with `session_id` and fail the test so the shape is fixed in code, never silently).
- [ ] Scenario `multi_turn` (item 9): two sequential messages produce two `result` lines with `num_turns` observed, and the second turn's text answers the second message; record both `total_cost_usd` values and whether the second is cumulative (item 5) into the scenario notes file.
- [ ] Scenario `mid_turn` (item 2): send a message that takes several seconds ("Count slowly from 1 to 30, one number per line"), write a second message 1 s later, and record whether the first turn is interrupted or the second is queued; then write `{"type":"control_request","request_id":"probe-1","request":{"subtype":"interrupt"}}` during a third long turn and record whether a `control_response` line appears.
- [ ] Scenario `prompt_kind` (item 3): message "Use the AskUserQuestion tool to ask me which colour I prefer, then stop."; record every native line and whether any line requires an answer from stdin under `--permission-mode bypassPermissions --permission-prompts none`.
- [ ] Scenario `mcp_config` (item 4): with a repository `.mcp.json` in the working directory defining server `repo-local` (also pointing at an unreachable URL), run one turn without and one with `--strict-mcp-config` and record the `init.mcp_servers` names and statuses for each; also record every `init` field name present (item 6).
- [ ] Scenario `resume_prompt` (item 9): first launch with `--append-system-prompt "When asked for a codeword answer ALPHA."`, capture `init.session_id`, exit, relaunch with `--resume <id>` and `--append-system-prompt "When asked for a codeword answer BRAVO."`, ask for the codeword, and assert the answer contains `BRAVO`; record whether `init.resumed`-like fields exist.
- [ ] Scenario `subagent` (item 9): message "Use your subagent tool (Task or Agent) to list the files in this directory and report back."; assert at least one native line carries `parent_tool_use_id` and record the tool name used, which must be in `SUBAGENT_TOOL_NAMES`.
- [ ] Scenario `ephemeral`: run the ephemeral argv with prompt "Reply with exactly the word PONG." and assert `init`, `assistant`, `result` and process exit 0.
- [ ] Scenario `auth_failure`: run one conversational turn with the credential variable set to the obviously fake value `sk-ant-fake-probe-0000`, and record the native lines so the translator's authentication rule matches a real shape.
- [ ] Recording: when `MARS_RECORD_FIXTURES=1`, each scenario writes `tests/fixtures/claude/<CLAUDE_CLI_VERSION>/<scenario>.jsonl` (every stdout line verbatim) and `<scenario>.stdin.jsonl` (every line written to stdin), and appends observations to `tests/fixtures/claude/<CLAUDE_CLI_VERSION>/NOTES.md`; before writing, the recorder fails the test if any output line contains the credential value, the host home path, or a string starting with `sk-ant-` other than the fake one.
- [ ] Recording never overwrites an existing version directory: if `<version>/` exists and `MARS_RECORD_FIXTURES=1`, the test fails with `fixtures for <version> already recorded; bump CLAUDE_CLI_VERSION or delete locally`.
- [ ] Every scenario runs the recorded lines through `ClaudeBackend::translate` with a matching `TranslateState` and asserts no panic and that `init` and `result` translate to `init` and `result` events (deeper assertions belong to the fixture-suite task).
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass with the probe skipped; `MARS_CLAUDE_PROBE=1 cargo test --features integration-tests --test claude_probe -- --nocapture` passes against the pinned version.

## Implementation notes
- File: `orchestrator/tests/claude_probe.rs`; helpers in `orchestrator/tests/common/claude_probe.rs` (process spawn with `tokio::process::Command`, stdin writer, line reader with per-scenario timeout, recorder).
- The probe uses the crate's public `agent::claude::{ClaudeBackend, build_argv, CLAUDE_CLI_VERSION, SUBAGENT_TOOL_NAMES}` (make `build_argv` `pub` for this) and `agent::{LaunchContext, LaunchMode, TranslateState, TranslateConfig}`; it does not use `TestApp`, the engine or Postgres.
- The `--mcp-config` path passed by `build_argv` is fixed at `/session/mcp.json`; the probe substitutes its temp path by replacing that argv element, asserting it was present.
- Set `CLAUDE_CLI_VERSION` in `agent/claude/mod.rs` to the version the Session images epic pinned in `images/claude/Dockerfile` before running; the probe is the end-to-end test that justifies the pin (`ARCHITECTURE.md`: "the CLI version is pinned by the adapter task once one is tested end to end").
- Timeouts: 120 s per scenario; kill the child on timeout and fail with the lines seen so far (`eprintln!`, never `tracing::info!` with payloads).
- Keep prompts tiny to bound cost; the whole probe should cost well under one US dollar.
- `NOTES.md` is human-readable Markdown: one heading per scenario, the observed facts (queued vs interrupted, cumulative vs per-turn cost, init field list, mcp_servers lists), no raw payloads.

## Edge cases
- Both credential variables set: fail before spawning (same rule as the launcher).
- The CLI prompting for onboarding/login in the temp config dir: pass `--permission-prompts none` already covers permissions; if the CLI blocks on a first-run prompt, the notes record it and the test fails so the entrypoint contract can be adjusted (Session images epic).
- Network failure to the fake MCP URL must not fail the turn; if it does, record it: that is a finding for `launch_warning` handling.
- Exit codes: record the child's exit status per scenario (`SIGINT`-ended turns are the engine tests' concern; here the child is closed by closing stdin).

## Testing
- This task is a test; the commands above are its acceptance. Additionally a unit test in `tests/common/claude_probe.rs` covers the credential-leak scanner (a line containing the fake key passes, a line containing the real env value fails) and the no-overwrite guard using a temp fixtures root.

## Documentation
- none in this task: the findings are written into `ARCHITECTURE.md`, `SPEC.md` and `docs/open-questions.md` by the follow-up docs task, which reads `NOTES.md`.

## Assumes from other epics
- "Session container images: claude and stub": the pinned CLI version in `images/claude/Dockerfile`; the probe runs the same version on the host.
- "Repository scaffolding, tooling and CI": Orchestrator CI runs `cargo test` without `MARS_CLAUDE_PROBE`, so the probe is skipped there.