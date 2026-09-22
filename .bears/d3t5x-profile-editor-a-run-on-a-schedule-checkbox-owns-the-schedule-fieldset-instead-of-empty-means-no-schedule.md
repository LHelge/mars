---
id: d3t5x
title: "Profile editor: a \"Run on a schedule\" checkbox owns the schedule fieldset instead of \"empty means no schedule\""
status: done
priority: P2
created: "2026-09-22T13:02:32.687972872Z"
updated: "2026-09-22T13:16:40.651615259Z"
tags:
  - frontend
  - scheduler
  - profiles
  - ux
attempts: 1
---

## Summary
Manual testing of `tup8z` (2026-09-22): clearing the cron field to disable a schedule is not discoverable even with the hint. The schedule fieldset of `ProfileEditor` (`SPEC.md`, "Frontend" → "Scheduled profiles") gets a checkbox that enables it: unchecked, the cron field, prompt and read-only instants are disabled and the save sends `schedule_cron: null, schedule_prompt: null`; checked, the fields are editable and the local required-pair rules apply. The typed values are kept while unchecked so re-ticking restores them, as the kind switch already does.

## Acceptance criteria
- [ ] A checkbox at the top of the Schedule fieldset; its state is derived on load from whether the stored profile has a schedule.
- [ ] Unchecked: both controls disabled, no local errors, `toInput` sends the null pair whatever the boxes hold.
- [ ] Checked with both fields blank: the local rule refuses the save on the cron field ("required"), instead of silently saving no schedule.
- [ ] `SPEC.md`, "Frontend" → "Scheduled profiles" describes the checkbox; Vitest over `toInput`/`scheduleErrors`; the Playwright schedule scenario ticks the box.