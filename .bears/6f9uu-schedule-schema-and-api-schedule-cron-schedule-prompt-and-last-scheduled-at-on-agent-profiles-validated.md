---
id: "6f9uu"
title: "Schedule schema and API: schedule_cron, schedule_prompt and last_scheduled_at on agent_profiles, validated"
status: open
priority: P1
created: "2026-09-21T20:25:08.694387419Z"
updated: "2026-09-21T20:25:08.694387419Z"
tags:
  - orchestrator
  - scheduler
  - profiles
  - migration
  - api
depends_on:
  - b6yk5
  - vx7sq
parent: tup8z
---

## Summary
The columns and REST fields the scheduler reads. Nothing fires yet. Implements epic `tup8z`, "Decisions" as written into `ARCHITECTURE.md`, "Scheduled agents" by `b6yk5`. Runs after the dispatcher's schema task `vx7sq`: same table, same `ProfileInput`, and it reuses that task's credential check and `max_concurrent`.

## Acceptance criteria
- [ ] One reversible migration: `agent_profiles.schedule_cron TEXT NULL`, `schedule_prompt TEXT NULL`, `last_scheduled_at TIMESTAMPTZ NULL`, with `CHECK ((schedule_cron IS NULL) = (schedule_prompt IS NULL))`. A partial index on profiles with a schedule if the job's query wants one.
- [ ] The cron crate named by the ADR is added with `cargo add`. `ProfileInput` parses `schedule_cron` as a 5-field expression (seconds and year fields, `@reboot` and other non-standard forms refused with a 400 that says what is accepted), requires a non-blank `schedule_prompt` with it, refuses a schedule on a `conversational` profile, and applies the same agent-credential rule as `auto_launch`. Decide and document a minimum period if the crate makes "every minute" expressible — the scheduler's tick is one minute, so nothing finer can be honoured.
- [ ] Clearing the schedule clears both fields and `last_scheduled_at`. `last_scheduled_at` is read-only over REST.
- [ ] Profile responses carry `schedule_cron`, `schedule_prompt`, `last_scheduled_at` and a computed `next_scheduled_at` (UTC, RFC 3339, null without a schedule) so the frontend does not need a cron parser.
- [ ] `SPEC.md` (profile endpoints, shapes, errors) and `docs/data-model.md` (`agent_profiles`) change in the same commit; `.sqlx/` committed.

## Testing
Model unit tests for the expression validation (valid, six-field, garbage, blank prompt). Route tests: set, update and clear a schedule; 400 on a conversational profile, on a bad expression, on a missing prompt, on a missing credential; `next_scheduled_at` is after now and matches the expression.