---
id: dtq74
title: "Curiosity sessions persist and load: a conversation survives the process, session/load continues it"
status: open
priority: P2
created: "2026-09-21T12:19:46.122775Z"
updated: "2026-09-21T20:26:14.978152496Z"
tags:
  - curiosity
  - acp
  - persistence
depends_on:
  - pfs5r
parent: vj82v
---

## Summary
Mars parks a session by stopping its container and resumes it in a new one on the same volumes (ADR 0003); the CLI's own state lives in a per-project directory that survives (ADR 0015). Curiosity therefore persists each conversation under a state directory and implements `session/load`. midgaard designed its events so a turn can be "reconstructed from the log alone" but never resumed a process; this task makes that real.

## Documents
- `curiosity/README.md`: `CURIOSITY_HOME` (default `~/.curiosity`), the on-disk layout, the durability guarantee, what `session/load` replays.

## Acceptance criteria
- [ ] State directory from `CURIOSITY_HOME`; sessions at `sessions/<session-id>.jsonl`, append-only, one persisted event per line, flushed at least at every turn boundary and after every tool result, so a `SIGKILL` loses at most the model call in flight.
- [ ] `session/load` with a known id rebuilds the model-visible history (user messages, assistant text, tool calls and results) and continues; `initialize` advertises `loadSession: true`. Whether history is replayed to the client as `session/update` follows the ACP specification — the Mars adapter will ignore a replay, since Mars already has the events (state this in the README for the adapter's author).
- [ ] An unknown id is a JSON-RPC error, not a fresh session. A log truncated mid-line (crash during write) loads up to the last complete line.
- [ ] A turn that was interrupted mid tool call loads into a state the provider accepts (no dangling tool call without a result): synthesize an "interrupted" tool result.
- [ ] The session id is stable across processes and is what `session/new` returned — it is what Mars stores as `cli_session_id`.
- [ ] Many sessions of one project share the directory concurrently (parallel containers, one bind): no shared mutable file, no lock needed; assert two processes writing different sessions do not interfere.
- [ ] The system prompt and model are **not** persisted: the launch's argv wins on load, which is what lets a changed Mars profile prompt apply on resume (ADR 0003, consequences).

## Testing
- Integration test: run a turn, kill the process, start another on the same `CURIOSITY_HOME`, `session/load`, run a second turn whose mock script asserts it saw the first turn's history. Truncated-log and dangling-tool-call cases as unit tests.