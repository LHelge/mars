---
id: baenx
title: "No fact lives only in a tooltip: visible text for the title-only reasons"
status: done
priority: P2
created: "2026-09-23T15:58:18.116439679Z"
updated: "2026-09-27T17:37:10.811854634Z"
tags:
  - frontend
  - mobile
  - a11y
depends_on:
  - cdkxp
  - a48hj
parent: yymt7
attempts: 1
---

## Summary

A phone shows no `title` tooltip, and neither does a screen reader reliably. Several facts a user acts on live only there: why a failed session failed in the project's sessions list, why launching from a task is disabled, why a state cannot be removed, a scheduled profile's cron and next run, and what each chip on a task card means. This task applies the tooltip rule of `SPEC.md`, "Frontend", "Mobile layout" — a `title` is never the only place a fact lives — to those places, and states which `title`s are decoration (full ids behind short ids, absolute times behind relative ones) and stay.

Implements `SPEC.md`, "Frontend", "Mobile layout" (the tooltip rule); touches "Scheduled profiles" and "Task board".

## Acceptance Criteria

- [ ] `SessionStatePill` (`src/components/SessionStatePill.tsx:27`): a `failed` pill in the sessions tab (`SessionsTab.tsx:223-226`) is followed by the error as visible muted text on its own line below `md` and in the existing hidden column at `md`+ if one exists, or by a disclosure "Why?" that reveals it; the session header already shows the error (`SessionHeader.tsx:125`).
- [ ] `LaunchForTask.tsx:171,178`: the disabled reason renders as a `text-xs` line under the disabled button; the `title` may stay in addition.
- [ ] `StateRow.tsx:236,240`: the "cannot remove" reason is visible at every width (drop the `hidden md:inline`; wrap instead).
- [ ] `ProfilesTab.tsx:285`: the `schedule` chip's expression and next run are visible text in the row (a second line under the chip below `lg`, a column at `lg`+), and `SPEC.md`, "Frontend", "Scheduled profiles" no longer says the tooltip carries them.
- [ ] `TaskCard.tsx:66,89,98,107,118,138`: each chip has an `aria-label` that is the meaning, and the board has one legend line (`TaskBoard.tsx`, under the search field, `text-xs text-console-muted`, collapsible, one line) naming the glyphs and chips; `TaskBoard.tsx:282,288`'s column-kind glyphs are in the same legend.
- [ ] `UserRow.tsx:151` (self-delete), `ProjectHeader.tsx:157` (what Fetch does), `AddStateForm.tsx:120`, `LaunchSourceTag.tsx:28`, `SessionBranchTable.tsx:155,164`, `DashboardPage.tsx:241`: each reviewed and given visible text or an `aria-label`, or listed in SPEC as decoration.
- [ ] `SPEC.md`, "Frontend", "Mobile layout" lists what a `title` may carry alone: the full form of a shortened id (the copy link carries the full one), the absolute time behind a relative one, and a repeated label; everything else is visible text.

## Implementation Notes

- The rule wants one component for "a reason under a control": a `Reason` or `FieldNote`-like `text-xs text-console-muted` line. Check `FieldShell`'s hint slot before adding a component; the disabled launch button and the state row may both be able to use an existing hint.
- The legend is one sentence, not a table: `▸ queue  ! human  ■ terminal · P1 priority · ⤷ child · ⛔ blocked · ⇄ 2 dependencies · 1 session` in the board's own glyphs; read `taskChrome.ts` and `taskStateRules.ts` for the actual marks.

## Edge Cases

- A `failed` session's error can be long (a container's last stderr line): `line-clamp-2` with the full text in a disclosure.

## Testing

- Vitest: `TaskCard.test.tsx` asserts each chip's accessible name.
- Playwright `@mobile`: `task-sessions.spec.ts` › `a failed session's reason is visible on a phone without a tooltip @mobile` — end a stub session in failure (the stub image's failing fixture) and assert the reason text is visible in the project's sessions tab. Row in `frontend/tests/README.md`.
- Frontend chain in full. Invoke `/frontend-design` first.