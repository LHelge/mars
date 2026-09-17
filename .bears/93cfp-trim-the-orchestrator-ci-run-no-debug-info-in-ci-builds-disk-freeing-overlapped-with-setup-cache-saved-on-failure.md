---
id: "93cfp"
title: "Trim the Orchestrator CI run: no debug info in CI builds, disk freeing overlapped with setup, cache saved on failure"
status: done
priority: P2
created: "2026-09-17T20:48:37.604840879Z"
updated: "2026-09-17T21:02:57.606820087Z"
tags:
  - infra
  - ci
  - docs
parent: yq6c3
assignee: claude-opus-subagent
attempts: 1
---

## Summary
The last successful Orchestrator CI run (35269143518, 2026-09-17) took 24 minutes: 312 s freeing runner disk, 48 s cache restore, 89 s clippy, 206 s compiling the crate and its 34 test binaries, 711 s running the tests, 62 s cache save. The dependency cache hits on every run, so the levers are elsewhere. This task takes the three that live in `orchestrator.yml`: build without debug info in CI so the workspace build, the link and the cache entry shrink; overlap the disk-freeing step with toolchain and cache setup instead of running it first; and let `rust-cache` save after a failed job so a `Cargo.lock` change is not rebuilt by every failing run until one passes. The 711 s of tests is the harness task `7fsrg`, not this one.

## Documents
- `README.md` "CI" (what the Orchestrator CI workflow checks; unchanged in substance).
- `CLAUDE.md` "Code quality" (CI runs the same two clippy invocations; unchanged).

## Acceptance criteria
- [ ] The `check` job sets `CARGO_PROFILE_DEV_DEBUG: 0` (or `line-tables-only` if a backtrace with line numbers is wanted for failing tests; say which and why in the workflow comment) in its `env`. `RUST_BACKTRACE=1` stays. The `sqlx-offline-data` job and the Engine workflow get the same setting for the same reason, each with its own cache key so a restore never mixes debug levels.
- [ ] The `rust-cache` cache entry for the `check` job is measured before and after; the size and the restore and save times go into the commit message.
- [ ] The "Free runner disk space" step either goes, if `df` after the tests shows headroom without it, or runs in the background from the first step (`... &` with the pid recorded) and is `wait`ed on immediately before the Tests step, so its 312 s overlap the toolchain install, cache restore, fmt and clippy. The `df -h /` before and after stays in the log.
- [ ] `Swatinem/rust-cache@v2` gets `cache-on-failure: true` on every use (both Orchestrator jobs and the Engine matrix).
- [ ] The workflow files validate with `npx -y @action-validator/cli@latest` and the run on the commit is green.
- [ ] The Tests step is split into `cargo test --features integration-tests --no-run` and the run, so the step timings show compile and run separately in future runs.

## Implementation notes
- Files: `.github/workflows/orchestrator.yml`, `.github/workflows/engine.yml`, `README.md` only if the CI table's wording changes.
- `rust-cache` keys include the job id and an environment hash; changing `env` changes the key, which is intended here (a full rebuild once, then hits).
- Do not touch `cancel-in-progress`; with one push per epic (implement-epic skill) cancellations are rare and the setting still protects against stacked runs.
- Point 5 of the survey, stale cache entries for old `Cargo.lock` hashes, is left to the seven-day expiry; do not add a cleanup workflow.

## Edge cases
- A test that panics prints a backtrace; with `debug = 0` frames still resolve to function names but not lines. If that turns out to hurt diagnosis in CI, `line-tables-only` is the compromise; record the choice.
- The background disk-freeing step must not race `docker image prune` against the testcontainers pull; the `wait` before the Tests step guarantees the prune is done first.

## Testing
- Push on a branch and read the step durations with `gh run view <id> --json jobs`; compare with the numbers above.
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` is unaffected locally.

## Documentation
- `README.md` "CI": no change unless the disk step is removed and the table mentions it (it does not today). Workflow comments carry the reasoning.