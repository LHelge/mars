---
id: yc2ah
title: "Mid-turn input is a backend policy: the owner queues through or holds until the turn ends"
status: open
priority: P2
created: "2026-09-21T12:16:51.108773Z"
updated: "2026-09-21T20:26:14.699360102Z"
tags:
  - orchestrator
  - agent
  - sessions
depends_on:
  - x8zqq
  - wt969
parent: fgbm3
---

## Summary
Claude Code queues a message written during a turn, and the owner relies on it: "the owner never needs to hold input back" (`ARCHITECTURE.md`, "Input encoding"). ACP defines no such queue — a second `session/prompt` during a running one is whatever the spike observed. Make it a declared property of the backend and let the owner hold input when the backend says so.

## Documents
- `ARCHITECTURE.md`, "Input encoding" and "Session owner task"; `SPEC.md`, "WebSocket: session stream" if the acknowledgement of a held input differs from a written one.

## Acceptance criteria
- [ ] `AgentBackend::mid_turn_input() -> MidTurnInput` with `Forward` (Claude: write immediately) and `Hold` (write when the turn in progress ends). A `Steer` variant is **not** added: nothing uses it.
- [ ] The owner knows whether a turn is in progress from the events it has appended (input written → turn open; `result` → turn closed), including after a restart (restart task).
- [ ] A held input is still recorded immediately as a `user_message` event, exactly as today, so the UI shows it; it is written to the process in arrival order when the turn closes.
- [ ] Stop, park and idle-reaping with held input behave as they do today with queued input: nothing is lost, the input is delivered to the next process or reported `input_rejected` by the existing rules (ADR 0020).
- [ ] Claude declares `Forward`; no Claude test changes.

## Implementation notes
- `session/owner.rs` (the input path around `encode_input`, ~l.1005), `agent/mod.rs`, `agent/mock.rs` (policy settable per test).

## Edge cases
- A turn that never closes (process died): the existing exit handling flushes or rejects held input; assert it.
- Two held inputs: both delivered, in order, each as its own turn.

## Testing
- Owner tests through the mock backend with `Hold`: input during a scripted turn reaches stdin only after the scripted `result`.