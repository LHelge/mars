---
id: k3gn4
title: The session header's cost and tokens refresh after every result, not only on a state change
status: done
priority: P2
created: "2026-09-25T17:57:19.331194466Z"
updated: "2026-09-25T18:37:36.231052327Z"
tags:
  - orchestrator
  - frontend
  - sessions
  - bug
attempts: 1
---

Implements `SPEC.md`, "Session stream" (`session` frame) and "Frontend" ("The session header shows `cost_usd` and the token counters").

**Bug.** The header reads `session.cost_usd` from the store's `Session`, which only a `session` frame updates, and the socket sends one only on a `session_state` notice (`ws/mod.rs`, `on_notice`). A `result` commits its cost to the row but notifies only `session_events`, so while a session runs the header shows the cost as of its last state change; it catches up on park or reload.

**Fix.** After the socket streams a `result` event it sends a fresh `session` frame. `SPEC.md` session-frame row updated. The transcript's result line shows the CLI's running total, not the turn's cost — label it so (`session/messages/ResultMessage.tsx`).

**Tests.** A ws integration test: a committed `result` event is followed by a `session` frame carrying the new `cost_usd`.