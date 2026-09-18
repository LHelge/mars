//! One native Claude Code line into the events it produces (`SPEC.md`,
//! "AgentEvent", Translation rules).
//!
//! [`translate_line`] is the whole dispatcher: it parses the line as JSON,
//! branches on the native `type`, and returns zero or more [`AgentEvent`]s. The
//! rules that do not need assistant content live here in full — `system`/`init`,
//! `system`/`permission_denied`, `result` with its cost, usage and denial list,
//! and the fatal authentication `error`. The `assistant`, `user` and
//! `stream_event` branches are named but still answer `raw`; their tasks fill
//! them in.
//!
//! Two invariants hold for every branch. Nothing is ever dropped: a line with no
//! rule, a line that is not JSON and a line whose shape does not parse all
//! become a `raw` event carrying what arrived. And nothing ever panics on
//! native input — every field is optional, every extraction falls back.

use serde_json::{Map, Value};

use super::super::TranslateState;
use crate::events::{AgentEvent, AgentEventBody, McpServerStatus};
use crate::models::AgentBackend as Backend;
use crate::prelude::*;

use super::native::{NativePermissionDenied, NativeResult, NativeSystemInit};

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
        // Filled in by the assistant, user and partial-message tasks. Until
        // then the lines are kept rather than dropped.
        Some("assistant") | Some("user") | Some("stream_event") => vec![raw(native.clone())],
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
