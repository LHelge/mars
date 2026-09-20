---
id: "7rf9z"
title: "Define the MCP tool input/output types, TaskArg parsing (UUID, number, \"12\", \"#12\"), tool names and the verbatim tool descriptions with a byte-for-byte SPEC.md test"
status: done
priority: P0
created: "2026-09-16T20:43:24.180888141Z"
updated: "2026-09-20T06:39:34.662267456Z"
tags:
  - orchestrator
  - mcp
  - tracker
  - tests
depends_on:
  - "8a6qd"
parent: qgj33
attempts: 1
---

## Summary
Put every part of the tool contract that is pure data in one place: the twelve tool names in listing order with their tracker/git classification, the verbatim descriptions from `SPEC.md`, the serde + `schemars` input and output types for each tool, and the `TaskArg` type that accepts a task's UUID or per-project number as a JSON number or string (`12`, `"12"`, `"#12"`). A test parses `SPEC.md` and asserts each description constant equals the document byte for byte, which is one of the epic's acceptance criteria.

## Documents
- `SPEC.md` "MCP tool contracts": the intro paragraph ("Tool descriptions are part of the contract... reproduced verbatim"; "Every `task` argument accepts a task's UUID or its per-project number (as a number or a string such as `\"12\"` or `\"#12\"`)"; state in inputs and outputs is the state's name), and every `### \`<tool>\`` subsection's `Description:` line and `Input`/`Output` shapes: `ready`, `claim`, `get_task`, `update`, `release`, `comment`, `needs_human`, `create_task`, `list_session_branches`, `merge`, `rebase`, `push`.
- `SPEC.md` "Tasks" (`Task`, `TaskDetail`, `Comment` shapes), "Code hand-offs and review" (`HandoffInput`; for MCP `source_session_id` is omitted and derived from the caller; a supplied one is rejected), "Git" (`SessionBranch`, `MergeInput = { target, message? } & ({ source } | { task_id, handoff_id })`).
- `docs/data-model.md` `tasks` (`priority` CHECK 0–3, `labels` pattern, `title` 1–200), `task_states` (name pattern).

## Acceptance criteria
- [ ] `orchestrator/src/mcp/tools/names.rs`: `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum ToolName { Ready, Claim, GetTask, Update, Release, Comment, NeedsHuman, CreateTask, ListSessionBranches, Merge, Rebase, Push }` with `pub const ALL: [ToolName; 12]` in exactly that order, `pub fn as_str(self) -> &'static str` (snake_case names as in `SPEC.md`), `pub fn parse(name: &str) -> Option<ToolName>`, `pub fn is_git(self) -> bool` (true for the last four), and `pub fn description(self) -> &'static str`.
- [ ] `orchestrator/src/mcp/tools/descriptions.rs`: one `pub const` per tool holding the description text copied from `SPEC.md` exactly (including backticks and punctuation), e.g. `READY`, `CLAIM`, ... `PUSH`.
- [ ] `orchestrator/tests/mcp_descriptions.rs` reads `concat!(env!("CARGO_MANIFEST_DIR"), "/../SPEC.md")`, locates the "## MCP tool contracts" section, and for each `ToolName` finds the heading line starting with `### \`<name>\`` and the next line starting with `Description: "`, extracts the text between the first and last double quote, and asserts equality with `ToolName::description()`; it also asserts that exactly twelve tool headings exist so a new tool in the document fails the test.
- [ ] `orchestrator/src/mcp/tools/types.rs`: `#[derive(Deserialize, JsonSchema)] #[serde(untagged)] pub enum TaskArg { Number(u64), Text(String) }` with `pub fn parse(&self) -> McpResult<TaskRef>` where `TaskRef` is the tracker epic's reference type (`Id(Uuid)` | `Number(i32)`): a JSON number → `Number`; a string that parses as a UUID → `Id`; a string of digits optionally prefixed with `#` → `Number`; anything else (empty, `#`, `abc`, negative, `12.5`, numbers above `i32::MAX`) → `invalid_argument` with message `task must be a UUID, a number, or "#<number>"`. Whitespace is not trimmed.
- [ ] Input types (all `Deserialize + JsonSchema`, `#[serde(deny_unknown_fields)]` off, unknown fields ignored): `ReadyInput { limit: Option<u32> }`; `TaskOnlyInput { task: TaskArg }` (for `claim`, `get_task`); `UpdateInput { task, state: Option<String>, title: Option<String>, description: Option<String>, priority: Option<i16>, labels: Option<Vec<String>>, parent: Option<Option<TaskArg>> (absent vs null distinguished with `#[serde(default, deserialize_with = "deserialize_double_option")]`), add_depends_on: Option<Vec<TaskArg>>, remove_depends_on: Option<Vec<TaskArg>>, handoff: Option<McpHandoffInput> }`; `ReleaseInput { task, reason: String }`; `CommentInput { task, body: String }`; `NeedsHumanInput { task, reason: String }`; `CreateTaskInput { title: String, description: Option<String>, state: Option<String>, priority: Option<i16>, labels: Option<Vec<String>>, parent: Option<TaskArg>, depends_on: Option<Vec<TaskArg>>, discovered_from: Option<TaskArg> }`; `EmptyInput {}`; `McpMergeInput { target: String, message: Option<String>, source: Option<String>, task_id: Option<TaskArg>, handoff_id: Option<Uuid> }`; `RebaseInput { branch: String, onto: String }`; `PushInput { r#ref: String, remote_branch: Option<String>, force: Option<bool> }`.
- [ ] `#[serde(tag = "kind", rename_all = "snake_case")] pub enum McpHandoffInput { Revision { source_session_id: Option<String>, commit: String, comment: String }, Forward { handoff_id: Uuid, comment: String, review: Option<ReviewDecision> } }` where `ReviewDecision` is the hand-off epic's `approved | changes_requested` enum; `McpHandoffInput::validate(&self) -> McpResult<()>` rejects a present `source_session_id` with `invalid_argument` message `source_session_id is derived from the calling session and must not be supplied`, an empty `comment` with `handoff comment must not be empty`, and a `commit` that is not 40 lowercase hex characters with `commit must be a full 40-character object id`.
- [ ] Output types (`Serialize + JsonSchema`): `TaskSummary { id, number, title, state, priority, labels, description_excerpt, attempts, depends_on_count }`, `ReadyOutput { tasks: Vec<TaskSummary> }`, `TaskOutput { task: Task }`, `TaskDetailOutput { task: TaskDetail }`, `CommentOutput { comment: Comment }`, `BranchesOutput { branches: Vec<SessionBranch> }`, `CommitOutput { commit: String }`, `PushOutput { remote_branch: String, commit: String }`; `Task`, `TaskDetail`, `Comment` and `SessionBranch` are the REST DTO types re-used, not copies (add `JsonSchema` derives to them where missing).
- [ ] Shared validation helpers in `types.rs`: `validate_priority(i16) -> McpResult<i16>` (0–3, message `priority must be between 0 (critical) and 3 (low)`), `validate_limit(Option<u32>) -> McpResult<i64>` (default 20, 1–100, message `limit must be between 1 and 100`), `non_empty(field, &str) -> McpResult<()>` (message `<field> must not be empty`).
- [ ] Unit tests: `TaskArg::parse` table (each accepted and rejected form); `UpdateInput` parent tri-state (absent → `None`, `null` → `Some(None)`, `"#3"` → `Some(Some(..))`); `McpHandoffInput` tag parsing and each validation message; JSON schema generation succeeds for every input type and the `task` property's schema accepts both `string` and `integer`; the description test above.
- [ ] `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` passes.

## Implementation notes
- Files: `orchestrator/src/mcp/tools/mod.rs`, `orchestrator/src/mcp/tools/names.rs`, `orchestrator/src/mcp/tools/descriptions.rs`, `orchestrator/src/mcp/tools/types.rs`, `orchestrator/tests/mcp_descriptions.rs`; `cargo add schemars` (the version `rmcp` re-exports; prefer `rmcp::schemars` if the crate re-exports it to avoid a version mismatch).
- The description constants are the only place the text lives in code; `list_tools` (dispatch task) reads them through `ToolName::description()`.
- `TaskRef` and its resolver belong to the tracker epic; if that epic's type has a different name, adapt the `parse` return type and note the name in the commit message rather than duplicating the type.
- `MergeInput` for MCP differs from REST only in `task_id: TaskArg` (a per-project number is accepted over MCP); keep a separate `McpMergeInput` and convert to the git epic's `MergeInput` in the git tools task.

## Edge cases
- A JSON number with a fractional part or exponent (`12.0`) fails `u64` deserialisation; serde's untagged error must be turned into the same `invalid_argument` message by the dispatcher (record this in a doc comment on `TaskArg`).
- Number `0` is syntactically valid and resolves to `not_found` later (task numbers start at 1).
- `labels` entries are validated by the tracker's model rules at mutation time, not here.

## Testing
- Unit tests in `types.rs`, `names.rs`; the `tests/mcp_descriptions.rs` document test.
- Command: `cd orchestrator && cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests`.

## Documentation
- none: implements the documented contract as written. (If a description constant and `SPEC.md` disagree, the document wins; fix the constant.)

## Assumes from other epics
- "Task tracker: states, tasks, leases, dependencies and events": the `TaskRef` (UUID or number) type and the REST `Task`, `TaskDetail`, `Comment` DTOs.
- "Code hand-offs and review": the `ReviewDecision` enum and `Handoff` DTO.
- "Git operations: mirror, clones, integration and REST API": `SessionBranch` and `MergeInput` DTOs.