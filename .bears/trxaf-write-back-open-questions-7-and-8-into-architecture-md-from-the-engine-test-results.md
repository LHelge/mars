---
id: trxaf
title: Write back open questions 7 and 8 into ARCHITECTURE.md from the engine test results
status: open
priority: P2
created: "2026-09-16T20:32:04.342490296Z"
updated: "2026-09-16T20:32:04.342490296Z"
tags:
  - docs
  - engine
depends_on:
  - "48ke5"
parent: naqhy
---

## Summary
Record what the Engine CI run on both engines showed for open questions 7 (`SIGINT` via `kill` reaching PID 1 without `Init: true`) and 8 (`UsernsMode: keep-id:uid=1000,gid=1000` accepted through Podman's compat API) in the documents that depend on the answers, and delete both entries from `docs/open-questions.md`. If question 7 turned out negative on either engine, this task also flips the container specification to `Init: true` and adjusts the spec builder, its key-set unit test and the signal engine test to the new contract.

## Documents
- `docs/open-questions.md` items 7 and 8 (delete)
- `ARCHITECTURE.md` "Session image" (sentence "If the engine tests show a signal not reaching PID 1 on either engine, the container is created with `Init: true` and the entrypoint stays the same" becomes a statement of fact)
- `ARCHITECTURE.md` "Engine adapter" table (`UsernsMode` row: replace "Verified at startup (see below)" wording with the verified Podman version; add an `Init` row only if it is now used) and "Session container specification" table (add an `Init` row only if used)
- `README.md` "Podman setup" (minimum Podman version if the CI run constrained it)
- ADR 0004 (unchanged unless the decision itself changes; a rejected alternative such as running an init process gets a new ADR only if `Init: true` is adopted and a real alternative was rejected)

## Acceptance criteria
- [ ] `docs/open-questions.md` no longer contains items 7 and 8; the remaining items are renumbered only if the document's numbering is positional (keep the others' text unchanged).
- [ ] `ARCHITECTURE.md` "Session image" states the observed result in one sentence, e.g. `Engine tests on Podman <x.y> and Docker <a.b> show SIGINT and SIGTERM sent through kill reach the CLI as PID 1 without Init; the container is created without Init.` (or the `Init: true` variant), citing `tests/engine.rs`.
- [ ] `ARCHITECTURE.md` "Engine adapter" `UsernsMode` row records `verified on Podman <version> through the compat API (tests/engine.rs, userns_keep_id_accepted)`.
- [ ] If `Init: true` is adopted: `spec.rs` `to_bollard` sets `HostConfig.init = Some(true)`, the key-set unit test lists `Init`, the "Session container specification" table gains the row `| Init | \`true\`: an init process forwards signals to the CLI. |`, the "Engine adapter" table gains an `Init` row marked verified on both engines, and the signal engine test asserts delivery with `Init` set.
- [ ] No behaviour changes other than the `Init` case; `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Read the Engine CI logs for both matrix jobs (engine versions are printed at the start of each job) and quote the versions in the write-back.
- The documents' convention is that the rule lives in the main document, not in an ADR (`CLAUDE.md` rule 1); keep the ADR untouched unless an alternative was rejected.
- Commit message scope `docs` (plus `orchestrator` if `Init` changes the code), citing this task id.

## Edge cases
- Mixed result (SIGINT works on Docker but not rootless Podman, or vice versa): adopt `Init: true` for both engines rather than an engine-conditional field, since the table rule is one `HostConfig` for both; record which engine needed it.
- `keep-id` accepted but ownership wrong (probe fails on Podman): that is a deployment issue, not a spec change; record the Podman version that passed and leave the probe as the gate.

## Testing
- If `Init` changes: the unit key-set test and `kill_sigint_reaches_pid1_without_init` (rename to `kill_sigint_reaches_pid1`) updated; re-run the Engine workflow and link the green run in the PR.
- Otherwise documentation only; run `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` to confirm nothing else moved.

## Documentation
- This task is the documentation write-back: `ARCHITECTURE.md`, `docs/open-questions.md`, possibly `README.md`.

## Assumes from other epics
- "Session container images: claude and stub": the entrypoint `exec`s the CLI as PID 1; the write-back describes the engine behaviour, not the image.
