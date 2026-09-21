---
id: duzjs
title: "Unattended-launch capacity check: one answer to \"may this profile launch now?\" over the three caps and the project pause"
status: done
priority: P1
created: "2026-09-21T20:23:53.227431319Z"
updated: "2026-09-21T21:56:48.146826591Z"
tags:
  - orchestrator
  - dispatcher
  - sessions
  - config
depends_on:
  - vx7sq
parent: qabvt
attempts: 1
---

## Summary
The rule both the dispatcher and the scheduler (`tup8z`) apply before launching, in one place (`ARCHITECTURE.md`, "Dispatcher", the unattended-launch capacity rule; epic `qabvt`, "Decisions"). An unattended launch is allowed only while the project is not `automation_paused` and the live (`creating` or `running`) sessions are below the profile's `max_concurrent`, the project's `max_concurrent_sessions` (when set) and the instance's `AUTOMATION_MAX_SESSIONS`. All live sessions count, whoever launched them. User launches never consult it.

## Acceptance criteria
- [ ] `Config` gains `AUTOMATION_MAX_SESSIONS` (optional, positive integer, documented default; a value below 1 fails fast at startup naming the variable). `README.md`, "Configuration" and `.env.example` carry it.
- [ ] `SessionRepository` gains the live-session counts (per profile, per project, instance-wide) as `sqlx::query!` with the scope in the `WHERE`; one round trip if practical. `.sqlx/` committed.
- [ ] A function in `session/` (next to the extracted creation path is the natural home) answers with an enum that says *which* bound refused — paused, profile cap, project cap, instance cap — so callers log the reason with structured fields and nothing string-formatted.
- [ ] The check is advisory by design: it is read outside the launch transaction, and the jobs are single-flight (a tick never overlaps its own previous run), so document in the function's rustdoc why that is sufficient and that a concurrent user launch may overshoot a cap by design.

## Testing
Integration tests with `TestApp`: each bound refuses at its limit and allows below it; `parked`, `done` and `failed` sessions do not count; a NULL project cap is no cap; the pause refuses regardless of counts; user-launched sessions count.