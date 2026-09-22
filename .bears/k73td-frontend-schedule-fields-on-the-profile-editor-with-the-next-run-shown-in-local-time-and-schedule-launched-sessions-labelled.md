---
id: k73td
title: "Frontend: schedule fields on the profile editor with the next run shown in local time, and schedule-launched sessions labelled"
status: in_progress
priority: P1
created: "2026-09-21T20:25:31.307977137Z"
updated: "2026-09-22T08:29:36.328952881Z"
tags:
  - frontend
  - scheduler
  - profiles
depends_on:
  - "6f9uu"
  - nsvc5
parent: tup8z
attempts: 1
---

## Summary
The controls for the schedule fields `6f9uu` adds (`SPEC.md`, profile endpoints; "Frontend"). Runs after the dispatcher's profile-editor task `nsvc5` — same form. Invoke `/frontend-design` first.

## Acceptance criteria
- [ ] `src/types/` mirrors `schedule_cron`, `schedule_prompt`, `last_scheduled_at`, `next_scheduled_at`.
- [ ] Profile editor, ephemeral profiles only: a cron expression field (monospace, labelled as UTC, with the accepted 5-field form as help text and two or three example expressions), a schedule prompt textarea required with it, and a read-only "next run" and "last run" rendered from the server's timestamps in the viewer's local time with the UTC instant available. No cron parser in the bundle: validity and the next run are the server's.
- [ ] The server's 400s (bad expression, missing prompt, missing credential) are shown in their own words on the right field.
- [ ] The profile list marks profiles that have a schedule or auto-launch, so automation is visible without opening each editor.
- [ ] A session with `launch_source: "schedule"` reads as launched by a schedule — `nsvc5` already renders the label from the field; verify it on a real scheduled session and adjust wording only.
- [ ] New `data-testid`s are constants in `src/utils/testIds.ts`.

## Testing
Vitest for any pure helpers. Playwright is the epic's last task.