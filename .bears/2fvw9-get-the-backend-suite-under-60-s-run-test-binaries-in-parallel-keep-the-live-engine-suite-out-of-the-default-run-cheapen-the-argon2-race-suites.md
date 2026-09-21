---
id: "2fvw9"
title: "Get the backend suite under 60 s: run test binaries in parallel, keep the live engine suite out of the default run, cheapen the Argon2 race suites"
status: done
priority: P2
created: "2026-09-17T21:43:25.516811148Z"
updated: "2026-09-21T02:53:30.626114249Z"
tags:
  - orchestrator
  - tests
  - infra
depends_on:
  - "7fsrg"
attempts: 1
---

## Summary
After `7fsrg` the full backend suite takes 196 s on the development machine (down from 264 s) and the per-binary container cost is gone: an ordinary database binary takes about 2 s. What is left is four binaries that account for 110 s and that container sharing cannot touch, plus cargo running the 38 binaries one after another. The `7fsrg` acceptance criterion of 60 s was derived on the wrong assumption that container starts were all of the 264 s; this task is what actually reaches it.

Measured by the `7fsrg` agent (two runs, 197 s and 196 s):

| binary | time | why |
| --- | --- | --- |
| `tests/engine.rs` | 55 s | live engine scenarios, serial by design (`u6zkz`) |
| `tests/users_last_admin_race.rs` | 26 s | `RACE_ITERATIONS = 20` with real Argon2 hashing and `IN_FLIGHT` sleeps |
| `tests/auth_revocation_race.rs` | 18 s | same |
| `tests/rotate_secrets_cli.rs` | 12 s | spawns the orchestrator binary per scenario |
| the other 34 binaries | 76 s | about 45 s of it is the one container start each still pays |

## Documents
- `CLAUDE.md` "Code quality" (the backend command) and "Testing expectations" (engine tests run only when `DOCKER_HOST` is set).
- `.claude/skills/implement-epic/scripts/merge-task.sh` (the chain the coordinator runs) and `SKILL.md` "Merging a branch".
- `README.md` "Development" and "CI".
- `.github/workflows/orchestrator.yml` and `engine.yml`.

## Acceptance criteria
- [ ] Decide and record one of: (a) `cargo nextest` as the local and CI test runner (`cargo nextest run --features integration-tests`), with a `.config/nextest.toml` that puts `tests/engine.rs` in a serial test group and everything else in parallel across binaries; or (b) keep `cargo test` and move `tests/engine.rs` behind an explicit `--test engine` invocation that the merge script and the Engine workflow run, so the default suite excludes it. State the choice in `CLAUDE.md` "Code quality" and update the command there, in `merge-task.sh`, in `README.md` and in the workflows in the same commit.
- [ ] The Argon2 race suites keep their iteration counts and their assertions but hash with a cheap parameter set under `cfg(test)` or through the existing password-hashing seam, or the iteration count comes from an environment variable with the current default only in CI; either way the race they prove still fails if the lock is removed (verify by temporarily removing it).
- [ ] `tests/rotate_secrets_cli.rs` reuses one built binary per scenario where it spawns several, if that is where its 12 s go; otherwise leave it and say so.
- [ ] Wall time of the default backend test command with `DOCKER_HOST` set is under 60 s on the development machine; state the number in the commit message.
- [ ] Every test that ran before still runs somewhere in the merge chain and in CI; `tests/engine.rs` is not skipped by default on the developer machine, only moved to its own invocation.

## Implementation notes
- Files: `orchestrator/tests/users_last_admin_race.rs`, `orchestrator/tests/auth_revocation_race.rs`, `orchestrator/tests/rotate_secrets_cli.rs`, `orchestrator/tests/common/races.rs`, `orchestrator/.config/nextest.toml` (if a), `orchestrator/Cargo.toml` (nothing for nextest; it is a cargo subcommand installed on the machine and in CI with `taiki-e/install-action@nextest`), `.claude/skills/implement-epic/scripts/merge-task.sh`, `CLAUDE.md`, `README.md`, `.github/workflows/orchestrator.yml`, `.github/workflows/engine.yml`.
- With nextest each test is its own process; the shared-server harness from `7fsrg` starts one container per *process*, which would then be one per test. Before choosing (a), extend `tests/common/db.rs` so that a server started by one process can be reused by siblings (a lock file under `CARGO_TARGET_DIR` holding the container id and URL, last process out removes it) or use nextest's `test-threads` and `slow-timeout` with a per-binary process model (`--no-capture` is not it; check the nextest "process-per-test" model against the harness first). If that turns out to be more than a day, choose (b).
- Argon2 parameters: `models/user.rs` owns hashing; look for the existing `verify_blocking` / hash parameters and whether a test-only cheap configuration can be selected without touching the production path.

## Edge cases
- `u6zkz` (Podman 6.1.2 `keep-id` under concurrent starts) is why `tests/engine.rs` runs its scenarios serially; running it in parallel with the rest of the suite is fine because the other binaries' containers do not use `keep-id`, but keep the engine scenarios serial among themselves.
- CI's Orchestrator workflow leaves `DOCKER_HOST` unset so engine tests are skipped there; the Engine workflow runs them. Option (b) makes that split explicit instead of relying on the skip.

## Testing
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings` plus the new default test command and the separate engine invocation, all green, timed.

## Documentation
- `CLAUDE.md` "Code quality" and "Testing expectations", `README.md` "Development" and "CI", `SKILL.md` "Merging a branch": the commands change, so they change in the same commit.