# 0032. A session is `running` when stdin is attached, not when `init` arrives

Status: accepted.

## Context

The launch sequence was written so that the session owner waited for the CLI's `system`/`init` line before it set the session `running`, stored `cli_session_id` and flushed queued input. The live probe against the pinned Claude Code version (2.1.274) showed that this cannot work: under `--print --input-format stream-json` the CLI writes no output at all, `system`/`init` included, until it has read its first line from stdin. A probe that waited for `init` before writing saw nothing for 120 seconds; one that wrote first had its `init` within a second (`orchestrator/tests/fixtures/claude/2.1.274/NOTES.md`, `stdin_shape`; `images/claude/VERIFY.md`, "Observed on 2.1.274").

The documented order is therefore a deadlock: the owner waits for `init`, the CLI waits for input. A conversational session created without a first message would never leave `creating` at all, and one created with a message would only work because the owner happened to write before waiting.

Two ways out were available.

## Decision

A conversational session becomes `running` as soon as its container has started and its stdin is attached. Queued input is flushed immediately at that point. `cli_session_id` is stored when the first `init` event arrives and is null until then, so a `running` session that has not been sent a message has none.

The rejected alternative was to keep the old order by writing a throwaway first line — a no-op or "say nothing" message — to force the CLI to emit `init` early. It was rejected because it costs a model turn on every launch, in money and in latency, and because it puts a message in the conversation that the user did not write and would see in the transcript. Neither cost buys anything: the only thing the early `init` provides is `cli_session_id`, which is needed for resume, and a session that has exchanged nothing has no conversation to resume.

## Consequences

- The `running` state means "the process is alive and can be written to", not "the process has spoken". `cli_session_id` is nullable in the schema and on the session DTO, and the UI shows it as absent until the first turn.
- The launcher's resume path uses `--resume <cli_session_id>` only when one is recorded; a session parked or failed before its first message relaunches fresh in its existing checkout. Resume is otherwise unaffected, because the id is already known from the earlier process.
- Ephemeral launches are unaffected: `claude -p "<prompt>"` carries its prompt in argv and writes `init` with nothing on stdin.
- The `launch_warning` for an MCP server that is not connected is derived from the `init` event, so for a conversational session it appears with the first turn rather than at container start.
- The stub CLI (`images/stub/claude`) matches this behaviour: under `--input-format stream-json` it writes nothing before its first stdin line, so an owner that reintroduced the old ordering fails in the end-to-end tests instead of passing against a friendlier stub.
