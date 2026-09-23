---
id: ub6y6
title: "Tables never push the page sideways: wrap the three bare tables and cap the remote-URL cell"
status: open
priority: P1
created: "2026-09-23T15:58:18.093143543Z"
updated: "2026-09-23T15:58:57.212039079Z"
tags:
  - frontend
  - mobile
depends_on:
  - cdkxp
  - a48hj
parent: yymt7
---

## Summary

Three tables have no scroll wrapper: the projects table (`src/pages/ProjectsPage.tsx:116`), the project sessions table (`src/pages/project/SessionsTab.tsx:210`) and the session branch table (`src/components/git/SessionBranchTable.tsx:67`, inside the session header and the sessions tab). The projects table also widens the page on any width: `ProjectRow.tsx:99` shows the remote URL as `block truncate` inside an automatic-layout cell, where `truncate` does not cap the cell, so the column grows to the URL's full width. The `SCROLLER` constant (`tableStyles.ts:22`, `max-h-96 overflow-y-auto`) is a nested scroll area that traps a touch scroll on the dashboard, users and invites tables. This task makes the table constants the one place the rule lives and applies them.

Implements `SPEC.md`, "Frontend", "Mobile layout" (no route scrolls sideways) and `CLAUDE.md`, "Frontend conventions" (tables are `tableStyles.ts`).

## Acceptance Criteria

- [ ] Every `<table>` in `src/` is inside `X_SCROLLER` or `SCROLLER` (which gains `overflow-x-auto` explicitly and a comment saying so): the three above get one; `Markdown.tsx:350` keeps its own wrapper.
- [ ] `ProjectRow.tsx:99`: the remote URL cell is capped — `max-w-0` on the `<td>` with `w-full` on the column, or `table-fixed` on that table with explicit column widths — so the truncation really truncates and the projects page never exceeds the viewport width at 360 px.
- [ ] `SCROLLER`'s `max-h-96` applies at `sm` and above only (`sm:max-h-96`): on a phone the dashboard, users and invites tables scroll with the page instead of inside a 384 px box the finger gets caught in.
- [ ] The phone column set of every table is reviewed against what a phone user needs (list below), and `secrets/columns.ts:12-14`'s "Orchestrator only" header gets a short phone label (`max-sm:` text or `abbr` with `title` and visible short text, never `title` alone).
- [ ] `TableHead`'s `TableColumn` (`src/components/TableHead.tsx`) carries the responsive `hidden` class per column as today; if any row file has drifted from its `columns.ts`, the row follows the columns.
- [ ] `tableStyles.ts`'s header comment states the rule: a table is always inside a scroller, a cell that truncates is capped by the column, and the phone column set is what identifies the row plus its state and its one action.

## Implementation Notes

- Phone column sets today (all / phone): dashboard sessions 7/4, dashboard tasks 8/5, projects 5/3, sessions tab 8/4, branch table 6/4 with three text buttons in the actions cell, profiles 8/6, shared dirs 4/3, states 5/5 with a `w-40` input, secrets 8/5, agent credentials 4/4, users 7/4, invites 6/4. Trim states and profiles to what fits 360 px with `X_SCROLLER` as the safety net.
- `SessionBranchTable.tsx:175-194`: the three actions become a `flex-wrap` cell or stack below `sm`.
- The `TaskStatesEditor` rename input (`StateRow.tsx:163`, `w-40`) becomes `w-full min-w-32`.

## Edge Cases

- A row action's inline `ConfirmPanel` spanning `colSpan` uses the column count (`TableColumn[].length`), which changes when a column is hidden by CSS but not removed — it does not change here, since hiding is CSS; keep it that way.

## Testing

- Playwright `@mobile`: `projects.spec.ts` › `the projects and sessions tables fit a phone @mobile` — create a project with a long remote URL, assert `document.documentElement.scrollWidth <= window.innerWidth` on `/projects` and on the project's sessions tab. Row in `frontend/tests/README.md`.
- Frontend chain in full. Invoke `/frontend-design` first.