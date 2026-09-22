---
id: v9rjg
title: Switch the icon set to Lucide behind one icon map, and add icons across the console
status: open
priority: P2
created: "2026-09-22T19:16:23.765882974Z"
updated: "2026-09-22T19:16:23.765882974Z"
tags:
  - frontend
  - docs
depends_on:
  - ujccg
  - snn6t
  - ky3hc
  - "34y5m"
  - "23pue"
parent: gtbp5
---

Heroicons (~300 icons, 7 used in 8 files) has no git icons (branch, merge, commit, pull request), and git is a large part of the console. Switch to `lucide-react` (same 24px outline style, git/terminal/bot icons, tree-shaken per icon). Invoke `/frontend-design` first.

- `npm install lucide-react`, `npm uninstall @heroicons/react`.
- One `src/components/icons.ts` maps concepts to icons (`Icon.logout`, `Icon.help`, `Icon.push`, `Icon.merge`, `Icon.branch`, `Icon.fetch`, `Icon.sync`, `Icon.edit`, `Icon.delete`, `Icon.close`, `Icon.lock`, `Icon.warning`, …). Components import from it, never from `lucide-react` directly (an ESLint `no-restricted-imports` rule enforces it). Replace the existing 7 Heroicons uses.
- Icon pass, always beside text and never replacing a label (icon-only buttons keep an `aria-label`; decorative icons are `aria-hidden`): the nav (including Help), git actions (fetch, sync, merge, rebase, push), session controls (launch, end, retry, delete), copy/link buttons, and state indicators (running, parked, failed, needs human) — quiet, consistent sizing via one class constant.
- Docs: `CLAUDE.md` frontend stack line (Heroicons → Lucide, and the icon-map rule), `ARCHITECTURE.md` "Frontend architecture" if it names the icon library, and a new ADR in `docs/decisions/` recording Lucide over Heroicons (no git icons) and over mixing two sets.
- Tests: update any test that queries by the old icon markup; lint/tsc/build/unit/E2E green.