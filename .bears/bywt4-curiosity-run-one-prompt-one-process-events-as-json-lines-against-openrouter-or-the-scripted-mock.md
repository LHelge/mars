---
id: bywt4
title: "curiosity run: one prompt, one process, events as JSON lines — against OpenRouter or the scripted mock"
status: open
priority: P2
created: "2026-09-21T12:19:01.235637Z"
updated: "2026-09-21T20:26:14.895017478Z"
tags:
  - curiosity
  - cli
depends_on:
  - rtfys
parent: w9nsq
---

## Summary
The smallest useful front end, so the copied parts are proven to work together before a protocol is put on top: `curiosity run [--model provider/model] [--system-prompt TEXT | --system-prompt-file PATH] [--effort …] [--max-tokens N] "<prompt>"` runs the loop in the current directory with the full tool set until the model stops calling tools, and writes every event as one JSON line to stdout.

## Documents
- `curiosity/README.md`: usage, the environment variables (`OPENROUTER_API_KEY`, `OPENROUTER_BASE_URL`, `OLLAMA_HOST`, `CURIOSITY_MODEL` as the default model), the event lines.

## Acceptance criteria
- [ ] Exit status: 0 when the run ended normally, non-zero with a final `error`-kind event when the provider failed past the retry policy or the configuration is wrong (missing key: the message names the variable, never a value).
- [ ] stdout carries only event lines; `tracing` goes to stderr, level from `RUST_LOG`/`CURIOSITY_LOG`.
- [ ] `--model mock/<path>` (or `CURIOSITY_MOCK_SCRIPT`) drives the run from a script file for the mock provider — scripted text, tool calls and usage — so an integration test runs a whole tool-using session with no network. The script format is documented; it is what Mars's end-to-end tests will use later.
- [ ] `SIGINT` cancels through the loop's cancellation token, reaps children and exits; the last line is still a well-formed event.
- [ ] A default system prompt ships in the binary (a neutral coding-agent prompt, written new — midgaard's role prompts are not copied); `--system-prompt` **appends** to it, mirroring how Mars uses `--append-system-prompt`.

## Implementation notes
- This is not the interface Mars will drive (that is `curiosity acp`, next epic); keep it thin, and keep the session-running code in the library so the ACP server calls the same function.

## Testing
- Integration tests in `curiosity/tests/` with the mock script: a run that reads, edits and runs a command in a `tempfile` directory and asserts the event sequence. The live test stays environment-gated.