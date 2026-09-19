---
id: dsfs3
title: Decide StatusBadge colour for `error` and stop PasswordChangeForm stealing focus on the settings page
status: open
priority: P3
created: "2026-09-19T11:29:34.264466241Z"
updated: "2026-09-19T11:29:34.264466241Z"
tags:
  - frontend
  - design
parent: vdscb
---

## Summary
Two small findings from reviewing the Frontend foundation epic (2f5u2), both left as the task texts specified them.

1. `frontend/src/components/StatusBadge.tsx` renders `state="error"` (a project whose clone failed) in the neutral muted colour, because task ea6xs said only the four state tokens carry colour. A failed clone reads as quiet as `ready`. Decide whether `error` shares `--color-state-failed`; it is a one-line change in `COLOURS`.
2. `frontend/src/components/PasswordChangeForm.tsx` sets `autoFocus` on the current-password field. That is right on `/change-password` but on `/settings` it pulls focus to the third section of the page on load. Make autofocus a prop (default off) and pass it from `ChangePasswordPage` only.

## Documents
- `CLAUDE.md` "Frontend conventions": quiet colour reserved for state (running, parked, failed, needs human).
- `SPEC.md` "Frontend", Routes: `/settings`, `/change-password`.

## Acceptance criteria
- [ ] The colour decision for `error` is made and, if changed, covered by the badge's usage on the dashboard and later project list.
- [ ] Loading `/settings` leaves focus where the browser put it; `/change-password` still focuses the first field.
- [ ] `cd frontend && npm run lint && npx tsc -b && npm run build && npm run test:unit` passes.

## Documentation
- none.

Discovered while implementing 2f5u2 (tasks ea6xs, hwebm, 4qe6q).