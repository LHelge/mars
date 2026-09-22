---
id: b6yk5
title: "ADR and ARCHITECTURE.md \"Scheduled agents\" section: write down the scheduling rules settled in planning"
status: done
priority: P1
created: "2026-09-21T20:24:54.983648270Z"
updated: "2026-09-22T07:37:05.619545551Z"
tags:
  - docs
  - adr
  - scheduler
  - orchestrator
depends_on:
  - jrgm7
parent: tup8z
attempts: 1
---

## Summary
The rules were decided with the user while planning (2026-09-21) and are listed in the epic `tup8z`, "Decisions". This task is the write-up, before any code: a new ADR in `docs/decisions/` and a "Scheduled agents" subsection in `ARCHITECTURE.md`, "Task tracker", replacing what is left of "After v1: dispatcher and scheduled agents". Documents only. Do not reopen the decisions.

## Acceptance criteria
- [ ] ADR (next free number, listed in `docs/decisions/README.md`) records each decision with the alternative rejected: 5-field cron in UTC always (rejected: instance or per-profile time zone); missed ticks skipped (rejected: one catch-up launch); overlap bounded by the unattended-launch capacity check, a refused tick skipped and logged, not queued (rejected: one live scheduled session per profile; always launch); `schedule_prompt` column required with a schedule (rejected: a `profile_schedules` table; a fixed generated message); `last_scheduled_at` as an exactly-once guard, not a catch-up cursor; ephemeral only, credential rule and `created_by` NULL as for the dispatcher.
- [ ] The cron crate is chosen here — check maintenance, 5-field support, UTC evaluation and "next occurrence after t" — and named in `ARCHITECTURE.md`, "Orchestrator internals" with the other per-concern crates; it is added with `cargo add` by the task that first uses it.
- [ ] "Scheduled agents" points to the capacity rule and the *unattended launch* definition the dispatcher section introduced rather than restating them, and says the project's `automation_paused` stops schedules too.
- [ ] The GitHub-issues instance stays described as waiting for GitHub access.
- [ ] No code changes.