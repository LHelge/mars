---
id: "5453d"
title: Allow retrying failed transcript history loads and report their actual status
status: done
priority: P2
created: "2026-09-21T10:44:34.504746905Z"
updated: "2026-09-21T18:14:08.240924055Z"
tags:
  - frontend
  - technical-review
  - bug
  - session
parent: "579dz"
attempts: 1
---

Problem: Transcript remembers requestedFor.current = oldestSeq before loading. loadOlder catches failure and logs it, leaving oldestSeq and the request marker unchanged. Subsequent scrolling refuses the same page forever until remount, while hasMore keeps the 'Loading earlier messages' indicator visible even when no request runs.

Acceptance: expose distinguishable idle/loading/error history state and an observable completion outcome. Release the duplicate-request guard after failure and provide a usable retry action. Render loading only during an actual request, retain the current transcript on failure, coalesce simultaneous requests, and preserve scroll anchoring on successful prepend. Add a fail-then-retry-success regression and loading-indicator coverage.

References: frontend/src/session/Transcript.tsx:92; frontend/src/session/useSessionSocket.ts:350; frontend/src/session/SessionView.tsx loadOlder adapter. Contract: SPEC.md, "Frontend", session state and transcript rendering.

Merged from the second review (2026-09-21), same finding plus a second way to reach it:
- The only trigger for loadOlder is onScroll (Transcript.tsx:86-105, :130-135). A page of 200 events can fold into one or two rows, because text_delta events are tiny, so the container may not overflow, no scroll event ever fires, and older history is unreachable with the spinner showing forever. Acceptance addition: when `hasMore` and `scrollHeight <= clientHeight + NEAR_TOP_PX`, request the next page from a layout effect (bounded, one page at a time); cover it with a test whose first page folds to a non-overflowing transcript.
- A history page that contains an event kind the client does not know currently throws out of prependHistory and loses the whole page, which then wedges on the guard above; the fold's missing default arm is tracked in the boundary-validation task (3q5pg).
- `useCallback` on handleScroll (Transcript.tsx:86-105) is inert because it depends on `stick`, a fresh object literal each render (useStickToBottom.ts:151); depend on stick.onScroll / stick.isPinned when this handler is reworked.