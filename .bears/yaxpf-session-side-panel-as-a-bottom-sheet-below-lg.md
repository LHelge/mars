---
id: yaxpf
title: Session side panel as a bottom sheet below lg
status: in_progress
priority: P1
created: "2026-09-23T15:58:18.017855776Z"
updated: "2026-09-27T15:47:28.038702643Z"
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

Below 1024 px the open side panel (`w-96 max-w-full shrink-0`, `src/session/SidePanel.tsx:100`) takes the whole row of `SessionView.tsx:63-70` and squeezes the transcript and the composer to zero width; collapsed, its rail still costs about 30 px of a 360 px screen. Below `lg` the panel becomes a sheet that opens over the transcript from the bottom, with the same tabs, and the rail becomes one button in the session header's action row. At `lg` and above nothing changes.

Implements `SPEC.md`, "Frontend", "Session side panel" and "Mobile layout".

## Acceptance Criteria

- [ ] At `lg` and above: unchanged — the panel opens beside the transcript, collapses to the rail, tabs and keyboard behaviour as today.
- [ ] Below `lg` (`useMediaQuery("(min-width: 1024px)")` from the foundation task): the panel is a sheet over the session box, `absolute inset-x-0 bottom-0` inside the box, about 85 % of its height, with the tab strip at its top, a close button, and a dimming overlay over the transcript that closes it on tap. The composer and transcript keep the full width when it is closed.
- [ ] The opener below `lg` is one button — icon `Icon.panelOpen`, label "Panels" visible, `aria-expanded` — in the session header's link row (`SessionHeader.tsx:98-118`, beside `branch`/`project`), not a rail beside the transcript. The rail is only rendered at `lg`.
- [ ] Opening the Terminal tab in the sheet still fetches the xterm chunk lazily and opens the exec only when selected (the registry order rule of `sidePanels.ts` is untouched); the terminal fits the sheet's width (`fit` addon on open and on resize).
- [ ] Escape closes the sheet; focus goes to the opener on close; the sheet has `role="dialog"` with `aria-label="Session panels"` and keeps the ARIA tablist inside it.
- [ ] The sheet's `open` state is reset when the viewport crosses `lg` (a sheet left open on a phone does not become an open column on rotate and vice versa is fine either way, but nothing stays half-rendered).
- [ ] `SPEC.md`, "Frontend", "Session side panel": replace "It opens beside the transcript on a screen at least 1024 px wide and starts collapsed to a rail below that" with the sheet rule; "Mobile layout" gets the one-line summary.

## Implementation Notes

- `SessionView.tsx:63-70` gains `relative` on the row so the sheet positions inside the session box; keep the box's `overflow-hidden`.
- Two renderings of one component is the wrong shape: keep `SidePanel` as the tablist and panel, and let a small `SidePanelFrame` (or a `mode: "column" | "sheet"` prop) decide the chrome. The tab registry and the derived-tab rule stay untouched.
- The opener button belongs to `SessionHeader`, which is also reshaped by the task "Session header folds on a phone" — keep the change to the link row minimal (one button) to ease the merge, and coordinate through the `sessionUi` store if the open state must be shared (`src/session/sessionUi.ts` already carries per-session UI state such as the resend request).
- `tests/git.spec.ts:200` assumes the panel is open at the desktop viewport; it must still pass on `chromium`.

## Edge Cases

- A `creating` session offers no Changes panel; the sheet shows Tasks (derived-tab rule).
- The overlay tap must not reach the transcript's `useStickToBottom` scroll handler.

## Testing

- Vitest: `SidePanel.test.tsx` covers both modes through a `matchMedia` stub.
- Playwright `@mobile`: `session-view.spec.ts` › `the side panel opens as a sheet on a phone and the transcript keeps the width @mobile` — launch a stub session, assert the transcript's bounding box spans the viewport width, open the sheet, switch to Tasks, close it with the overlay. Row in `frontend/tests/README.md` under "Session side panel" and "Mobile layout".
- Frontend chain in full. Invoke `/frontend-design` first.