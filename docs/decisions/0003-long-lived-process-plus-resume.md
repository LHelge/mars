# 0003. Long-lived CLI process, resume for parked sessions

Status: accepted; the placement of the CLI state directory (first consequence) is superseded by 0015, which puts it per project rather than per session.

Superseded by the current [launch contract](../../ARCHITECTURE.md#claude-code-invocation) for prompt persistence: the CLI can retain a system-prompt snapshot, so launches use `--system-prompt-snapshot off` with the current profile prompt.

## Context

A conversational session must accept many user messages over hours or days. Options:

1. Resume per turn: for every user message, start `claude -p --resume <id>`, wait for `result`, exit.
2. Long-lived only: one process with `--input-format stream-json` for the life of the session; if it dies the session is over.
3. Both: a long-lived process while active, `--resume` to relaunch after the process or container has gone away.

Resume-per-turn is simple and robust but pays the CLI startup and context reload cost on every message, cannot interject mid-turn, and creates a container churn problem. Long-lived only cannot survive orchestrator restarts, idle reaping or container failure.

## Decision

Both. A conversational session runs one long-lived CLI process with `--input-format stream-json`; user messages are JSON lines on stdin, which also allows interjecting while a turn is in progress. When the session is parked (idle-reaped, container died, orchestrator chose to stop it) the next user message relaunches the CLI with `--resume <cli_session_id>` in a fresh container on the same session volume. The frontend never learns whether a message hit a running or a parked session.

Ephemeral sessions run one `-p` prompt and become `done` when `result` arrives; they are never parked or resumed. Keeping them resumable was considered (a reviewer agent could answer follow-up questions on the same context) and rejected: it leaves every finished ephemeral run lingering as resumable state, and a follow-up can start a new session from the finished branch instead.

## Consequences

- The CLI's own state (its config directory with transcripts) must live on the session volume, otherwise `--resume` has nothing to resume. The session volume therefore holds `home/`, `work/` and `log/`.
- The profile's system prompt is re-applied on every launch because `--append-system-prompt` is not persisted by the CLI across resumes.
- Two launch code paths (fresh and resume) must produce identical behaviour from the user's point of view; this is a core integration test.
- Idle sessions cost nothing but disk.
