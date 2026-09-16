# 0003. Long-lived CLI process, resume for parked sessions

Status: accepted; the placement of the CLI state directory (first consequence) is superseded by 0015, which puts it per project rather than per session.

Superseded by the current [launch contract](../../ARCHITECTURE.md#claude-code-invocation) for prompt persistence: the CLI can retain a system-prompt snapshot, so launches use `--system-prompt-snapshot off` with the current profile prompt.

## Context

A conversational session must accept many user messages over hours or days. Options considered:

1. Resume per turn: `claude -p --resume <id>` per message. Robust, but pays CLI startup and context reload every message, cannot interject mid-turn, and churns containers.
2. Long-lived only: one process with `--input-format stream-json`; if it dies the session is over. Cannot survive orchestrator restarts, idle reaping or container failure.
3. Both. Chosen.

## Decision

A conversational session runs one long-lived CLI process fed JSON lines on stdin, which also allows interjecting mid-turn. When the session is parked (idle-reaped, container died, orchestrator stopped it), the next message relaunches the CLI with `--resume <cli_session_id>` in a fresh container on the same session volume. The frontend never learns whether a message hit a running or a parked session.

Ephemeral sessions run one `-p` prompt and finish when `result` arrives; they are never resumed. Making them resumable, so a reviewer could answer follow-ups, was rejected: every finished run would linger as resumable state, and a follow-up can start a new session from the finished branch.

## Consequences

- The CLI's own state must live on a mounted volume, otherwise `--resume` has nothing to resume.
- The profile's system prompt is re-applied on every launch.
- Fresh and resume launches must behave identically for the user; this is a core integration test.
- Idle sessions cost nothing but disk.
