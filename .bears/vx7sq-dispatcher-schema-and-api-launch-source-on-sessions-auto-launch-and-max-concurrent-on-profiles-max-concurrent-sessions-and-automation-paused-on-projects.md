---
id: vx7sq
title: "Dispatcher schema and API: launch_source on sessions, auto_launch and max_concurrent on profiles, max_concurrent_sessions and automation_paused on projects"
status: done
priority: P1
created: "2026-09-21T20:23:41.520962153Z"
updated: "2026-09-21T21:26:41.688979233Z"
tags:
  - orchestrator
  - dispatcher
  - profiles
  - projects
  - migration
  - api
depends_on:
  - jrgm7
  - rgrvp
parent: qabvt
attempts: 1
---

## Summary
The columns and REST fields the dispatcher reads, with their validation. Nothing launches yet. Implements epic `qabvt`, "Decisions" (caps, pause, eligibility) as written into `ARCHITECTURE.md`, "Dispatcher" by `jrgm7`.

Also the record of who launched a session: `sessions.launch_source`, written by the creation path `rgrvp` extracted, from its launch actor.

## Acceptance criteria
- [ ] `sessions.launch_source`: a new Postgres enum `session_launch_source` (`user`, `dispatcher`, `schedule`), `NOT NULL DEFAULT 'user'`, so every existing row is a user launch. `NewSession` carries it, the creation path sets it from the launch actor (`User` → `user`, `Dispatcher` → `dispatcher`, `Schedule` → `schedule`) and nothing else can, and `Session` in `SPEC.md`, "Sessions" and `docs/data-model.md` (`sessions`, "Enums") gains the field. It is never updated after insert; a retry or resume keeps it.
- [ ] One reversible migration (`sqlx migrate add -r`): `agent_profiles.auto_launch BOOLEAN NOT NULL DEFAULT FALSE`, `agent_profiles.max_concurrent INTEGER NOT NULL DEFAULT 1 CHECK (max_concurrent >= 1)`, `projects.max_concurrent_sessions INTEGER NULL CHECK (max_concurrent_sessions >= 1)` (NULL means no project cap), `projects.automation_paused BOOLEAN NOT NULL DEFAULT FALSE`. An index that serves "profiles with `auto_launch`" if the dispatcher's query wants one. The `.down.sql` fully reverses it.
- [ ] `models/agent_profile.rs` `ProfileInput`: `auto_launch = true` is refused on a `conversational` profile; `max_concurrent < 1` is refused; both with the model's error enum and a 400 message in `SPEC.md`'s words. `max_concurrent` is valid on any ephemeral profile whether or not `auto_launch` is set (the scheduler uses it too).
- [ ] Credential rule: saving a profile with `auto_launch = true` is refused (400) unless the profile backend's agent credential exists at `global` scope or at this project's `project` scope (ADR 0036; the lookup the launcher and the guided credential form already use). The check lives where `tup8z` can reuse it for a schedule.
- [ ] `models/project.rs` and the project update route take `max_concurrent_sessions` (nullable, `>= 1`) and `automation_paused`; seeded role profiles keep `auto_launch = false`.
- [ ] Profile and project responses carry the new fields; `SPEC.md` (profile and project endpoints, shapes and error answers) and `docs/data-model.md` (`agent_profiles`, `projects`) change in the same commit. `cargo sqlx prepare` run and `.sqlx/` committed.

## Testing
A session created over REST answers `launch_source: "user"`; one created through the entry point as `Dispatcher` or `Schedule` stores and returns that value with `created_by` NULL. Route tests per CLAUDE.md, "Testing expectations": happy path for each field on create and update; 400 for `auto_launch` on a conversational profile, for a cap below 1, for `auto_launch` with no resolvable credential (and 200 once a `project`-scope and, separately, a `global`-scope credential exists; a `user`-scope one does not satisfy it); unauthenticated and forbidden paths unchanged. Model unit tests for the validation.