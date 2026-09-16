---
id: tjhbr
title: Add HandoffInput parsing and caller-context validation for revision and forward hand-offs
status: open
priority: P1
created: "2026-09-16T20:40:07.557220327Z"
updated: "2026-09-16T20:40:07.557220327Z"
tags:
  - orchestrator
  - tracker
parent: xjaah
---

## Summary
Define the `HandoffInput` request shape from `SPEC.md` "Code hand-offs and review" as a serde-tagged enum with the validation that does not need the database: non-empty comment, full commit id, the REST-vs-MCP rule for `source_session_id`, and the "must come with a different target `state`" rule expressed as a typed check the route and the MCP `update` tool both call. This is the shared input contract every later task in the epic consumes, and it keeps the MCP epic from re-deriving the rules.

## Documents
- `SPEC.md` "Code hand-offs and review" (the `HandoffInput` TypeScript union; `handoff` requires a different target `state` in the same update, 400 otherwise, and a non-empty `comment`; REST revision requires `source_session_id` and it must belong to the task's project; for MCP it is omitted and derived from the caller, a supplied id is rejected; `commit` is a full object id, not a moving ref).
- `SPEC.md` "Tasks" (`PUT /projects/{pid}/tasks/{id}` body `{..., state?, handoff?: HandoffInput}`), "MCP tool contracts" -> `update` (`handoff?: HandoffInput`; hand-off inputs obey "Code hand-offs and review"; revision publication uses the calling session).
- `docs/data-model.md` `task_handoffs` (`commit` is a full git object id; `review_status` values).
- ADR 0018.

## Acceptance criteria
- [ ] `models::task_handoff::HandoffInput` deserialises `{ "kind": "revision", "source_session_id"?: uuid, "commit": string, "comment": string }` and `{ "kind": "forward", "handoff_id": uuid, "comment": string, "review"?: "approved" | "changes_requested" }` via `#[serde(tag = "kind", rename_all = "snake_case")]`; an unknown `kind` or a missing required field is a deserialisation error the route maps to 400.
- [ ] `ReviewDecision { Approved, ChangesRequested }` serialises as `approved` / `changes_requested` and converts into the existing `ReviewStatus` from the tracker models.
- [ ] `HandoffCaller` enum: `User { user_id: Uuid }` (REST) and `Session { session_id: Uuid }` (MCP). `HandoffInput::validate(&self, caller: &HandoffCaller) -> Result<ValidatedHandoff, TaskError>` returns:
  - `ValidatedHandoff::Revision { source_session_id: Uuid, commit: String, comment: String }` where for `HandoffCaller::User` a missing `source_session_id` fails with `TaskError::HandoffSourceRequired` ("revision hand-off requires source_session_id"), and for `HandoffCaller::Session` a supplied `source_session_id` fails with `TaskError::HandoffSourceNotAllowed` ("source_session_id is derived from the calling session") and the resolved source is the caller's session id;
  - `ValidatedHandoff::Forward { handoff_id: Uuid, comment: String, review: Option<ReviewDecision> }`.
- [ ] `comment` is trimmed and must be non-empty (`TaskError::EmptyComment`, reusing the tracker model's variant); `commit` must match `^[0-9a-f]{40}$` (`TaskError::InvalidCommit`, reused; abbreviated, uppercase or ref-like values such as `main` or `HEAD` are rejected).
- [ ] `HandoffInput::require_state_change(target_state: Option<&str>, current_state_name: &str) -> Result<(), TaskError>` fails with `TaskError::HandoffRequiresStateChange` ("handoff requires a different target state") when `target_state` is `None` or equals the current state's name; the route and the MCP tool call this before any git work.
- [ ] The new `TaskError` variants map to HTTP 400 through the existing `#[from] TaskError` variant of `prelude::Error`; every rule has a unit test.

## Implementation notes
- File: `orchestrator/src/models/task_handoff.rs` (extend the tracker model file; keep row types and input types together), re-exported from `models/mod.rs`. Add the new variants to the tracker epic's `TaskError` enum in the same file or in `models/task.rs`, whichever holds it.
- Reuse `is_state_name`-style helpers only where they apply; the commit regex lives in one `fn is_full_object_id(&str) -> bool` shared with `NewTaskHandoff::validate`.
- Deserialisation is lenient about unknown fields (matching the rest of the API); do not use `deny_unknown_fields`.
- The MCP `update` tool receives the same JSON shape inside its input object; the tool handler (MCP epic) constructs `HandoffCaller::Session` from its `SessionContext`. Document this on the type.

## Edge cases
- `comment` consisting only of whitespace is empty.
- `review` given on a `revision` input: rejected at deserialisation (the variant has no such field only if `deny_unknown_fields` were set; instead validate explicitly and return `TaskError::HandoffReviewOnRevision` "review applies to forward hand-offs only") — keep the message exact.
- `source_session_id` equal to the calling session under `HandoffCaller::Session` is still rejected: MCP callers never pass it.
- A `state` that equals the current state but with different case or whitespace is a different (unknown) state name and fails later as an unknown state; `require_state_change` compares exact strings.

## Testing
- Unit tests in `models/task_handoff.rs`: both variants round-trip through serde; each rejection above with its exact message; `validate` for user-without-source, session-with-source, session-without-source (source becomes the caller); `require_state_change` for `None`, equal and different names.
- `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Documentation
- `SPEC.md` "Code hand-offs and review": add one sentence listing the 400 messages above (the document names the rules but not the messages).

## Assumes from other epics
- "Database schema, models, repositories and test harness": `TaskError`, `ReviewStatus`, `NewTaskHandoff::validate` and `TaskError::{EmptyComment, InvalidCommit}` from the tracker models task.