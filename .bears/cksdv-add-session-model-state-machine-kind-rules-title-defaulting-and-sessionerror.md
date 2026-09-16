---
id: cksdv
title: "Add Session model: state machine, kind rules, title defaulting and SessionError"
status: open
priority: P0
created: "2026-09-16T20:28:22.693124755Z"
updated: "2026-09-16T20:51:51.994353910Z"
tags:
  - orchestrator
  - sessions
  - core
depends_on:
  - naqhy
  - pkaee
  - "8vnwy"
parent: s52qg
---

## Summary
Deliver the `Session` domain model in `orchestrator/src/models/session.rs`: the `SessionState` enum with its legal transitions, the conversational/ephemeral input rules, the title-defaulting function, the `StopSignal` type and a per-model `SessionError` mapped into the crate `Error`. Every later task in this epic (repository, owner, launcher, routes) branches on these types, so they must exist first and carry the exact rules from the documents.

## Documents
- `ARCHITECTURE.md` "Session lifecycle" (state diagram and state table: which states hold a container, which accept input, retry rule).
- `ARCHITECTURE.md` "Stop semantics" (SIGINT then SIGTERM; signal recorded on `state_change`).
- `SPEC.md` "Sessions" (`Session` DTO field list; title defaulting: task title, else first line of `message` truncated to 80 characters, else null; ephemeral sessions accept no further input).
- `SPEC.md` "AgentEvent" (`state_change` carries `from`, `to`, `reason`, `signal?: "SIGINT" | "SIGTERM"`).
- `docs/data-model.md` "Enums" (`session_state`, `profile_kind`) and `sessions` table.
- ADR 0003 (ephemeral sessions are never parked, resumed or retried).

## Acceptance criteria
- [ ] `SessionState` has exactly `Creating`, `Running`, `Parked`, `Done`, `Failed`; derives `sqlx::Type` for the Postgres enum `session_state` and serde `snake_case`.
- [ ] `SessionState::can_transition(self, to) -> bool` allows exactly: creating→running, creating→failed, running→parked, running→failed, running→done, parked→running, parked→done, parked→failed, failed→parked; everything else (including any transition out of `done`) is false.
- [ ] `SessionKind` mirrors `profile_kind` (`Conversational`, `Ephemeral`); `Session::accepts_input(&self) -> Result<(), SessionError>` returns `Ok` only for a conversational session in `creating`, `running` or `parked`; returns `SessionError::EphemeralInput` for any ephemeral session and `SessionError::NotAcceptingInput(state)` for `done`/`failed`.
- [ ] `Session::can_retry(&self)` is true only for a conversational session in `failed`; an ephemeral one yields `SessionError::EphemeralNotRetried`.
- [ ] `default_title(task_title: Option<&str>, message: Option<&str>) -> Option<String>` returns the task title when given, else the first line of `message` (split at the first `\n`, trimmed) truncated to at most 80 characters on a char boundary, else `None`; an empty first line yields `None`.
- [ ] `validate_title(&str) -> Result<String, SessionError>` trims and requires 1–200 characters (used by `PUT /sessions/{id}`).
- [ ] `StopSignal { Sigint, Sigterm }` serialises as `"SIGINT"` / `"SIGTERM"`.
- [ ] `SessionError` variants exist for: `InvalidTransition { from, to }` (409), `EphemeralInput` (409, message `ephemeral sessions accept no input`), `NotAcceptingInput(SessionState)` (409, `session is <state>`), `EphemeralNotRetried` (409), `NotRetryable(SessionState)` (409), `InvalidTitle` (400), `EphemeralRequiresPrompt` (400, `an ephemeral session needs a task_id or a message`), `NotDeletable(SessionState)` (409, `session must be done or failed`). `impl From<SessionError> for Error` maps each to `Conflict`/`BadRequest` with these messages.
- [ ] `Session` struct has every DTO field from `SPEC.md` "Sessions" with the API names: `id, project_id, profile_id, kind, created_by, title, task_id, handoff_id, state, base_ref, branch, container_id, cli_session_id, last_seq, last_activity_at, cost_usd, input_tokens, output_tokens, error, created_at, parked_at, ended_at`; `mcp_token_hash` is never on the DTO (keep it on a separate `SessionRow`/private field that `Serialize` skips).

## Implementation notes
- Files: `orchestrator/src/models/session.rs` (new), `orchestrator/src/models/mod.rs` (export), `orchestrator/src/prelude/error.rs` (add `#[from] SessionError`).
- Follow the model conventions from `ARCHITECTURE.md` "Orchestrator internals": validation and a per-model error enum, no SQL. Use `crate::prelude::*`.
- `Session::branch` is always `session/<id>`; provide `Session::branch_name(id: Uuid) -> String`.
- Keep the transition table as a single `match` so it is readable next to the state diagram; add a doc comment quoting the diagram.
- Truncation to 80 characters must use `char_indices` (not byte slicing) so multi-byte titles never panic.

## Edge cases
- `default_title` with a message that starts with blank lines: use the first non-empty line? No: the spec says "first line"; take the first line, trim it, and return `None` if it is empty (a later task may set a title with `PUT`).
- `failed → parked` is legal only for conversational sessions; the model check is `can_retry`, the transition table itself is kind-agnostic.
- `parked → failed` exists for "relaunch fails".

## Testing
- Unit tests in the module: every allowed transition true, a representative set of forbidden ones false (especially anything out of `done`, `creating → parked`, `failed → running`); `accepts_input` for each state × kind; `default_title` for task title, multi-line message, message longer than 80 chars with a multi-byte character at the boundary, empty message; `validate_title` boundaries (0, 1, 200, 201 characters).
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `SPEC.md` "Sessions": add one sentence stating that `title` (on `POST` and `PUT`) is 1–200 characters after trimming, since the documents currently leave the limit unspecified.

## Assumes from other epics
- "Database schema, models, repositories and test harness" delivers the `sessions` migration, the `Error` enum skeleton and the `prelude`.
- "Projects, agent profiles and shared directories" delivers `Profile` with `kind: profile_kind`; `SessionKind` must convert from it (`From<ProfileKind>`).