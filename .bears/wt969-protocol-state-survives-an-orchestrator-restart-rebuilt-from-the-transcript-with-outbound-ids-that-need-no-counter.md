---
id: wt969
title: "Protocol state survives an orchestrator restart: rebuilt from the transcript, with outbound ids that need no counter"
status: open
priority: P2
created: "2026-09-21T12:13:28.797492Z"
updated: "2026-09-21T20:26:14.671912264Z"
tags:
  - orchestrator
  - agent
  - sessions
  - recovery
depends_on:
  - x8zqq
parent: fgbm3
---

## Summary
The transcript file holds only what the process wrote (ADR 0010); what the owner wrote into stdin is not in it. After an orchestrator restart the process is still running (ADR 0034) and the new owner must be able to encode the next input: it needs the protocol's session id and must not collide with a request id still in flight, and it must not re-send a reply the previous owner already sent.

## Documents
- `ARCHITECTURE.md`, "Durability and recovery", "Restart procedure"; "Agent process model" for the rule on ids.

## Acceptance criteria
- [ ] The rule is stated and enforced by the trait's documentation: everything `encode_input` needs is derivable from lines the process wrote (responses and notifications), and outbound request ids are unique without a counter (UUIDs or an owner-launch prefix).
- [ ] Recovery replays the transcript through `translate` in a mode that rebuilds state and **discards** outbound lines for already-consumed input, so no reply is sent twice.
- [ ] A turn in flight at restart is recognised as in flight from the transcript alone (needed by the turn-gating task).
- [ ] Session-owner test through the mock backend: a scripted handshake and one turn, kill and restart the owner mid-file, next input is encoded with the right session id; `events` has no gaps and no duplicates.

## Implementation notes
- `session/recovery.rs`, `session/owner.rs` (the replay at ~l.1769), `agent/state.rs`.
- Claude needs none of this; its replay behaviour must be byte-identical before and after.

## Edge cases
- A reply that was generated but not written before the crash: the process is still waiting for it. Decide and document: replies to requests seen after the recorded read offset are sent, earlier ones are not; a request straddling the boundary is answered again only if the protocol tolerates a duplicate response — record what the spike found.

## Testing
- `tests/` session-owner suite ("Session owner tests" in `CLAUDE.md`).