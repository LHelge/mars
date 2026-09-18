//! One native Claude Code line into the events it produces (`SPEC.md`,
//! "AgentEvent", Translation rules).
//!
//! [`translate_line`] is the whole dispatcher: it parses the line as JSON,
//! branches on the native `type`, and returns zero or more [`AgentEvent`]s:
//! `system`/`init`, `system`/`permission_denied`, `result` with its cost, usage
//! and denial list, the fatal authentication `error`, the `assistant` message's
//! content blocks and the `stream_event` text deltas. The `user` branch is named
//! but still answers `raw`; its task fills it in.
//!
//! Two invariants hold for every branch. Nothing is dropped except where a rule
//! says so: a line with no rule, a line that is not JSON and a line whose shape
//! does not parse all become a `raw` event carrying what arrived, and the one
//! deliberate exception is the `stream_event` branch, where everything but a
//! text delta is dropped because the complete block follows in the `assistant`
//! message. And nothing ever panics on native input — every field is optional,
//! every extraction falls back.

use serde_json::{Map, Value};

use super::super::TranslateState;
use crate::events::{AgentEvent, AgentEventBody, McpServerStatus};
use crate::models::AgentBackend as Backend;
use crate::prelude::*;

use super::native::{
    NativeAssistant, NativeContent, NativeContentBlock, NativePermissionDenied, NativeResult,
    NativeStreamEvent, NativeSystemInit,
};

/// The tool that starts a subagent, under both names the CLI has used
/// (`SPEC.md`, "AgentEvent", the `assistant` rule). Matched exactly and
/// case-sensitively, as the CLI writes them.
pub(crate) const SUBAGENT_TOOL_NAMES: [&str; 2] = ["Task", "Agent"];

/// The `event.type` of the only stream event with a rule.
const STREAM_CONTENT_BLOCK_DELTA: &str = "content_block_delta";

/// The `event.delta.type` of the only delta with a rule.
const STREAM_TEXT_DELTA: &str = "text_delta";

/// The tool name a denial falls back to when the native line names none.
const UNKNOWN_TOOL: &str = "unknown";

/// The reason a denial falls back to when the native line gives none.
const DEFAULT_DENIAL_REASON: &str = "denied";

/// The `result` subtype used when the native line carries none.
const UNKNOWN_RESULT_SUBTYPE: &str = "unknown";

/// Lower-case substrings that identify an authentication failure.
///
/// The first four are the CLI's user-facing wording; `authentication_failed` is
/// what the 2.1.274 probe recorded in the `system`/`api_retry` lines
/// (`images/claude/VERIFY.md`, "Observed on 2.1.274"). `401` is handled
/// separately, because it is short enough to appear in prose.
const AUTH_FAILURE_NEEDLES: &[&str] = &[
    "invalid api key",
    "authentication_error",
    "authentication_failed",
    "not logged in",
    "please run /login",
];

/// Keys whose value is free text the needles are matched against.
const AUTH_TEXT_KEYS: &[&str] = &[
    "error",
    "error_message",
    "message",
    "reason",
    "result",
    "subtype",
    "type",
];

/// Keys that carry an HTTP status, as a number or as a string.
const AUTH_STATUS_KEYS: &[&str] = &[
    "api_error_status",
    "error_status",
    "http_status",
    "status_code",
];

/// Keys whose text may also be matched against the bare status `401`.
///
/// Deliberately not `result` or `message`: an agent that ran `curl` and
/// reported a 401 back must not park its own session.
const AUTH_ERROR_TEXT_KEYS: &[&str] = &["error", "error_message"];

/// How deep the authentication scan descends into a native line.
const AUTH_SCAN_MAX_DEPTH: usize = 6;

/// Translate one native line into the events it produces.
///
/// An empty or whitespace-only line produces nothing: the tail may deliver one
/// at end of file, and a translator that answered `raw` to it would write an
/// event for a line the CLI never wrote.
pub(crate) fn translate_line(line: &str, state: &mut TranslateState) -> Vec<AgentEvent> {
    if line.trim().is_empty() {
        return Vec::new();
    }

    // Not JSON at all: the line is kept verbatim, as a JSON string.
    let Ok(native) = serde_json::from_str::<Value>(line) else {
        return logged(vec![raw(Value::from(line))]);
    };

    // A JSON array or scalar is kept as the value it is.
    let Some(object) = native.as_object() else {
        return logged(vec![raw(native)]);
    };

    let parent_tool_use_id = object
        .get("parent_tool_use_id")
        .and_then(Value::as_str)
        .map(str::to_string);

    let events = match object.get("type").and_then(Value::as_str) {
        Some("system") => translate_system(&native, object, state),
        Some("result") => translate_result(&native, state),
        Some("assistant") => translate_assistant(&native, state),
        Some("stream_event") => translate_stream_event(&native),
        // Filled in by the user task. Until then the lines are kept rather
        // than dropped.
        Some("user") => vec![raw(native.clone())],
        _ => vec![raw(native.clone())],
    };

    let events = match parent_tool_use_id {
        Some(id) => events
            .into_iter()
            .map(|event| match event.parent_tool_use_id {
                Some(_) => event,
                None => event.in_subagent(id.clone()),
            })
            .collect(),
        None => events,
    };

    logged(events)
}

/// Every `system` subtype.
fn translate_system(
    native: &Value,
    object: &Map<String, Value>,
    state: &mut TranslateState,
) -> Vec<AgentEvent> {
    let subtype = object.get("subtype").and_then(Value::as_str);

    let mut events = match subtype {
        Some("init") => translate_init(native, state),
        Some("permission_denied") => translate_permission_denied(native, state),
        // `status`, `api_retry`, `thinking_tokens`, `task_*`,
        // `vcs_state_changed` and anything a later CLI adds.
        _ => vec![raw(native.clone())],
    };

    // A retry line is where an authentication failure shows first, several
    // lines before the `result` that ends the turn.
    events.extend(authentication_error(native, state));
    events
}

/// `system`/`init` (`SPEC.md`, "AgentEvent", `init`).
fn translate_init(native: &Value, state: &TranslateState) -> Vec<AgentEvent> {
    let Ok(init) = serde_json::from_value::<NativeSystemInit>(native.clone()) else {
        warn!(kind = "init", "a system/init line did not parse");
        return vec![raw(native.clone())];
    };

    let Some(cli_session_id) = init.session_id.filter(|id| !id.is_empty()) else {
        // Without it the owner cannot record `cli_session_id` and cannot
        // resume, so the line is kept as it arrived and the operator is told.
        warn!(kind = "init", "a system/init line carried no session_id");
        return vec![raw(native.clone())];
    };

    vec![
        AgentEventBody::Init {
            cli_session_id,
            model: init.model,
            tools: init.tools,
            mcp_servers: init
                .mcp_servers
                .into_iter()
                .map(|server| McpServerStatus {
                    name: server.name,
                    status: server.status,
                })
                .collect(),
            // The state's, never the line's: no native field reports it
            // (`images/claude/VERIFY.md`).
            resumed: state.resumed,
        }
        .into(),
    ]
}

/// `system`/`permission_denied`.
fn translate_permission_denied(native: &Value, state: &mut TranslateState) -> Vec<AgentEvent> {
    let Ok(denied) = serde_json::from_value::<NativePermissionDenied>(native.clone()) else {
        warn!(kind = "permission_denied", "a denial line did not parse");
        return vec![raw(native.clone())];
    };

    vec![denial_event(&denied, state)]
}

/// `result`, its denial list and any authentication failure it reports.
fn translate_result(native: &Value, state: &mut TranslateState) -> Vec<AgentEvent> {
    let Ok(result) = serde_json::from_value::<NativeResult>(native.clone()) else {
        warn!(kind = "result", "a result line did not parse");
        return vec![raw(native.clone())];
    };

    let denials = result.permission_denials.clone();
    let is_error = result.is_error;

    let mut events = vec![
        AgentEventBody::Result {
            subtype: result
                .subtype
                .unwrap_or_else(|| UNKNOWN_RESULT_SUBTYPE.to_string()),
            is_error: result.is_error,
            num_turns: result.num_turns,
            duration_ms: result.duration_ms,
            cost_usd: result.total_cost_usd,
            usage: result.usage,
            permission_denials: result.permission_denials,
        }
        .into(),
    ];

    // One event per denial the turn's own `permission_denied` line did not
    // already report (`ARCHITECTURE.md`, "Claude Code invocation": both are
    // translated, and the same denial appears in both).
    for denial in denials {
        let Ok(denial) = serde_json::from_value::<NativePermissionDenied>(denial) else {
            continue;
        };
        if let Some(id) = denial.tool_use_id.as_deref()
            && state.denied_tool_use_ids.contains(id)
        {
            continue;
        }
        events.push(denial_event(&denial, state));
    }

    // Only a failed turn can report a failed credential: the `result` text of a
    // successful turn is the agent's own prose, and an agent that fixed an
    // "Invalid API key" message must not park its own session.
    if is_error {
        events.extend(authentication_error(native, state));
    }
    events
}

/// `assistant`: one event per content block, in block order (`SPEC.md`,
/// "AgentEvent", the `assistant` rule).
///
/// Every event carries the native `message.id` as `message_id`; the top-level
/// `parent_tool_use_id` is copied on by the dispatcher, so a message produced
/// inside a subagent is translated exactly like one at the top level.
fn translate_assistant(native: &Value, state: &mut TranslateState) -> Vec<AgentEvent> {
    let Ok(assistant) = serde_json::from_value::<NativeAssistant>(native.clone()) else {
        warn!(kind = "assistant", "an assistant line did not parse");
        return vec![raw(native.clone())];
    };

    let Some(message) = assistant.message else {
        warn!(kind = "assistant", "an assistant line carried no message");
        return vec![raw(native.clone())];
    };

    // Absent, `null` and `[]` all mean the message said nothing, and a message
    // that said nothing produces nothing.
    let events = match message.content {
        None => Vec::new(),
        Some(NativeContent::Text(text)) => vec![AgentEventBody::Text { text }.into()],
        Some(NativeContent::Blocks(blocks)) => blocks
            .into_iter()
            .flat_map(|block| translate_content_block(block, state))
            .collect(),
    };

    match message.id {
        Some(id) => events
            .into_iter()
            .map(|event| event.with_message_id(id.clone()))
            .collect(),
        None => events,
    }
}

/// One content block into the one or two events it produces.
fn translate_content_block(
    block: NativeContentBlock,
    state: &mut TranslateState,
) -> Vec<AgentEvent> {
    match block {
        // Empty text is still an event: what to render is the frontend's call.
        NativeContentBlock::Text(text) => vec![AgentEventBody::Text { text: text.text }.into()],
        NativeContentBlock::Thinking(thinking) => vec![
            AgentEventBody::Thinking {
                text: thinking.thinking,
                redacted: false,
            }
            .into(),
        ],
        NativeContentBlock::RedactedThinking => vec![
            AgentEventBody::Thinking {
                text: String::new(),
                redacted: true,
            }
            .into(),
        ],
        NativeContentBlock::ToolUse(tool_use) => {
            let subagent = SUBAGENT_TOOL_NAMES.contains(&tool_use.name.as_str());
            let mut events = vec![
                AgentEventBody::ToolCall {
                    tool_use_id: tool_use.id.clone(),
                    name: tool_use.name,
                    // Untouched, with no redaction (ADR 0027).
                    input: tool_use.input.clone(),
                }
                .into(),
            ];

            if subagent {
                state.open_subagents.insert(tool_use.id.clone(), ());
                events.push(
                    AgentEventBody::SubagentStart {
                        tool_use_id: tool_use.id,
                        description: tool_use
                            .input
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        agent_type: tool_use
                            .input
                            .get("subagent_type")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    }
                    .into(),
                );
            }

            events
        }
        // A block kind with no rule, or a `tool_use` without the id the
        // frontend needs to pair it with its result: kept, never dropped.
        NativeContentBlock::Other(value) => {
            if NativeContentBlock::is_malformed_tool_use(&value) {
                warn!(block_type = "tool_use", "a tool_use block carried no id");
            }
            vec![raw(value)]
        }
    }
}

/// `stream_event`: text deltas only (`SPEC.md`, "AgentEvent", the
/// `stream_event` rule).
///
/// The one branch that drops a line on purpose. `message_start`,
/// `content_block_start`, `thinking_delta`, `signature_delta`,
/// `input_json_delta`, `message_delta`, `message_stop` and anything a later CLI
/// adds produce nothing, because the complete block follows in the `assistant`
/// message and a `raw` per token would double the rows for no reader.
fn translate_stream_event(native: &Value) -> Vec<AgentEvent> {
    let Ok(stream) = serde_json::from_value::<NativeStreamEvent>(native.clone()) else {
        warn!(kind = "stream_event", "a stream_event line did not parse");
        return vec![raw(native.clone())];
    };

    let Some(event) = stream.event else {
        return Vec::new();
    };
    if event.kind.as_deref() != Some(STREAM_CONTENT_BLOCK_DELTA) {
        return Vec::new();
    }

    let Some(delta) = event
        .delta
        .filter(|d| d.kind.as_deref() == Some(STREAM_TEXT_DELTA))
    else {
        return Vec::new();
    };

    vec![
        AgentEventBody::TextDelta {
            text: delta.text.unwrap_or_default(),
        }
        .into(),
    ]
}

/// One `permission_denied` event, recording its `tool_use_id` as seen.
fn denial_event(denied: &NativePermissionDenied, state: &mut TranslateState) -> AgentEvent {
    if let Some(id) = denied.tool_use_id.as_deref() {
        state.denied_tool_use_ids.insert(id.to_string());
    }

    AgentEventBody::PermissionDenied {
        tool_use_id: denied.tool_use_id.clone(),
        name: denied
            .tool_name
            .clone()
            .or_else(|| denied.tool.clone())
            .unwrap_or_else(|| UNKNOWN_TOOL.to_string()),
        reason: denied
            .message
            .clone()
            .or_else(|| denied.reason.clone())
            .unwrap_or_else(|| DEFAULT_DENIAL_REASON.to_string()),
    }
    .into()
}

/// The fatal `error` an authentication failure produces, at most once per
/// process (`ARCHITECTURE.md`, "Claude Code invocation", Credentials).
///
/// The failure is reported by the CLI as a run of `api_retry` lines and then by
/// the `result`; the session is parked once, so the event is emitted once and
/// the state remembers that it was.
fn authentication_error(native: &Value, state: &mut TranslateState) -> Vec<AgentEvent> {
    if state.authentication_failed || !indicates_authentication_failure(native) {
        return Vec::new();
    }
    state.authentication_failed = true;

    // Names the variable and its scope, never a value (rule 3).
    let message = match state.credential {
        Some(credential) => format!(
            "Authentication failed with {} ({} scope); replace the secret and send the next message.",
            credential.name, credential.scope,
        ),
        None => {
            "Authentication failed: no ANTHROPIC_API_KEY or CLAUDE_CODE_OAUTH_TOKEN was injected."
                .to_string()
        }
    };

    vec![
        AgentEventBody::Error {
            message,
            fatal: true,
        }
        .into(),
    ]
}

/// Whether a native line reports that the CLI could not authenticate.
fn indicates_authentication_failure(native: &Value) -> bool {
    scan_for_authentication_failure(native, 0)
}

fn scan_for_authentication_failure(value: &Value, depth: usize) -> bool {
    if depth > AUTH_SCAN_MAX_DEPTH {
        return false;
    }

    match value {
        Value::Object(fields) => fields.iter().any(|(key, field)| {
            field_indicates_authentication_failure(key, field)
                || scan_for_authentication_failure(field, depth + 1)
        }),
        Value::Array(items) => items
            .iter()
            .any(|item| scan_for_authentication_failure(item, depth + 1)),
        _ => false,
    }
}

fn field_indicates_authentication_failure(key: &str, value: &Value) -> bool {
    if AUTH_STATUS_KEYS.contains(&key) {
        return match value {
            Value::Number(number) => number.as_i64() == Some(401),
            Value::String(text) => text.trim() == "401",
            _ => false,
        };
    }

    if !AUTH_TEXT_KEYS.contains(&key) {
        return false;
    }

    let Some(text) = value.as_str() else {
        return false;
    };
    let text = text.to_lowercase();

    AUTH_FAILURE_NEEDLES
        .iter()
        .any(|needle| text.contains(needle))
        || (AUTH_ERROR_TEXT_KEYS.contains(&key) && text.contains("401"))
}

/// A `raw` event for this backend.
fn raw(native: Value) -> AgentEvent {
    AgentEventBody::Raw {
        backend: Backend::Claude,
        native,
    }
    .into()
}

/// Log what was produced, at `debug` and by kind only: an event's fields are
/// agent output and never reach the logs (`CLAUDE.md`, rule 3).
fn logged(events: Vec<AgentEvent>) -> Vec<AgentEvent> {
    for event in &events {
        debug!(kind = %event.kind(), "translated a claude line");
    }
    events
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::super::state::{CredentialName, InjectedCredential, TranslateConfig};
    use super::*;
    use crate::models::SecretScope;

    fn state() -> TranslateState {
        TranslateState::new(TranslateConfig::default())
    }

    fn body(event: &AgentEvent) -> &AgentEventBody {
        &event.body
    }

    #[test]
    fn a_blank_line_produces_nothing() {
        let mut state = state();
        assert!(translate_line("", &mut state).is_empty());
        assert!(translate_line("   \t ", &mut state).is_empty());
    }

    #[test]
    fn a_line_that_is_not_json_is_raw_as_a_string() {
        let mut state = state();
        let events = translate_line("npm warn something", &mut state);
        assert_eq!(events, vec![raw(json!("npm warn something"))],);
    }

    #[test]
    fn a_json_array_or_scalar_is_raw_as_that_value() {
        let mut state = state();
        assert_eq!(
            translate_line("[1,2]", &mut state),
            vec![raw(json!([1, 2]))]
        );
        assert_eq!(translate_line("7", &mut state), vec![raw(json!(7))]);
    }

    #[test]
    fn an_unknown_type_is_raw() {
        let mut state = state();
        let line = json!({ "type": "rate_limit_event", "rate_limit_info": { "used": 0 } });
        assert_eq!(
            translate_line(&line.to_string(), &mut state),
            vec![raw(line)],
        );
    }

    #[test]
    fn an_init_carries_the_session_id_model_tools_and_servers() {
        let mut state = TranslateState::new(TranslateConfig {
            resumed: true,
            ..TranslateConfig::default()
        });
        let line = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "fake-cli-session",
            "model": "claude-test",
            "tools": ["Bash", "Task"],
            "mcp_servers": [{ "name": "mars-orchestrator", "status": "failed", "source": "dynamic" }],
            "claude_code_version": "2.1.274",
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(
            events,
            vec![
                AgentEventBody::Init {
                    cli_session_id: "fake-cli-session".to_string(),
                    model: Some("claude-test".to_string()),
                    tools: vec!["Bash".to_string(), "Task".to_string()],
                    mcp_servers: vec![McpServerStatus {
                        name: "mars-orchestrator".to_string(),
                        status: "failed".to_string(),
                    }],
                    resumed: true,
                }
                .into()
            ],
        );
    }

    #[test]
    fn an_init_without_a_model_or_tools_defaults_them() {
        let mut state = state();
        let line = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "fake-cli-session",
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(
            events,
            vec![
                AgentEventBody::Init {
                    cli_session_id: "fake-cli-session".to_string(),
                    model: None,
                    tools: vec![],
                    mcp_servers: vec![],
                    resumed: false,
                }
                .into()
            ],
        );
    }

    #[test]
    fn an_init_without_a_session_id_is_raw() {
        let mut state = state();
        let line = json!({ "type": "system", "subtype": "init", "model": "claude-test" });
        assert_eq!(
            translate_line(&line.to_string(), &mut state),
            vec![raw(line)],
        );
    }

    #[test]
    fn a_permission_denied_system_line_names_the_tool_and_reason() {
        let mut state = state();
        let line = json!({
            "type": "system",
            "subtype": "permission_denied",
            "tool_name": "Bash",
            "tool_use_id": "toolu_01FIXTURE0008",
            "decision_reason_type": "subcommandResults",
            "message": "Permission to use Bash has been denied.",
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(
            events,
            vec![
                AgentEventBody::PermissionDenied {
                    tool_use_id: Some("toolu_01FIXTURE0008".to_string()),
                    name: "Bash".to_string(),
                    reason: "Permission to use Bash has been denied.".to_string(),
                }
                .into()
            ],
        );
        assert!(state.denied_tool_use_ids.contains("toolu_01FIXTURE0008"));
    }

    #[test]
    fn a_denial_without_a_tool_or_message_falls_back() {
        let mut state = state();
        let line = json!({ "type": "system", "subtype": "permission_denied" });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(
            events,
            vec![
                AgentEventBody::PermissionDenied {
                    tool_use_id: None,
                    name: "unknown".to_string(),
                    reason: "denied".to_string(),
                }
                .into()
            ],
        );
    }

    #[test]
    fn every_other_system_subtype_is_raw() {
        let mut state = state();
        for subtype in [
            "status",
            "thinking_tokens",
            "task_started",
            "vcs_state_changed",
        ] {
            let line = json!({ "type": "system", "subtype": subtype, "session_id": "s" });
            assert_eq!(
                translate_line(&line.to_string(), &mut state),
                vec![raw(line)],
                "{subtype}",
            );
        }
    }

    #[test]
    fn a_result_carries_the_cost_usage_and_an_empty_denial_list() {
        let mut state = state();
        let line = json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "num_turns": 3,
            "duration_ms": 5460,
            "total_cost_usd": 0.0727456,
            "usage": { "input_tokens": 12, "output_tokens": 34 },
            "permission_denials": [],
            "result": "done",
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(
            events,
            vec![
                AgentEventBody::Result {
                    subtype: "success".to_string(),
                    is_error: false,
                    num_turns: 3,
                    duration_ms: 5460,
                    cost_usd: Some(0.0727456),
                    usage: Some(json!({ "input_tokens": 12, "output_tokens": 34 })),
                    permission_denials: vec![],
                }
                .into()
            ],
        );
    }

    #[test]
    fn a_result_emits_one_permission_denied_per_new_denial() {
        let mut state = state();
        let line = json!({
            "type": "result",
            "subtype": "success",
            "num_turns": 2,
            "duration_ms": 4335,
            "permission_denials": [{
                "tool_name": "Bash",
                "tool_use_id": "toolu_01FIXTURE0008",
                "tool_input": { "command": "true" },
            }, {
                "tool_name": "WebFetch",
            }],
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].kind(), "result");
        assert_eq!(
            body(&events[1]),
            &AgentEventBody::PermissionDenied {
                tool_use_id: Some("toolu_01FIXTURE0008".to_string()),
                name: "Bash".to_string(),
                reason: "denied".to_string(),
            },
        );
        assert_eq!(
            body(&events[2]),
            &AgentEventBody::PermissionDenied {
                tool_use_id: None,
                name: "WebFetch".to_string(),
                reason: "denied".to_string(),
            },
        );
    }

    #[test]
    fn a_denial_already_reported_is_not_emitted_twice() {
        let mut state = state();
        let denial = json!({
            "type": "system",
            "subtype": "permission_denied",
            "tool_name": "Bash",
            "tool_use_id": "toolu_01FIXTURE0008",
            "message": "denied by settings",
        });
        assert_eq!(translate_line(&denial.to_string(), &mut state).len(), 1);

        let line = json!({
            "type": "result",
            "subtype": "success",
            "permission_denials": [{
                "tool_name": "Bash",
                "tool_use_id": "toolu_01FIXTURE0008",
                "tool_input": { "command": "true" },
            }],
        });
        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind(), "result");
    }

    #[test]
    fn a_result_reporting_an_invalid_api_key_names_the_injected_credential() {
        let mut state = TranslateState::new(TranslateConfig {
            credential: Some(InjectedCredential {
                name: CredentialName::AnthropicApiKey,
                scope: SecretScope::Project,
            }),
            ..TranslateConfig::default()
        });
        let line = json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "result": "Invalid API key · Please run /login",
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind(), "result");
        assert_eq!(
            body(&events[1]),
            &AgentEventBody::Error {
                message: "Authentication failed with ANTHROPIC_API_KEY (project scope); \
                          replace the secret and send the next message."
                    .to_string(),
                fatal: true,
            },
        );
    }

    #[test]
    fn an_authentication_failure_without_a_credential_says_none_was_injected() {
        let mut state = state();
        let line = json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "error": { "type": "authentication_error", "message": "invalid x-api-key" },
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(
            body(&events[1]),
            &AgentEventBody::Error {
                message: "Authentication failed: no ANTHROPIC_API_KEY or \
                          CLAUDE_CODE_OAUTH_TOKEN was injected."
                    .to_string(),
                fatal: true,
            },
        );
    }

    #[test]
    fn the_recorded_api_retry_run_produces_one_fatal_error() {
        let mut state = TranslateState::new(TranslateConfig {
            credential: Some(InjectedCredential {
                name: CredentialName::ClaudeCodeOauthToken,
                scope: SecretScope::User,
            }),
            ..TranslateConfig::default()
        });
        let retry = json!({
            "type": "system",
            "subtype": "api_retry",
            "error_status": 401,
            "error": "authentication_failed",
            "attempt": 1,
        });

        let events = translate_line(&retry.to_string(), &mut state);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], raw(retry.clone()));
        assert_eq!(
            body(&events[1]),
            &AgentEventBody::Error {
                message: "Authentication failed with CLAUDE_CODE_OAUTH_TOKEN (user scope); \
                          replace the secret and send the next message."
                    .to_string(),
                fatal: true,
            },
        );

        // Every later retry line, and the `result` that ends the run, are
        // translated without a second fatal error.
        let events = translate_line(&retry.to_string(), &mut state);
        assert_eq!(events, vec![raw(retry)]);

        let line = json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "error": "authentication_failed",
        });
        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind(), "result");
    }

    #[test]
    fn an_error_result_without_authentication_text_is_just_a_result() {
        let mut state = state();
        let line = json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "terminal_reason": "aborted_streaming",
            "result": "the request returned 401 from example.invalid",
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind(), "result");
    }

    #[test]
    fn a_successful_result_that_mentions_an_authentication_failure_is_just_a_result() {
        let mut state = state();
        let line = json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": "Reworded the \"Invalid API key\" message as asked.",
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind(), "result");
        assert!(!state.authentication_failed);
    }

    #[test]
    fn a_parent_tool_use_id_is_copied_onto_every_event() {
        let mut state = state();
        let line = json!({
            "type": "result",
            "subtype": "success",
            "parent_tool_use_id": "toolu_01FIXTURE0001",
            "permission_denials": [{ "tool_name": "WebFetch" }],
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(events.len(), 2);
        for event in &events {
            assert_eq!(
                event.parent_tool_use_id.as_deref(),
                Some("toolu_01FIXTURE0001"),
            );
        }
    }

    #[test]
    fn an_assistant_message_yields_one_event_per_block_in_order() {
        let mut state = state();
        let line = json!({
            "type": "assistant",
            "message": {
                "id": "msg_01FIXTURE0001",
                "role": "assistant",
                "content": [
                    { "type": "thinking", "thinking": "weighing it", "signature": "fixture-sig" },
                    { "type": "text", "text": "" },
                    {
                        "type": "tool_use",
                        "id": "toolu_01FIXTURE0001",
                        "name": "Read",
                        "input": { "file_path": "/session/work/README.md" },
                    },
                ],
            },
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(
            events.iter().map(AgentEvent::kind).collect::<Vec<_>>(),
            vec!["thinking", "text", "tool_call"],
        );
        assert_eq!(
            body(&events[0]),
            &AgentEventBody::Thinking {
                text: "weighing it".to_string(),
                redacted: false,
            },
        );
        assert_eq!(
            body(&events[1]),
            &AgentEventBody::Text {
                text: String::new(),
            },
        );
        assert_eq!(
            body(&events[2]),
            &AgentEventBody::ToolCall {
                tool_use_id: "toolu_01FIXTURE0001".to_string(),
                name: "Read".to_string(),
                input: json!({ "file_path": "/session/work/README.md" }),
            },
        );
        for event in &events {
            assert_eq!(event.message_id.as_deref(), Some("msg_01FIXTURE0001"));
        }
        assert!(state.open_subagents.is_empty());
    }

    #[test]
    fn a_redacted_thinking_block_is_an_empty_redacted_thinking() {
        let mut state = state();
        let line = json!({
            "type": "assistant",
            "message": { "content": [{ "type": "redacted_thinking", "data": "opaque" }] },
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(
            events,
            vec![
                AgentEventBody::Thinking {
                    text: String::new(),
                    redacted: true,
                }
                .into()
            ],
        );
        assert_eq!(events[0].message_id, None);
    }

    #[test]
    fn both_subagent_tool_names_also_yield_a_subagent_start() {
        for name in SUBAGENT_TOOL_NAMES {
            let mut state = state();
            let line = json!({
                "type": "assistant",
                "message": {
                    "id": "msg_01FIXTUREAGENT00000000001",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_01FIXTUREagent0000000001",
                        "name": name,
                        "input": {
                            "description": "Count modules",
                            "prompt": "Count the Rust modules and report the number.",
                            "subagent_type": "general-purpose",
                        },
                    }],
                },
            });

            let events = translate_line(&line.to_string(), &mut state);
            assert_eq!(events.len(), 2, "{name}");
            assert_eq!(events[0].kind(), "tool_call", "{name}");
            assert_eq!(
                body(&events[1]),
                &AgentEventBody::SubagentStart {
                    tool_use_id: "toolu_01FIXTUREagent0000000001".to_string(),
                    description: "Count modules".to_string(),
                    agent_type: Some("general-purpose".to_string()),
                },
                "{name}",
            );
            assert_eq!(
                events[1].message_id.as_deref(),
                Some("msg_01FIXTUREAGENT00000000001"),
                "{name}",
            );
            assert!(
                state
                    .open_subagents
                    .contains_key("toolu_01FIXTUREagent0000000001"),
                "{name}",
            );
        }
    }

    #[test]
    fn a_subagent_call_without_a_description_or_type_falls_back() {
        let mut state = state();
        let line = json!({
            "type": "assistant",
            "message": {
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_01FIXTUREagent0000000002",
                    "name": "Task",
                    "input": { "prompt": "go" },
                }],
            },
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(
            body(&events[1]),
            &AgentEventBody::SubagentStart {
                tool_use_id: "toolu_01FIXTUREagent0000000002".to_string(),
                description: String::new(),
                agent_type: None,
            },
        );
    }

    #[test]
    fn an_ordinary_tool_call_does_not_open_a_subagent() {
        let mut state = state();
        for name in ["task", "AgentTool", "Bash"] {
            let line = json!({
                "type": "assistant",
                "message": {
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_01FIXTURE0002",
                        "name": name,
                        "input": {},
                    }],
                },
            });

            let events = translate_line(&line.to_string(), &mut state);
            assert_eq!(events.len(), 1, "{name}");
            assert_eq!(events[0].kind(), "tool_call", "{name}");
        }
        assert!(state.open_subagents.is_empty());
    }

    #[test]
    fn a_parent_tool_use_id_reaches_every_event_of_an_assistant_message() {
        let mut state = state();
        let line = json!({
            "type": "assistant",
            "parent_tool_use_id": "toolu_01FIXTUREagent0000000001",
            "message": {
                "id": "msg_01FIXTURE0002",
                "content": [
                    { "type": "text", "text": "inside" },
                    {
                        "type": "tool_use",
                        "id": "toolu_01FIXTURE0003",
                        "name": "Task",
                        "input": { "description": "nested" },
                    },
                ],
            },
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(events.len(), 3);
        for event in &events {
            assert_eq!(
                event.parent_tool_use_id.as_deref(),
                Some("toolu_01FIXTUREagent0000000001"),
            );
            assert_eq!(event.message_id.as_deref(), Some("msg_01FIXTURE0002"));
        }
    }

    #[test]
    fn an_assistant_content_string_is_one_text() {
        let mut state = state();
        let line = json!({
            "type": "assistant",
            "message": { "id": "msg_01FIXTURE0004", "content": "just words" },
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(
            events,
            vec![
                AgentEvent::from(AgentEventBody::Text {
                    text: "just words".to_string(),
                })
                .with_message_id("msg_01FIXTURE0004")
            ],
        );
    }

    #[test]
    fn an_assistant_message_that_says_nothing_yields_nothing() {
        let mut state = state();
        for content in [json!([]), json!(null)] {
            let line = json!({ "type": "assistant", "message": { "content": content } });
            assert!(translate_line(&line.to_string(), &mut state).is_empty());
        }

        let line = json!({ "type": "assistant", "message": { "id": "msg_01FIXTURE0005" } });
        assert!(translate_line(&line.to_string(), &mut state).is_empty());
    }

    #[test]
    fn an_assistant_line_without_a_message_is_raw() {
        let mut state = state();
        let line = json!({ "type": "assistant", "session_id": "fake-cli-session" });
        assert_eq!(
            translate_line(&line.to_string(), &mut state),
            vec![raw(line)],
        );
    }

    #[test]
    fn an_unknown_block_type_is_raw_and_the_other_blocks_are_not_lost() {
        let mut state = state();
        let unknown = json!({ "type": "server_tool_use", "id": "srvtoolu_1" });
        let line = json!({
            "type": "assistant",
            "message": {
                "id": "msg_01FIXTURE0006",
                "content": [unknown.clone(), { "type": "text", "text": "after" }],
            },
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(events.len(), 2);
        assert_eq!(body(&events[0]), &raw(unknown).body);
        assert_eq!(events[0].message_id.as_deref(), Some("msg_01FIXTURE0006"));
        assert_eq!(events[1].kind(), "text");
    }

    #[test]
    fn a_tool_use_block_without_an_id_is_raw() {
        let mut state = state();
        let block = json!({ "type": "tool_use", "name": "Read", "input": {} });
        let line = json!({ "type": "assistant", "message": { "content": [block.clone()] } });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(events, vec![raw(block)]);
    }

    #[test]
    fn a_text_delta_stream_event_is_a_text_delta() {
        let mut state = state();
        let line = json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "This" },
            },
            "session_id": "00000000-0000-4000-8000-000000000001",
            "parent_tool_use_id": "toolu_01FIXTUREagent0000000001",
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(
            events,
            vec![
                AgentEvent::from(AgentEventBody::TextDelta {
                    text: "This".to_string(),
                })
                .in_subagent("toolu_01FIXTUREagent0000000001")
            ],
        );
    }

    #[test]
    fn every_other_stream_event_is_dropped() {
        let mut state = state();
        let events = [
            json!({ "type": "message_start", "message": { "id": "msg_01FIXTURE0001" } }),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "thinking", "thinking": "", "signature": "fixture-sig" },
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "thinking_delta", "thinking": "" },
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "signature_delta", "signature": "fixture-sig" },
            }),
            json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": { "type": "input_json_delta", "partial_json": "" },
            }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" } }),
            json!({ "type": "message_stop" }),
        ];

        for event in events {
            let line = json!({ "type": "stream_event", "event": event.clone() });
            assert!(
                translate_line(&line.to_string(), &mut state).is_empty(),
                "{event}",
            );
        }

        let line = json!({ "type": "stream_event", "session_id": "fake-cli-session" });
        assert!(translate_line(&line.to_string(), &mut state).is_empty());
    }

    #[test]
    fn a_null_parent_tool_use_id_is_not_copied() {
        let mut state = state();
        let line = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "fake-cli-session",
            "parent_tool_use_id": null,
        });

        let events = translate_line(&line.to_string(), &mut state);
        assert_eq!(events[0].parent_tool_use_id, None);
    }
}
