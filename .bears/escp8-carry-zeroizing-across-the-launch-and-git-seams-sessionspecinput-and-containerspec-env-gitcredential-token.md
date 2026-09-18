---
id: escp8
title: "Carry Zeroizing across the launch and git seams: SessionSpecInput and ContainerSpec env, GitCredential token"
status: done
priority: P1
created: "2026-09-17T20:03:49.284808788Z"
updated: "2026-09-18T23:46:56.485720451Z"
tags:
  - orchestrator
  - secrets
  - engine
  - git
  - architecture
  - docs
depends_on:
  - "48ke5"
  - trxaf
parent: zeccj
attempts: 1
---

## Summary
The one seam that will carry a secret value into a container drops the zeroize guarantee: `ResolvedSecrets.env` is `Vec<(String, Zeroizing<String>)>` while `SessionSpecInput.secrets` and `ContainerSpec.env` hold plain `String`s, and `GitCredential.token` is a plain `String` with a hand-written `Drop`. Neither seam has a production caller yet; fix the types before the session launcher (`9wxhs`) and the git epic build on them.

## Documents
- `ARCHITECTURE.md` "Secrets" → "Resolution at launch", "Session container specification" (env order), "Git model" (credential use), "Orchestrator internals" (crates: `zeroize`).
- ADRs 0002, 0006.

## Acceptance criteria
- [ ] `SessionSpecInput.secrets: Vec<(String, Zeroizing<String>)>`; `ContainerSpec.env` values are `Zeroizing<String>` (or `ContainerSpec` holds `env: Vec<(String, String)>` for fixed values plus `secret_env: Vec<(String, Zeroizing<String>)>` appended last, matching the documented order); `to_bollard` is the only place the bytes are copied into bollard's `Vec<String>`, and that copy is documented as the point where zeroization ends.
- [ ] `ContainerSpec` still has no `Debug` derive; `MockEngine::specs`/`spec_of` inspectors return the spec with secret values readable for tests only behind `integration-tests`.
- [ ] `GitCredential.token: Zeroizing<String>`; the manual `Drop` impl is removed; `project_git_credential` returns it without conversion; the git module's header encoding takes `&str` from it.
- [ ] `resolve.rs` documents that the `ANTHROPIC_API_KEY`/`CLAUDE_CODE_OAUTH_TOKEN` exclusivity is applied by the launcher (as `9wxhs` specifies) or moves that rule into the resolver; pick one and say it in `ARCHITECTURE.md` "Claude Code invocation".

## Implementation notes
- Files: `orchestrator/src/engine/{spec,types,mock,bollard}.rs`, `orchestrator/src/secrets/resolve.rs`, `orchestrator/src/git/mod.rs`, `orchestrator/src/secrets/git_credential.rs`, `ARCHITECTURE.md`.
- Bollard needs owned `String`s in `Config.env`; the copy is unavoidable and the container process holds the values anyway, so the goal is that no orchestrator-side buffer outlives the launch call.

## Edge cases
- `build_session_spec`'s reserved-name refusal (`RESERVED_ENV_NAMES`) must still compare names without materialising values.
- Logging: the spec is never `Debug`-logged today; keep the `types.rs` note and make sure the new fields do not derive `Debug` either.

## Testing
- `spec.rs` unit tests updated for the new field types; a test asserts the secret values appear in `to_bollard` output in the documented position and nowhere in a `format!("{:?}")` of the input (compile-time by the missing derive).
- `cd orchestrator && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo clippy --all-targets --features integration-tests -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `ARCHITECTURE.md` "Secrets" → "Resolution at launch": one sentence that resolved values stay zeroizing through spec construction and are copied only into the engine call.
- `ARCHITECTURE.md` "Claude Code invocation": where the both-credentials refusal is applied, if this task moves it.

## Assumes from other epics
- "Session lifecycle": `9wxhs` depends on this task; "Git operations": the credential provider implementation takes `Zeroizing<String>` as given.