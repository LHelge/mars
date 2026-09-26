---
id: "8mj5h"
title: Move the project route tests' local HTTP and bare-repository helpers onto tests/common
status: in_progress
priority: P3
created: "2026-09-18T12:15:49.991291150Z"
updated: "2026-09-26T17:16:40.899316056Z"
tags:
  - orchestrator
  - tests
  - projects
depends_on:
  - xfu5s
attempts: 1
---

## Summary
Found while closing epic `pkaee`. `tests/common/git.rs` (`BareFixture`, task `xfu5s`) is the shared bare-repository fixture, but `tests/projects.rs`, `tests/projects_clone_job.rs`, `tests/profiles.rs`, `tests/shared_dirs.rs` and `tests/project_lifecycle.rs` each still carry local copies of the same helpers: `signed_in`, `create`/`created`, `new_project`, `id_of`, `unauthorized`, `password_change_required`, the bare-upstream builders (`bare_upstream_at`, `upstream_with`, `TestUpstream`) and the per-table row counters. They were written in parallel waves, which is why they were not shared at the time.

## Documents
- `CLAUDE.md` "Testing expectations" (`TestApp::spawn()`, helpers under `tests/common/`, git never mocked).

## Acceptance criteria
- [ ] One implementation of the project HTTP helpers in `tests/common/` (a `projects.rs` module or on `TestApp`), used by the five files above; the local copies are deleted.
- [ ] The bare-repository builders in `tests/projects.rs` and `tests/projects_clone_job.rs` are replaced by `common::git::BareFixture` (extended if a case needs it, e.g. a fixture at a caller-chosen path for the retry-after-the-remote-appears scenario).
- [ ] The per-table row count helper (deletion tests in `tests/projects.rs`, `tests/project_lifecycle.rs`) exists once.
- [ ] No test is removed or weakened; every binary reports the same test count as before.
- [ ] `cargo fmt && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` pass.

## Documentation
- none: test-only refactor.

## Out of scope
- The Session lifecycle and Git epics' own tests; they adopt `BareFixture` when they are next touched.