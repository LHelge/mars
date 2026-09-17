---
id: w8ezk
title: Define the AgentBackend trait, LaunchContext, TranslateState and backend selection with a mock
status: open
priority: P0
created: "2026-09-16T20:27:16.163281732Z"
updated: "2026-09-17T04:57:36.177787413Z"
tags:
  - orchestrator
  - agent
  - core
depends_on:
  - "32s57"
parent: "8vnwy"
---

## Summary
Deliver `orchestrator/src/agent/` as the seam between the session owner and any CLI: the `AgentBackend` trait with its three responsibilities (launch command, translate one native line, encode one input line), the `LaunchContext` the launcher fills from a profile and session, the `TranslateState` that carries per-process translation memory, the `Command` type the engine receives, `backend_for(Backend) -> Arc<dyn AgentBackend>` selection, and a `MockAgentBackend` behind the `integration-tests` feature with `as_any()`. The Claude implementation itself is the following tasks; this task compiles with a stub `ClaudeBackend` that returns `raw` for every line.

## Documents
- `ARCHITECTURE.md` "Agent process model" (trait signature), "Orchestrator internals" (`agent/` module, mock-per-trait rule, `Arc<dyn Trait>` in `AppState`), "Claude Code invocation" (the inputs the context must carry), "Input encoding".
- `docs/data-model.md` `agent_profiles` (`kind`, `backend`, `model`, `system_prompt`, `partial_messages`), `sessions` (`cli_session_id`), `secrets` resolution scopes.
- `SPEC.md` "AgentEvent" (translation rules the state must support: `resumed`, echo suppression, subagent bookkeeping).
- ADRs 0003, 0008.

## Acceptance criteria
- [ ] `pub trait AgentBackend: Send + Sync { fn launch_command(&self, ctx: &LaunchContext) -> Command; fn translate(&self, line: &str, state: &mut TranslateState) -> Vec<AgentEvent>; fn encode_input(&self, input: &SessionInput) -> Result<String>; fn as_any(&self) -> &dyn Any; }` exists in `agent/mod.rs` and is object-safe.
- [ ] `Command` is `pub struct Command { pub argv: Vec<String> }`: the container `Cmd` the image entrypoint execs, first element the CLI binary name.
- [ ] `LaunchContext` carries `mode: LaunchMode` (`Conversational { resume: Option<String> }` or `Ephemeral { prompt: String }`), `model: Option<String>`, `system_prompt: Option<String>`, `partial_messages: bool`, `mcp_config_path: String` (always `/session/mcp.json` in v1); invalid combinations (ephemeral with resume, conversational with an inline prompt) are unrepresentable.
- [ ] `TranslateState::new(TranslateConfig { resumed: bool, partial_messages: bool, credential: Option<InjectedCredential> }) -> TranslateState` where `InjectedCredential { name: CredentialName (ANTHROPIC_API_KEY | CLAUDE_CODE_OAUTH_TOKEN), scope: SecretScope (global|project|user) }`; the state also holds `sent_input_hashes: HashSet<[u8; 32]>` with `record_sent_input(&mut self, text: &str)` (SHA-256 of the text), `open_subagents: HashMap<String /*tool_use_id*/, ()>`, and `denied_tool_use_ids: HashSet<String>`; all fields are `pub(crate)` so the Claude translator can use them.
- [ ] `backend_for(backend: Backend) -> Arc<dyn AgentBackend>` returns `ClaudeBackend` for `Backend::Claude`; the match is exhaustive so a new enum value fails to compile until handled.
- [ ] `MockAgentBackend` (feature `integration-tests`) records every `LaunchContext` it was asked for, returns a fixed `Command { argv: ["mock-cli"] }`, translates each line by parsing it as a JSON `AgentEvent` (so tests can script exact events) or `raw` when it is not one, and encodes inputs as `serde_json::to_string(input) + "\n"`; `as_any()` allows downcasting from `Arc<dyn AgentBackend>`.
- [ ] `AppState` gains nothing new in this task; the owner obtains its backend through `backend_for(profile.backend)`.
- [ ] `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` pass.

## Implementation notes
- Files: `orchestrator/src/agent/mod.rs` (trait, `Command`, `LaunchContext`, `LaunchMode`, `backend_for`), `orchestrator/src/agent/state.rs` (`TranslateState`, `TranslateConfig`, `InjectedCredential`, `CredentialName`), `orchestrator/src/agent/mock.rs` (`#[cfg(feature = "integration-tests")]`), `orchestrator/src/agent/claude/mod.rs` (stub `ClaudeBackend` with `pub const CLAUDE_CLI_VERSION: &str` placeholder to be pinned by the probe task; `translate` returns one `raw` event per line until the translator tasks replace it).
- Every module starts with `use crate::prelude::*`.
- `CredentialName` implements `Display` as the exact variable name; it is used in generated messages and never accompanied by a value (rule 3).
- `SecretScope` is the enum the secrets epic defines in `models`; if it does not exist yet when this task starts, define it in `models/secret.rs` with the three documented values and note it for the Secrets epic.
- `record_sent_input` hashes the raw text of the message the owner wrote (for `answer`, the `text`), not the encoded JSON line, so the echo match does not depend on encoding details.
- Do not put per-line parsing helpers here; the Claude translator owns the native shapes.

## Edge cases
- `TranslateState` is per process launch: the owner creates a fresh one on every actual launch, resume and retry; adoption of an existing process reconstructs its state before live tailing as specified by `qtx4x` and `ARCHITECTURE.md` "Durability and recovery"; `resumed` is true on `--resume` launches and on adoption of a process that was launched with `--resume`.
- `sent_input_hashes` is a set, not a counter: two identical user messages are both recorded once each in `user_message` events by the owner but only one hash is stored; document this as accepted (the second echo becomes `raw`).
- The mock must not depend on the Claude module; feature-gated code compiles without warnings under both feature settings.

## Testing
- Unit tests: `backend_for(Backend::Claude)` downcasts to `ClaudeBackend`; `LaunchMode` serialises nothing (no serde on it); `record_sent_input` stores the SHA-256 of the text and `contains` works; `MockAgentBackend` round-trips a scripted `AgentEvent` line and turns a non-event line into `raw`; `encode_input` on the mock ends with `\n`.
- `cargo fmt && cargo clippy -- -D warnings && cargo test --features integration-tests` (and `cargo build` without the feature).

## Documentation
- `ARCHITECTURE.md` "Agent process model": append `as_any()` to the trait sketch and one sentence naming `LaunchContext`/`LaunchMode` and `TranslateState` as the types the owner constructs per launch, in the same commit.

## Assumes from other epics
- "Database schema, models, repositories and test harness": `models::profile::{ProfileKind, Backend}` and `models::secret::SecretScope` (or this task defines `SecretScope` as noted).
- "Repository scaffolding, tooling and CI": the `integration-tests` cargo feature and the `agent/` module stub.