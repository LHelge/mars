---
id: yq6c3
title: "Faster local verification for implement-epic: one Postgres per test process and a private target directory for the coordinator"
type: epic
status: done
priority: P1
created: "2026-09-17T20:26:41.921401600Z"
updated: "2026-09-17T21:43:27.384458212Z"
tags:
  - orchestrator
  - infra
  - tests
---

## Why

Measured on the development machine on 2026-09-17, the chain `merge-task.sh` runs on every merged task costs about six minutes: 65 s forced rebuild and link of all 34 test binaries, 29 s + 8 s for the two clippy runs, and 264 s for the 782-test suite. The suite is almost entirely Postgres startup: every integration test starts its own `postgres:18` container, cargo runs the binaries one after another, and the four-test health binary alone takes 11 s. The rebuild and the clippy runs are forced by `find src tests migrations -exec touch` in the merge script, which exists only because the coordinator's `main` shares `orchestrator/target` with the subagents' worktrees and has been served stale sibling binaries.

## Scope

- The test harness starts one Postgres per test process and gives each test its own database from a migrated template, so the suite runs in well under a minute; the engine tests keep their own containers.
- The coordinator's chain builds in a target directory no worktree shares, so the merge script stops touching every source file and builds incrementally.

## Documents

`CLAUDE.md` "Testing expectations" and "Code quality"; `README.md` "Development"; `.claude/skills/implement-epic/SKILL.md` (pitfalls, merging) and `references/dispatch-prompt.md`.

## Acceptance criteria

- [ ] `cargo test --features integration-tests` with `DOCKER_HOST` set completes in under 60 s wall time on the development machine with the same tests passing, and leaves no container behind.
- [ ] `merge-task.sh` no longer touches sources; a merge of a one-file change runs the backend chain in incremental time.

## Out of scope

`mold` as the linker, running the merge chain in the background and the Podman socket check in the skill: recorded as follow-ups in the same survey, deferred by the user.