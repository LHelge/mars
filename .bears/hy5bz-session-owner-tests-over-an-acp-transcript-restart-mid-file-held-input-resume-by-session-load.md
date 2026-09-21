---
id: hy5bz
title: "Session-owner tests over an ACP transcript: restart mid-file, held input, resume by session/load"
status: open
priority: P2
created: "2026-09-21T12:21:49.665350Z"
updated: "2026-09-21T20:26:15.144630276Z"
tags:
  - orchestrator
  - agent
  - acp
  - tests
  - sessions
depends_on:
  - sw5d3
  - qvvu8
parent: eydgf
---

## Summary
`CLAUDE.md`, "Testing expectations": session owner tests feed a transcript file line by line, kill and restart the owner mid-file, and assert `events` has no gaps and no duplicates. Prove the same for the real ACP adapter (the seam epic proved it for the mock backend), plus the two behaviours that are new with this backend.

## Acceptance criteria
- [ ] Restart mid-turn: the new owner rebuilds the ACP state from the transcript, does not re-send the handshake or a permission answer, recognises the turn as open, and encodes the next input with the right session id after the turn closes.
- [ ] Held input: a message sent during a turn is recorded as `user_message` at once and reaches stdin only after the prompt response line; two held messages arrive in order as two turns.
- [ ] Park and resume: a parked Curiosity session relaunches with `session/load <cli_session_id>`; a session parked before any message (id already known for this backend) also loads. "Fresh and resume launches behave identically for the user" (ADR 0003) is asserted on the event stream.
- [ ] Cost: two turns each reporting their own cost sum to the session counters (per-turn rule), and a restart between them does not double-count.
- [ ] Ephemeral: the prompt is delivered as decided in the enum task, the session ends `done` on the prompt response, and nothing is written to stdin afterwards.

## Implementation notes
- The existing owner suites and the mock engine; the agent side is a scripted transcript, not a process.

## Testing
- This task is tests; the full backend chain passes, including `--features integration-tests` clippy.