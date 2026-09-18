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
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct NativeResult {
    #[serde(default)]
    pub subtype: Option<String>,
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
}
