//! The native Claude Code `stream-json` shapes the translator reads.
//!
//! One struct per line kind the translator has a rule for, and nothing else:
//! the fields Mars uses, every one of them optional or defaulted, plus a
//! flattened `extra` that swallows the rest. The CLI adds fields between
//! versions — the 2.1.274 recording in `images/claude/VERIFY.md` lists a dozen
//! on `init` alone — and a new field must never turn a line into a parse
//! failure, so nothing here is `deny_unknown_fields` and nothing here is
//! required.
//!
//! The structs are `Deserialize` only. Nothing in Mars ever writes a native
//! line; `extra` exists so a shape can be logged or re-serialised whole while
//! the translator reads the fields it knows.

use serde::Deserialize;
use serde_json::{Map, Value};

// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

/// `{"type":"system","subtype":"init",...}` (`SPEC.md`, "AgentEvent", `init`).
///
/// `session_id` is the only field the owner cannot do without: it becomes
/// `sessions.cli_session_id` and is what `--resume` is given
/// (`ARCHITECTURE.md`, "Claude Code invocation"). `model` and `tools` were the
/// open question the 2.1.274 probe answered — both are present — but they stay
/// optional because the event schema allows their absence.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct NativeSystemInit {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<NativeMcpServer>,
    // Read by the unit tests and kept for diagnostics: the fields a later CLI
    // version adds land here instead of being lost, while the translator works
    // from the raw `Value` it already holds.
    #[allow(dead_code)]
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One entry of `init.mcp_servers`.
///
/// The recorded shape also carries `source`, which is display noise Mars does
/// not forward; `status` is kept as the backend's own word (`SPEC.md`,
/// "AgentEvent", `McpServerStatus`).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct NativeMcpServer {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    // Read by the unit tests and kept for diagnostics: the fields a later CLI
    // version adds land here instead of being lost, while the translator works
    // from the raw `Value` it already holds.
    #[allow(dead_code)]
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `{"type":"system","subtype":"permission_denied",...}`.
///
/// Recorded on 2.1.274 with `tool_name`, `tool_use_id`, `decision_reason_type`
/// and `message`. `tool` and `reason` are accepted beside them because the
/// denial entries inside `result.permission_denials` are a different shape and
/// older CLI versions spelled the fields differently; the translator falls back
/// across them in one place.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct NativePermissionDenied {
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    // Read by the unit tests and kept for diagnostics: the fields a later CLI
    // version adds land here instead of being lost, while the translator works
    // from the raw `Value` it already holds.
    #[allow(dead_code)]
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `{"type":"result",...}` (`ARCHITECTURE.md`, "Cost accounting").
///
/// `usage` is passed through untouched: the owner accumulates it, nothing here
/// interprets it. `total_cost_usd` is cumulative for the process on 2.1.274,
/// which is the owner's problem and not the translator's.
///
/// `terminal_reason` is read because it is the only field that tells a turn the
/// user stopped (`aborted_streaming`) from a turn that failed: both arrive with
/// `is_error: true` and `subtype: "error_during_execution"`
/// (`ARCHITECTURE.md`, "Stop semantics").
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct NativeResult {
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub terminal_reason: Option<String>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub num_turns: i64,
    #[serde(default)]
    pub duration_ms: i64,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub usage: Option<Value>,
    #[serde(default)]
    pub permission_denials: Vec<Value>,
    // Read by the unit tests and kept for diagnostics: the fields a later CLI
    // version adds land here instead of being lost, while the translator works
    // from the raw `Value` it already holds.
    #[allow(dead_code)]
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `{"type":"assistant","message":{...},"parent_tool_use_id":...}`
/// (`SPEC.md`, "AgentEvent", the `assistant` rule).
///
/// The top-level `parent_tool_use_id` is not read here: the dispatcher copies
/// it onto every event a branch returns, from the raw line.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct NativeAssistant {
    #[serde(default)]
    pub message: Option<NativeMessage>,
}

/// The `message` object of an `assistant` line.
///
/// `id` is the backend message id that becomes `AgentEvent.message_id`; a CLI
/// version that does not send one simply leaves the field off every event.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct NativeMessage {
    #[serde(default)]
    pub id: Option<String>,
    /// Absent, `null` and `[]` are all "this message says nothing".
    #[serde(default)]
    pub content: Option<NativeContent>,
}

/// `message.content`, which is a block array in every recorded line but is
/// allowed by the Messages API to be a bare string.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum NativeContent {
    Text(String),
    Blocks(Vec<NativeContentBlock>),
}

/// One entry of `message.content`.
///
/// Tagged on `type`, with everything else kept as [`Self::Other`]: a block kind
/// a later CLI adds, and a `tool_use` missing the `id` the frontend needs to
/// pair it with its result, both become a `raw` event rather than a parse
/// failure for the whole line.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NativeContentBlock {
    Text(NativeTextBlock),
    Thinking(NativeThinkingBlock),
    RedactedThinking,
    ToolUse(NativeToolUseBlock),
    Other(Value),
}

/// `{"type":"text","text":...}`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub(crate) struct NativeTextBlock {
    #[serde(default)]
    pub text: String,
}

/// `{"type":"thinking","thinking":...,"signature":...}`.
///
/// The `signature` is the CLI's own integrity token for the block and is
/// deliberately not read: Mars neither replays nor verifies it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub(crate) struct NativeThinkingBlock {
    #[serde(default)]
    pub thinking: String,
}

/// `{"type":"tool_use","id":...,"name":...,"input":{...}}`.
///
/// `id` and `name` are required, which is what sends a malformed tool call to
/// [`NativeContentBlock::Other`]. `input` is passed through untouched, with no
/// redaction (ADR 0027).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct NativeToolUseBlock {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub input: Value,
}

/// The `type` of a block that has a rule, for the diagnostic a malformed one
/// produces.
const BLOCK_TYPE_TOOL_USE: &str = "tool_use";

impl NativeContentBlock {
    /// The native `type` of a block that reached [`Self::Other`], when it has
    /// one, so the translator can tell a malformed known block from a block
    /// kind it has never heard of.
    pub fn other_type(value: &Value) -> Option<&str> {
        value.get("type").and_then(Value::as_str)
    }

    /// Whether an [`Self::Other`] value is a `tool_use` Mars could not read.
    pub fn is_malformed_tool_use(value: &Value) -> bool {
        Self::other_type(value) == Some(BLOCK_TYPE_TOOL_USE)
    }
}

impl<'de> Deserialize<'de> for NativeContentBlock {
    // The crate's `Result` alias has its own error type, so this one is
    // spelled out.
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        let block = match value.get("type").and_then(Value::as_str) {
            Some("text") => serde_json::from_value(value.clone()).map(Self::Text).ok(),
            Some("thinking") => serde_json::from_value(value.clone())
                .map(Self::Thinking)
                .ok(),
            Some("redacted_thinking") => Some(Self::RedactedThinking),
            Some(BLOCK_TYPE_TOOL_USE) => serde_json::from_value(value.clone())
                .map(Self::ToolUse)
                .ok(),
            _ => None,
        };

        Ok(block.unwrap_or(Self::Other(value)))
    }
}

/// `{"type":"user","message":{...},"parent_tool_use_id":...}` (`SPEC.md`,
/// "AgentEvent", the `user` rule).
///
/// The CLI writes a `user` line for two quite different things: the tool
/// results it feeds back to the model, and the plain text of a message — its
/// echo of one the orchestrator wrote, a subagent's prompt, or the
/// `[Request interrupted by user]` line it writes on `SIGINT`
/// (`images/claude/VERIFY.md`). The translator tells them apart by the content
/// blocks, so both shapes have to survive parsing.
///
/// The 2.1.274 recording repeats the tool result in a top-level
/// `tool_use_result` field; it is deliberately not read, because the block
/// inside `message.content` already carries it.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct NativeUser {
    #[serde(default)]
    pub message: Option<NativeUserMessage>,
}

/// The `message` object of a `user` line.
///
/// No `id`: the CLI does not give its `user` lines a backend message id, and an
/// event with none simply leaves the field off.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct NativeUserMessage {
    /// Absent, `null`, a bare string and a block array are all accepted.
    #[serde(default)]
    pub content: Option<NativeUserContent>,
}

/// `message.content` of a `user` line.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub(crate) enum NativeUserContent {
    Text(String),
    Blocks(Vec<NativeUserBlock>),
}

/// One entry of a `user` message's `content`.
///
/// Deliberately its own enum rather than [`NativeContentBlock`]: the two block
/// sets barely overlap (a `user` message carries `tool_result`, an `assistant`
/// message never does) and the `user` rule needs every block it has no rule for
/// — including a `thinking` or `tool_use` block that has no business being
/// here — kept verbatim as [`Self::Other`], so it can become a `raw` event
/// carrying exactly what arrived.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NativeUserBlock {
    Text(NativeTextBlock),
    ToolResult(NativeToolResultBlock),
    Other(Value),
}

/// `{"type":"tool_result","tool_use_id":...,"content":...,"is_error":...}`.
///
/// `tool_use_id` is required, which is what sends a result the frontend could
/// not pair with its call to [`NativeUserBlock::Other`]. `content` is a string
/// in most recorded results and a block array in some, and is passed through
/// untouched apart from the size limit, with no redaction (ADR 0027).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct NativeToolResultBlock {
    pub tool_use_id: String,
    /// Absent and `null` both mean "no content", which the translator stores
    /// as an empty string.
    #[serde(default)]
    pub content: Option<Value>,
    /// Absent means `false` (`SPEC.md`, "AgentEvent", `tool_result`).
    #[serde(default)]
    pub is_error: Option<bool>,
}

/// The `type` of the one `user` block kind that has a rule.
const BLOCK_TYPE_TOOL_RESULT: &str = "tool_result";

/// The `type` of a `user` block whose text takes part in the echo hash.
const BLOCK_TYPE_TEXT: &str = "text";

impl<'de> Deserialize<'de> for NativeUserBlock {
    // The crate's `Result` alias has its own error type, so this one is
    // spelled out.
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        let block = match value.get("type").and_then(Value::as_str) {
            Some(BLOCK_TYPE_TEXT) => serde_json::from_value(value.clone()).map(Self::Text).ok(),
            Some(BLOCK_TYPE_TOOL_RESULT) => serde_json::from_value(value.clone())
                .map(Self::ToolResult)
                .ok(),
            _ => None,
        };

        Ok(block.unwrap_or(Self::Other(value)))
    }
}

/// `{"type":"stream_event","event":{...}}`, which the CLI only writes with
/// `--include-partial-messages` (`ARCHITECTURE.md`, "Claude Code invocation").
///
/// Only a text delta has a rule; every other stream event is dropped, because
/// the complete block follows in the `assistant` message.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct NativeStreamEvent {
    #[serde(default)]
    pub event: Option<NativeStreamEventInner>,
}

/// The `event` object of a `stream_event` line.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct NativeStreamEventInner {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub delta: Option<NativeStreamDelta>,
}

/// The `delta` of a `content_block_delta` stream event.
///
/// `thinking_delta`, `signature_delta` and `input_json_delta` all arrive in
/// this shape and are told apart by `kind`; only `text_delta` carries `text`.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct NativeStreamDelta {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn an_init_keeps_its_unknown_fields_and_defaults_the_missing_ones() {
        let init: NativeSystemInit = serde_json::from_value(json!({
            "type": "system",
            "subtype": "init",
            "session_id": "fake-cli-session",
            "mcp_servers": [{ "name": "mars", "status": "failed", "source": "dynamic" }],
            "claude_code_version": "2.1.274",
        }))
        .unwrap();

        assert_eq!(init.session_id.as_deref(), Some("fake-cli-session"));
        assert_eq!(init.model, None);
        assert!(init.tools.is_empty());
        assert_eq!(init.mcp_servers.len(), 1);
        assert_eq!(init.mcp_servers[0].name, "mars");
        assert_eq!(init.mcp_servers[0].status, "failed");
        assert_eq!(init.mcp_servers[0].extra["source"], json!("dynamic"));
        assert_eq!(init.extra["claude_code_version"], json!("2.1.274"));
    }

    #[test]
    fn a_result_defaults_every_field_it_is_missing() {
        let result: NativeResult = serde_json::from_value(json!({ "type": "result" })).unwrap();

        assert_eq!(result.subtype, None);
        assert_eq!(result.terminal_reason, None);
        assert!(!result.is_error);
        assert_eq!(result.num_turns, 0);
        assert_eq!(result.duration_ms, 0);
        assert_eq!(result.total_cost_usd, None);
        assert_eq!(result.usage, None);
        assert!(result.permission_denials.is_empty());
    }

    #[test]
    fn a_result_reads_the_recorded_cost_and_denials() {
        let result: NativeResult = serde_json::from_value(json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "num_turns": 2,
            "duration_ms": 4335,
            "terminal_reason": "completed",
            "total_cost_usd": 0.1633155,
            "usage": { "input_tokens": 12 },
            "permission_denials": [{
                "tool_name": "Bash",
                "tool_use_id": "toolu_01FIXTURE0008",
                "tool_input": { "command": "true" },
            }],
        }))
        .unwrap();

        assert_eq!(result.subtype.as_deref(), Some("success"));
        assert_eq!(result.terminal_reason.as_deref(), Some("completed"));
        assert_eq!(result.num_turns, 2);
        assert_eq!(result.duration_ms, 4335);
        assert_eq!(result.total_cost_usd, Some(0.1633155));
        assert_eq!(result.usage, Some(json!({ "input_tokens": 12 })));
        assert_eq!(result.permission_denials.len(), 1);
    }

    #[test]
    fn a_denial_reads_the_recorded_field_names() {
        let denied: NativePermissionDenied = serde_json::from_value(json!({
            "type": "system",
            "subtype": "permission_denied",
            "tool_name": "Bash",
            "tool_use_id": "toolu_01FIXTURE0008",
            "decision_reason_type": "subcommandResults",
            "message": "Permission to use Bash has been denied.",
        }))
        .unwrap();

        assert_eq!(denied.tool_name.as_deref(), Some("Bash"));
        assert_eq!(denied.tool, None);
        assert_eq!(denied.tool_use_id.as_deref(), Some("toolu_01FIXTURE0008"));
        assert_eq!(
            denied.message.as_deref(),
            Some("Permission to use Bash has been denied."),
        );
        assert_eq!(
            denied.extra["decision_reason_type"],
            json!("subcommandResults")
        );
    }

    #[test]
    fn an_assistant_reads_the_recorded_block_kinds_in_order() {
        let assistant: NativeAssistant = serde_json::from_value(json!({
            "type": "assistant",
            "message": {
                "id": "msg_01FIXTURE0001",
                "role": "assistant",
                "content": [
                    { "type": "thinking", "thinking": "", "signature": "fixture-signature" },
                    { "type": "text", "text": "hello" },
                    {
                        "type": "tool_use",
                        "id": "toolu_01FIXTURE0001",
                        "name": "Read",
                        "input": { "file_path": "/session/work/README.md" },
                        "caller": { "type": "direct" },
                    },
                    { "type": "server_tool_use", "id": "srvtoolu_1" },
                ],
            },
            "parent_tool_use_id": null,
        }))
        .unwrap();

        let message = assistant.message.unwrap();
        assert_eq!(message.id.as_deref(), Some("msg_01FIXTURE0001"));
        let Some(NativeContent::Blocks(blocks)) = message.content else {
            panic!("expected a block array");
        };
        assert_eq!(
            blocks,
            vec![
                NativeContentBlock::Thinking(NativeThinkingBlock {
                    thinking: String::new(),
                }),
                NativeContentBlock::Text(NativeTextBlock {
                    text: "hello".to_string(),
                }),
                NativeContentBlock::ToolUse(NativeToolUseBlock {
                    id: "toolu_01FIXTURE0001".to_string(),
                    name: "Read".to_string(),
                    input: json!({ "file_path": "/session/work/README.md" }),
                }),
                NativeContentBlock::Other(json!({
                    "type": "server_tool_use",
                    "id": "srvtoolu_1",
                })),
            ],
        );
    }

    #[test]
    fn a_tool_use_without_an_id_is_kept_as_other() {
        let block: NativeContentBlock =
            serde_json::from_value(json!({ "type": "tool_use", "name": "Read" })).unwrap();

        let NativeContentBlock::Other(value) = &block else {
            panic!("expected other");
        };
        assert!(NativeContentBlock::is_malformed_tool_use(value));
    }

    #[test]
    fn a_redacted_thinking_block_needs_no_fields() {
        let block: NativeContentBlock =
            serde_json::from_value(json!({ "type": "redacted_thinking", "data": "opaque" }))
                .unwrap();
        assert_eq!(block, NativeContentBlock::RedactedThinking);
    }

    #[test]
    fn an_assistant_accepts_a_string_or_a_missing_content() {
        let assistant: NativeAssistant =
            serde_json::from_value(json!({ "message": { "content": "plain" } })).unwrap();
        let Some(NativeContent::Text(text)) = assistant.message.unwrap().content else {
            panic!("expected a string content");
        };
        assert_eq!(text, "plain");

        let assistant: NativeAssistant =
            serde_json::from_value(json!({ "message": { "content": null } })).unwrap();
        assert!(assistant.message.unwrap().content.is_none());

        let assistant: NativeAssistant =
            serde_json::from_value(json!({ "type": "assistant" })).unwrap();
        assert!(assistant.message.is_none());
    }

    #[test]
    fn a_user_line_reads_its_tool_result_blocks() {
        let user: NativeUser = serde_json::from_value(json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [
                    {
                        "tool_use_id": "toolu_01FIXTURE0001",
                        "type": "tool_result",
                        "content": "1\t# Greeter\n",
                    },
                    {
                        "tool_use_id": "toolu_01FIXTURE0002",
                        "type": "tool_result",
                        "content": [{ "type": "text", "text": "rows" }],
                        "is_error": true,
                    },
                ],
            },
            "tool_use_result": { "ignored": true },
            "parent_tool_use_id": null,
        }))
        .unwrap();

        let Some(NativeUserContent::Blocks(blocks)) = user.message.unwrap().content else {
            panic!("expected a block array");
        };
        assert_eq!(
            blocks,
            vec![
                NativeUserBlock::ToolResult(NativeToolResultBlock {
                    tool_use_id: "toolu_01FIXTURE0001".to_string(),
                    content: Some(json!("1\t# Greeter\n")),
                    is_error: None,
                }),
                NativeUserBlock::ToolResult(NativeToolResultBlock {
                    tool_use_id: "toolu_01FIXTURE0002".to_string(),
                    content: Some(json!([{ "type": "text", "text": "rows" }])),
                    is_error: Some(true),
                }),
            ],
        );
    }

    #[test]
    fn a_user_line_accepts_text_blocks_a_string_and_no_content() {
        let user: NativeUser = serde_json::from_value(json!({
            "type": "user",
            "message": { "content": [{ "type": "text", "text": "[Request interrupted by user]" }] },
        }))
        .unwrap();
        assert_eq!(
            user.message.unwrap().content,
            Some(NativeUserContent::Blocks(vec![NativeUserBlock::Text(
                NativeTextBlock {
                    text: "[Request interrupted by user]".to_string(),
                }
            )])),
        );

        let user: NativeUser =
            serde_json::from_value(json!({ "message": { "content": "plain" } })).unwrap();
        assert_eq!(
            user.message.unwrap().content,
            Some(NativeUserContent::Text("plain".to_string())),
        );

        let user: NativeUser = serde_json::from_value(json!({ "type": "user" })).unwrap();
        assert!(user.message.is_none());
    }

    #[test]
    fn a_tool_result_without_a_tool_use_id_is_kept_as_other() {
        let block: NativeUserBlock =
            serde_json::from_value(json!({ "type": "tool_result", "content": "x" })).unwrap();
        assert_eq!(
            block,
            NativeUserBlock::Other(json!({ "type": "tool_result", "content": "x" })),
        );

        let block: NativeUserBlock =
            serde_json::from_value(json!({ "type": "image", "source": {} })).unwrap();
        assert_eq!(
            block,
            NativeUserBlock::Other(json!({ "type": "image", "source": {} })),
        );
    }

    #[test]
    fn a_stream_event_reads_the_delta_kind_and_text() {
        let event: NativeStreamEvent = serde_json::from_value(json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "This" },
            },
            "session_id": "00000000-0000-4000-8000-000000000001",
        }))
        .unwrap();

        let inner = event.event.unwrap();
        assert_eq!(inner.kind.as_deref(), Some("content_block_delta"));
        let delta = inner.delta.unwrap();
        assert_eq!(delta.kind.as_deref(), Some("text_delta"));
        assert_eq!(delta.text.as_deref(), Some("This"));
    }

    #[test]
    fn a_stream_event_without_a_delta_still_parses() {
        let event: NativeStreamEvent = serde_json::from_value(
            json!({ "type": "stream_event", "event": { "type": "message_stop" } }),
        )
        .unwrap();
        let inner = event.event.unwrap();
        assert_eq!(inner.kind.as_deref(), Some("message_stop"));
        assert!(inner.delta.is_none());
    }
}
