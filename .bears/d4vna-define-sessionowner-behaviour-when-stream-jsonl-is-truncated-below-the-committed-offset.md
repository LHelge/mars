---
id: d4vna
title: Define SessionOwner behaviour when stream.jsonl is truncated below the committed offset
status: open
priority: P3
created: "2026-09-18T22:14:14.858432603Z"
updated: "2026-09-18T22:14:14.858432603Z"
tags:
  - orchestrator
  - sessions
parent: s52qg
---

## Summary
Found while implementing qtx4x. When `log/stream.jsonl` shrinks, `SessionOwner` (`orchestrator/src/session/owner.rs`, `handle_truncation`) logs `warn!` and continues from the new EOF without rewinding, as `ARCHITECTURE.md` "Durability and recovery" requires. But if the file was truncated *below* an offset that is already committed, the stored `MAX(_offset)` stays ahead of the file: new lines end at offsets lower than the committed one, so the offset recheck in `SessionRepository::append_native_line` and the resume point after a restart no longer mean anything. Nothing in Mars truncates the file (`SessionDirs::ensure` only touches it), so this is a hand-edited or replaced-file scenario.

## Documents
- `ARCHITECTURE.md` "Durability and recovery" (decide and record the behaviour).

## Acceptance criteria
- [ ] The behaviour is decided and written down: for example fail the session with a clear `error`, or stand the owner down with an `error` event, rather than silently committing offsets that go backwards.
- [ ] A test in `orchestrator/tests/session_owner.rs` truncates the transcript below the committed offset and asserts the decided outcome.

## Reference
Bears qtx4x (report caveat).