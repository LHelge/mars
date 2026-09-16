---
id: m4vdn
title: "Implement ClaudeBackend::launch_command for conversational, resume and ephemeral launches"
status: open
priority: P1
created: "2026-09-16T20:27:39.404240977Z"
updated: "2026-09-16T20:27:39.404240977Z"
tags:
  - orchestrator
  - agent
depends_on:
  - w8ezk
parent: "8vnwy"
---

## Summary
Build the exact `claude` argument vector for the three launch shapes: a fresh conversational process, a resumed conversational process, and a one-shot ephemeral run. Every documented flag is emitted in a fixed order so the engine's container `Cmd` is deterministic and the fixture and probe tests can assert it byte for byte.

## Documents
- `ARCHITECTURE.md` "Claude Code invocation" (full flag list, `--include-partial-messages` by profile flag, `--bare` and `--strict-mcp-config` not used, `--resume <cli_session_id>`), "Launch sequence" (`--append-system-prompt` on every launch, `--system-prompt-snapshot off`, `--mcp-config /session/mcp.json` on every launch, ephemeral prompt = generated task message + user message), "Session container specification" (Command row).
- `docs/data-model.md` `agent_profiles` (`model` passed as `--model` when set; `system_prompt` appended on every launch; `partial_messages`).
- ADR 0003.

## Acceptance criteria
- [ ] Conversational fresh launch produces exactly: `claude --print --output-format stream-json --input-format stream-json --verbose --forward-subagent-text --system-prompt-snapshot off [--include-partial-messages] --permission-mode bypassPermissions --permission-prompts none --mcp-config /session/mcp.json [--model <model>] [--append-system-prompt <system_prompt>]`, optional groups present only when the context sets them, in that order.
- [ ] Conversational resume appends `--resume <cli_session_id>` as the last two elements.
- [ ] Ephemeral launch produces `claude -p <prompt> --output-format stream-json --verbose --forward-subagent-text --system-prompt-snapshot off [--include-partial-messages] --permission-mode bypassPermissions --permission-prompts none --mcp-config /session/mcp.json [--model <model>] [--append-system-prompt <system_prompt>]` with no `--print`, no `--input-format` and no `--resume`.
- [ ] The prompt and system prompt are passed as single argv elements, never shell-quoted or joined; empty-string `model` or `system_prompt` is treated as unset.
- [ ] `--bare` and `--strict-mcp-config` never appear.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- File: `orchestrator/src/agent/claude/launch.rs`; `ClaudeBackend::launch_command` delegates to `pub(crate) fn build_argv(ctx: &LaunchContext) -> Vec<String>`.
- Flag constants in one place (`const OUTPUT_FORMAT: &str = "stream-json"` etc.) so the probe and docs task can reference them; the binary name is `claude` (on `PATH` in the session image, `ARCHITECTURE.md` "Session image").
- `--append-system-prompt` takes the profile's current prompt verbatim; the launcher (Session lifecycle epic) passes it through `LaunchContext.system_prompt`, and this task does no templating.
- The ephemeral `prompt` is already the concatenation of the generated task message and the user's message (Session lifecycle epic joins them with a blank line); `build_argv` uses it as is.

## Edge cases
- A `model` or `system_prompt` containing spaces, quotes or newlines is one argv element; do not escape.
- `LaunchMode` makes ephemeral-with-resume impossible; no runtime validation is needed, but document that the entrypoint receives argv, not a shell string.
- Prompt length: the CLI receives it as an argument; there is no v1 limit, but note in the module that very long prompts (> 128 KiB) are the launcher's concern.

## Testing
- Unit tests in `launch.rs`: exact `Vec<String>` equality for (a) conversational, no model, no prompt, partial on; (b) conversational with model, prompt and partial off; (c) resume; (d) ephemeral with task prompt containing a newline; (e) no `--bare`/`--strict-mcp-config` anywhere; (f) empty strings treated as unset.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written.

## Assumes from other epics
- "Session lifecycle: launcher, owner, recovery and sessions API": fills `LaunchContext` from the profile and session row and joins the ephemeral prompt; passes `Command.argv` to the engine as `Cmd`.
- "Session container images: claude and stub": the entrypoint execs argv as given.