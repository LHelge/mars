# 0037. `cargo nextest` runs the backend suite; the live engine suites keep `cargo test`

Status: accepted.

## Context

The backend suite is a hundred-odd integration test binaries carrying 2228 tests. Giving every binary one Postgres container instead of one per test brought a full run from 264 to 196 seconds (Bears `7fsrg`); measured again for this decision, on the same machine and with the same command, it was 540 seconds, of which about 190 was still one container start per binary and 518 was inside the binaries. (The machine is a 4-vCPU VM whose host it shares, so a figure from one day is not a figure from another; every number here is from one sitting.)

What was left was structural rather than wasteful: `cargo test` builds all the binaries in parallel and then runs them strictly one after another, so a run is the *sum* of the binaries. `tests/engine.rs` alone was 26 seconds of it — live containers whose scenarios must stay serial, because rootless Podman cannot resolve `keep-id` for two containers at once (Bears `u6zkz`). No amount of work inside a binary reaches a minute while the binaries are added up.

Two runners were on the table. `cargo test` with `tests/engine.rs` moved behind its own `--test engine` invocation is a one-line change that removes that binary and nothing else. `cargo nextest` runs every test in a process of its own and schedules those processes across all binaries at once, which turns the sum into a maximum.

Nextest's process-per-test model is also what makes it awkward here. The harness starts one Postgres *per process*, so a naive move would have gone back to one container per test — hundreds of them.

## Decision

`cargo nextest run --features integration-tests` is the default backend test command, locally, in the merge chain and in Orchestrator CI. `orchestrator/.config/nextest.toml` carries the runner's configuration.

`tests/common/db.rs` shares one Postgres server between the processes of a run instead of per process. The state lives in a lock file under `CARGO_TARGET_TMPDIR`: the first process to need a server takes the lock, starts the container, migrates the template database and writes the URL, the container id and a fingerprint of the embedded migrations; every later process reads it and clones its test databases from the same template. Each process registers its pid, and the last one out removes the container — after a short wait and a second look, because being the last process of the moment is not being the last process of the run and a server torn down in the gap between two tests costs the next one a container start. A recorded server whose migrations fingerprint differs from the running binary's, or whose URL no longer answers, is removed and replaced, so a stale container can never serve an old schema.

`tests/engine.rs` and `tests/session_e2e.rs` are excluded from the default nextest run (`default-filter`) and keep running under `cargo test --features integration-tests --test engine --test session_e2e`. They serialise their container work with in-process mutexes, which is exactly the thing process-per-test takes away; one process per binary preserves it without a nextest test group that would have to reproduce the same discipline across processes. That invocation is part of the merge chain and is what the Engine workflow already ran.

Four things the suite spent its time on came out with it, because process-per-test made each of them visible per test rather than per binary:

- **SCRAM.** The test server runs with `POSTGRES_HOST_AUTH_METHOD=trust`. `scram-sha-256` is 4096 rounds of PBKDF2 at each end, and an unoptimised test binary spent about 55 ms of its own CPU on every connection it opened — three per `TestApp::spawn`. The server is a container on a loopback port that the run removes.
- **Argon2.** `models::user::hash_password` derives with `Params::MIN_M_COST`, one pass and one lane in a build carrying the `integration-tests` feature. Verification takes its parameters from the stored hash and is untouched, so a production hash still costs what it should.
- **The starting gates of the race suites.** They slept a flat half second for the racing requests to reach the lock. They now wait for `pg_stat_activity` to show the requests blocked on one, which is both faster and an assertion: a race whose requests never block now fails instead of passing degenerately.
- **Unoptimised dependencies.** `[profile.dev.package."*"] opt-level = 1`. Most of the suite's CPU is inside sqlx, serde and tokio rather than inside this crate, which stays unoptimised so that an edit recompiles at debug speed.

The advisory lock that serialised `CREATE DATABASE` went with them, for a retry: under nextest it spanned the run rather than a process, and the suite measures 9 % faster without it (78 s against 85 s).

## Consequences

The default suite is a minute and a quarter rather than nine. Measured on the development machine with the container engine warm: `cargo test --features integration-tests` took 540 s; `cargo nextest run --features integration-tests` takes 78 s for the same 2228 tests, the doctests 3 s and `cargo test --test engine --test session_e2e` 36 s. The nextest run is CPU-bound — about 260 core-seconds of busy CPU, 83 % of the four cores — so 65 s is this machine's floor for it whatever the scheduling, and the rest is a question of cores or of optimising this crate as well as its dependencies.

Dependencies are built once per build directory and that build is about a third longer (7.5 to 10 minutes cold, measured). CI caches them between runs.

`cargo-nextest` becomes a prerequisite: a line in `README.md`, "Development", and `taiki-e/install-action@nextest` in Orchestrator CI.

Nextest does not run doctests, so `cargo test --features integration-tests --doc` joins the chain beside it.

A future suite that serialises itself with a `static` mutex is silently unserialised under nextest. Such a suite either joins the `default-filter` exclusion and runs under `cargo test`, or is given a nextest test group; the exclusion list is the place to look when a suite starts flaking.
