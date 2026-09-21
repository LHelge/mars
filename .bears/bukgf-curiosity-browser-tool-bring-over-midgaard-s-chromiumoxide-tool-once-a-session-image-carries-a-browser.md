---
id: bukgf
title: "Curiosity browser tool: bring over midgaard's chromiumoxide tool once a session image carries a browser"
status: open
priority: P3
created: "2026-09-21T12:23:50.112512Z"
updated: "2026-09-21T12:23:50.112512Z"
tags:
  - curiosity
  - tools
  - backlog
depends_on:
  - p7ghm
---

## Summary
`../midgaard/src/tools/browser.rs` (1,840 lines, `chromiumoxide`) was left out of the foundation copy: it needs a Chromium in the image and a heavy dependency, and nothing in Mars's first Curiosity sessions requires it. Useful later for agents that verify frontend work.

## Acceptance criteria
- [ ] Decide the packaging first: a Cargo feature (`browser`) so the default static binary stays small, and an image variant (`mars-session-curiosity-browser`) or a documented way for a project image to add Chromium.
- [ ] The tool is offered to the model only when a browser is found at start; absence is not an error.
- [ ] Runs as uid 1000 with `CapDrop: ALL` and `no-new-privileges` (`ARCHITECTURE.md`, "Session container specification"): Chromium's own sandbox will not work there — record the flags used and why that is acceptable inside the container boundary (ADR 0012).
- [ ] Copied tests pass; musl static linking still works with the feature off.