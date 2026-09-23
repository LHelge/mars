---
id: "235cs"
title: "Navigation on a phone: icon-only top nav below sm and wrapped project tabs"
status: open
priority: P2
created: "2026-09-23T15:58:18.139418140Z"
updated: "2026-09-23T15:58:57.261813195Z"
tags:
  - frontend
  - mobile
depends_on:
  - cdkxp
  - a48hj
parent: yymt7
---

## Summary

`PageLayout`'s single header row (`src/components/PageLayout.tsx:51-107`) gives the nav about 150 px on a 360 px phone: four or five text links scroll sideways with no cue, while "Log out" keeps its text. `ProjectTabs` (`src/pages/project/ProjectTabs.tsx:27`) is six tabs, one of them "Shared directories", in the same kind of strip. A dense console does not want a hamburger; it wants the same entries, smaller. Below `sm` the nav links are icon-only with their label as `aria-label`, the account cluster is two icons, and the project tabs wrap into two rows.

Implements `SPEC.md`, "Frontend", "Mobile layout" and the "Routes" paragraph's nav description.

## Acceptance Criteria

- [ ] Below `sm`: each `NAV` entry renders its icon with the label `sr-only` (`max-sm:sr-only`, as the Help link already does at `:90`), each link at least 44 px wide on a coarse pointer (`TAP`), the active entry still marked by the accent underline; the whole nav fits 360 px with no `overflow-x` scrolling for an admin (five entries plus brand, help and log out).
- [ ] "Log out" is icon-only below `sm` with `aria-label="Log out"`; the username stays hidden below `sm` as today.
- [ ] The header row keeps `overflow-x-auto` as the safety net for a width below 320 px, and the brand "mars." stays.
- [ ] `ProjectTabs`: `flex-wrap` below `sm` (`max-sm:flex-wrap`), `gap-x-4 gap-y-0`, so the six tabs take two rows at 360 px and one row at `sm`+ as today; `aria-current="page"` unchanged.
- [ ] `PageLayout`'s `main` keeps `px-4` (the 16 px gutter); the title/actions row (`:118-131`) stacks below `sm` when it has actions (`max-sm:flex-col max-sm:items-start`).
- [ ] `SPEC.md`, "Frontend", "Mobile layout" gets the nav sentence; the "Routes" paragraph's description of the top bar, if it names the labels, says they are icons below `sm`.

## Implementation Notes

- A `NavLink` with an `sr-only` label still has an accessible name; do not add `aria-label` on top of visible text at `sm`+ (a duplicated name).
- `PageLayout` is on the first-paint path (`scripts/check-entry-chunk.mjs` budget); no new dependency, and the icons are already imported.
- The session task "Session header folds on a phone" measures `PageLayout`'s header height; keep the phone header at one row of the same height as today so the `6rem` there stays right, or measure it with a ref there.

## Edge Cases

- Landscape phone (≈740 px wide) is `sm`: text labels return; fine.

## Testing

- Vitest: `PageLayout.test.tsx` (if present, else add one) asserts every nav link has an accessible name at both renderings.
- Playwright `@mobile`: `smoke.spec.ts` › `the top nav and project tabs fit a phone @mobile` — assert the nav has no horizontal scroll (`scrollWidth === clientWidth`) as an admin, and that every project tab is within the viewport. Row in `frontend/tests/README.md`.
- Frontend chain in full. Invoke `/frontend-design` first.