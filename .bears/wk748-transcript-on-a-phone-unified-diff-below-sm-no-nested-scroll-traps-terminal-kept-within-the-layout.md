---
id: wk748
title: "Transcript on a phone: unified diff below sm, no nested scroll traps, terminal kept within the layout"
status: done
priority: P3
created: "2026-09-23T15:58:18.185824361Z"
updated: "2026-09-27T17:37:10.849147542Z"
tags:
  - frontend
  - mobile
  - sessions
depends_on:
  - cdkxp
  - a48hj
parent: yymt7
attempts: 1
---

## Summary

Three transcript-level details are wrong on a phone: `DiffView` offers side-by-side at every width (`src/components/DiffView.tsx:117-128,158-163`), which at 360 px is two 150 px panes; `SubagentToolRenderer.tsx:39` nests a `max-h-96 overflow-y-auto` scroller inside the transcript's own scroller, where a touch scroll gets caught; and the terminal (`TerminalView.tsx`) needs a fit on resize inside whatever frame the side-panel task gives it, and its known limitations on a phone need saying. The transcript itself already contains wide content (`CodeBlock`, `ShellToolRenderer`, `JsonTree`, `Markdown` all scroll their own boxes).

Implements `SPEC.md`, "Frontend", "Transcript rendering", "Session side panel" (the terminal sentence) and "Mobile layout".

## Acceptance Criteria

- [ ] `DiffView`: below `sm` the layout is unified and the "Side by side" toggle is not rendered; a stored preference (if there is one — read the component) is kept for `sm`+. The line-number gutter (`:45`, `w-10`) stays.
- [ ] `SubagentToolRenderer.tsx:39`: no inner `max-h`/`overflow-y-auto` below `sm` (`sm:max-h-96 sm:overflow-y-auto`); the group's own collapse (`CollapsibleLines`/`Disclosure`) is what bounds it on a phone. The same review for any other `max-h-*` + `overflow-y-auto` inside `src/session/` (`grep -rn 'max-h-' src/session src/components`).
- [ ] `TerminalView`: the xterm `fit` addon runs on mount, on the panel's `ResizeObserver`, and on `visualViewport` resize (the keyboard), so the terminal never exceeds its frame; the font stays 12.
- [ ] `SPEC.md`, "Frontend", "Session side panel": one sentence that the terminal is desktop-first — a phone keyboard has no Ctrl, Tab, Esc or arrows, and Mars does not add a key bar; "Transcript rendering": the diff is unified below `sm`; "Mobile layout" gets the summary line.
- [ ] `docs/open-questions.md` gets no entry: the key bar is a decision (out of scope), recorded in the epic's ADR from the foundation task if that ADR's alternatives list does not already cover it — add it there.

## Implementation Notes

- `DiffView` is also used by `HandoffDiff` and the Changes panel: check every caller keeps its props.
- `useStickToBottom` (`src/session/useStickToBottom.ts`) unpins on upward movement only; a nested scroller that stops existing below `sm` changes no pin logic.

## Edge Cases

- A diff opened in the drawer (`HandoffDiff`) at 360 px: unified, the file list (`ChangesFileList`) wraps.

## Testing

- Vitest: `DiffView.test.tsx` renders both widths via a `matchMedia` stub (or the class-based approach needs no JS — prefer classes; then the test only asserts the toggle's `max-sm:hidden`).
- Playwright `@mobile`: `sessions.spec.ts` › `an edit diff is unified on a phone and the transcript scrolls as one @mobile` — the stub's edit-tool turn renders without the side-by-side toggle and `document.documentElement.scrollWidth <= window.innerWidth`. Row in `frontend/tests/README.md` under "Transcript rendering".
- Frontend chain in full. Invoke `/frontend-design` first.