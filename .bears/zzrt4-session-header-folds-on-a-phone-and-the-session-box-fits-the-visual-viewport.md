---
id: zzrt4
title: Session header folds on a phone and the session box fits the visual viewport
status: done
priority: P1
created: "2026-09-23T15:58:18.044207558Z"
updated: "2026-09-27T17:37:10.773958100Z"
tags:
  - frontend
  - mobile
  - sessions
depends_on:
  - cdkxp
  - yaxpf
parent: yymt7
attempts: 1
---

## Summary

The session box is `h-[calc(100dvh-6rem)] min-h-[28rem] overflow-hidden` (`src/session/SessionView.tsx:39`) with a header that does not scroll (`SessionHeader.tsx:66-131`). On a phone the header wraps to the state pill, the title, eight metadata fields, three links, `SessionActions`, a possible error alert and, when `branch` is toggled, a whole `SessionBranchTable` with its forms — and the transcript, `min-h-0 flex-1`, is pushed to nothing. The 448 px minimum is taller than a landscape phone, and iOS does not shrink `dvh` for the keyboard, so focusing the composer scrolls the header off screen and the box with it. This task folds the header below `sm`, moves the branch section out of the header's flow on a phone, and sizes the box from the visual viewport so the composer stays above the keyboard.

Implements `SPEC.md`, "Frontend", "Session state" (the header) and "Mobile layout".

## Acceptance Criteria

- [ ] Below `sm`: the header shows one row — state pill, connection dot, title (truncated, still editable on tap), and a disclosure button "Details" with `aria-expanded` — and the `Metadata` block (`SessionHeader.tsx:245-263`), `LaunchSourceTag`, the link row and `SessionActions` render under the disclosure, closed by default. At `sm` and above the header is unchanged.
- [ ] The `branch` section (`SessionHeader.tsx:130,162-196`) is no longer inside the fixed header's flow below `sm`: it opens as a sheet over the session box (reuse the sheet frame that task yaxpf, which this depends on, added for the side panel) and the `SessionBranchTable` inside is wrapped in `X_SCROLLER` at every width.
- [ ] The header never takes more than half the session box: `max-h-[50%] overflow-y-auto` on the header below `sm`, so a long error alert scrolls inside the header instead of eating the transcript.
- [ ] `SessionView.tsx:39`: `min-h-[28rem]` becomes `min-h-[16rem]` below `sm`; the box's height below `sm` follows `window.visualViewport` (a `useVisualViewportHeight()` hook beside `useMediaQuery` in `src/hooks/`, subscribing to `resize` and `scroll` of `visualViewport`, `undefined` where it is missing) so the composer sits above the on-screen keyboard instead of the page scrolling; `PageLayout`'s frame stays in the calculation.
- [ ] Long monospace ids in the metadata (`SessionHeader.tsx:251,256`, branch and base names) get `break-all` or `truncate` with the full value as text in the disclosure, never clipped by the box's `overflow-hidden`.
- [ ] `SPEC.md`, "Frontend", "Session state" (or the session header sentence nearest it) documents the folded header and the branch sheet; "Mobile layout" gets the one-line summary.

## Implementation Notes

- The disclosure is the existing `src/session/Disclosure.tsx` pattern if it fits, otherwise a plain button with `aria-expanded` and a `hidden` block.
- `visualViewport` height: `Math.round(window.visualViewport.height)`; style `height` via an inline `style` only below `sm`, the `calc(100dvh-6rem)` class at `sm` and above. Mind the `6rem` — measure what `PageLayout`'s header and paddings really take at phone width after the nav task lands (they run in parallel: read the DOM height of `header` with a ref rather than a constant if the two disagree).
- `useStickToBottom` must re-pin after a height change that shrinks the transcript, or the newest line sits under the composer: call the scroll-to-bottom on a resize when it was pinned.
- Task yaxpf has already added the "Panels" opener to the same link row: it folds under the disclosure with the other links.

## Edge Cases

- Title edit on a phone: blur saves and Escape cancels (`SessionHeader.tsx:352-361`); a phone has no Escape, so add a visible cancel (an `×` button beside the input while editing) at every width.
- A session in `creating` has no branch; the branch button stays hidden as today.

## Testing

- Vitest for `useVisualViewportHeight` with a stubbed `visualViewport`; `SessionHeader` unit tests for the disclosure.
- Playwright `@mobile`: `session-view.spec.ts` › `the session header folds on a phone and the transcript keeps its height @mobile` — the metadata is hidden until Details is tapped, and the transcript's bounding box is at least 40 % of the viewport height with Details open. Row in `frontend/tests/README.md`.
- Frontend chain in full. Invoke `/frontend-design` first.