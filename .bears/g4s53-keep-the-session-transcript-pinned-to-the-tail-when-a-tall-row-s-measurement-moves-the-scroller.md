---
id: g4s53
title: Keep the session transcript pinned to the tail when a tall row's measurement moves the scroller
status: done
priority: P3
created: "2026-09-20T18:38:08.440823647Z"
updated: "2026-09-20T20:10:16.720665603Z"
tags:
  - frontend
  - sessions
parent: "6s8j7"
attempts: 1
---

## Summary
Found by the 2acdq session E2E specs. On a session carrying a very tall row (the stub fixture's 8 KiB tool result), `useStickToBottom` loses `pinned` without the user scrolling: the virtualizer's post-measure correction fires a `scroll` whose distance from the bottom exceeds `NEAR_BOTTOM_PX`, the hook reads that as the reader scrolling away, "Jump to latest N" appears and new output no longer follows. The E2E spec works around it with a `pinToLatest` helper that presses "Jump to latest".

## Documents
- `SPEC.md` "Frontend" (session transcript: virtualised list, renderers, 40-line collapse). The follow-the-tail behaviour is not specified; add one sentence.

## Acceptance criteria
- [ ] A scroll event caused by the virtualizer's own measurement or by content growth does not unpin; only a user-initiated scroll away from the bottom does (for example by tracking wheel/touch/keyboard/scrollbar intent, or by re-pinning when the scroll was programmatic). Say in the commit which.
- [ ] Unit test beside `useStickToBottom` reproducing a measurement-driven scroll larger than `NEAR_BOTTOM_PX` while pinned.
- [ ] `frontend/tests/sessions.spec.ts`: the `pinToLatest` workaround is removed where it only compensated for this, and the first-turn scenario asserts the tail stays in view.
- [ ] `SPEC.md` "Frontend" says the transcript follows the tail until the reader scrolls away and offers "Jump to latest"; frontend quality chain passes.
