---
id: b9n9c
title: "Task board and drawer on a phone: one column per swipe, a column picker, and a draft guard on Close"
status: done
priority: P2
created: "2026-09-23T15:58:18.162517672Z"
updated: "2026-09-27T16:47:53.645371605Z"
tags:
  - frontend
  - mobile
  - tracker
depends_on:
  - cdkxp
  - a48hj
parent: yymt7
attempts: 1
---

## Summary

The board is a horizontal strip of `w-64` columns with proximity snap (`src/tasks/TaskBoard.tsx:219,277`): a phone shows about 1.3 columns and nothing says there are six. The task drawer already fills a phone (`TaskDetail.tsx:196,204`), but its unsaved-draft guard runs only on Escape (`:128-166`, `drawerEscape.ts:46-55`); Close and a tap on the overlay call `close()` and throw the draft away (`:92-94,199-202,221`). Below `sm` the columns are one viewport wide and snap one at a time, a row of column chips above the strip names them and scrolls to them, and the draft guard covers every way out of the drawer.

Implements `SPEC.md`, "Frontend", "Task board" and "Mobile layout".

## Acceptance Criteria

- [ ] Below `sm`: each `BoardColumn` is `w-[calc(100vw-2rem)]` (the gutter) and the strip is `snap-x snap-mandatory`; at `sm`+ the columns stay `w-64` with proximity snap.
- [ ] Below `sm` a row of chips above the strip — one per column, name and count, the one in view marked (`aria-current`) — scrolls the strip to that column with `scrollIntoView({ inline: "start" })`; which column is in view comes from an `IntersectionObserver` over the columns (threshold 0.6), not from scroll math. At `sm`+ the row is not rendered.
- [ ] The `CreateTaskForm` and `TaskSearch` (`max-w-72`) are full width below `sm`.
- [ ] `TaskDetail`: the Close button and the overlay tap go through the same decision as Escape — when `typed.current && hasDraftText(dialog)`, an inline `ConfirmPanel` ("Discard unsaved text?", `confirmLabel="Discard and close"`) replaces the `armed` question for pointer users; Escape keeps its press-twice behaviour. One owner of the decision: extend `drawerEscape.ts`'s `escapeAction` into a `closeAction({ via: "escape" | "button" })` so the rule is tested once.
- [ ] The drawer's sticky header (`TaskDetail.tsx:205`) keeps `#number`, the title and Close on one row at 360 px; the copy link moves into the `TaskActions` bar below `sm`.
- [ ] `SPEC.md`, "Frontend", "Task board": the drawer paragraph's Escape rule gains the button and overlay case; "Mobile layout" gets the board sentence.

## Implementation Notes

- `100vw` includes the scrollbar on desktop but the rule applies below `sm` only; use `w-[calc(100vw-2rem)]` under `max-sm:` or a CSS variable set from the strip's `clientWidth` if the gutter ever changes.
- `IntersectionObserver` root is the strip element; observe each column; the chip row is `overflow-x-auto` itself with the active chip kept in view.
- `drawerEscape.ts` is pure and unit-tested; extend those tests.

## Edge Cases

- One column: the chip row is still rendered (it says which state the project has) but nothing scrolls.
- `UNKNOWN_COLUMN`: same chip, muted, as its heading is.
- A drawer opened by direct link (`/projects/:id/tasks/:number`) on a phone: the board behind it is inert; the chip row must not fire `scrollIntoView` while the dialog is modal (it only runs on chip click).

## Testing

- Vitest: `drawerEscape.test.ts` for `closeAction`; `TaskBoard.test.tsx` for the chip row (jsdom has no `IntersectionObserver`; stub it).
- Playwright `@mobile`: `tasks.spec.ts` › `a phone swipes the board one column at a time and a tapped Close keeps a draft @mobile` — tap the `review` chip and assert that column's bounding box starts at the gutter; open a task, type in the comment box, tap Close, assert the confirmation, cancel, assert the text is still there. Row in `frontend/tests/README.md` under "Task board".
- Frontend chain in full. Invoke `/frontend-design` first.