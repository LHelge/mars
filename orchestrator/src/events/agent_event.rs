//! The `AgentEvent` union and its storage contract (`SPEC.md`, "AgentEvent").
//!
//! One schema for every backend: the translator produces these, the session
//! owner appends them, and the WebSocket, REST and frontend consume them
//! unchanged (ADR 0008). Nothing here touches the database — this module owns
//! the serde shape, and [`AgentEvent::into_row_parts`] and
//! [`SessionEvent::from_row`] are the two sides of the mapping onto an
//! `events` row (`docs/data-model.md`, `events`): `kind` and `ts` are columns,
//! everything else is the `payload` object, with the internal `_offset` added
//! on write and every `_`-prefixed key stripped on read.
//!
//! The types carry no redaction: agent output and user messages pass through
//! verbatim (ADR 0027). `usage`, `detail`, `input`, `content` and `native` are
//! opaque `serde_json::Value` and are never validated.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::models::event::{INTERNAL_FIELD_PREFIX, OFFSET_FIELD};
use crate::models::{AgentBackend, SessionState};
use crate::prelude::*;

/// The largest `tool_result` content the translator stores inline, in bytes.
///
/// `docs/data-model.md`, `events`: a tool result above this is truncated and
/// the event carries `truncated: true`, so an `events` row stays well inside
/// the page-size rule. The constant lives here so the translator and the
/// row-size rule share one number.
pub const TOOL_RESULT_MAX_BYTES: usize = 256 * 1024;

/// One MCP server as the CLI reported it at startup (`SPEC.md`, "AgentEvent",
/// `init`).
///
/// `status` is the backend's own word, kept as a string: it is display data,
/// not something Mars branches on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub status: String,
}

/// The signal a `state_change` was caused by (`SPEC.md`, "AgentEvent").
///
/// Spelled in capitals on the wire because that is how the signal is named
/// everywhere else the user sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopSignal {
    #[serde(rename = "SIGINT")]
    Sigint,
    #[serde(rename = "SIGTERM")]
    Sigterm,
}

/// Which git operation a `git` event reports (`SPEC.md`, "AgentEvent";
/// `ARCHITECTURE.md`, "Git model").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitOp {
    Sync,
    Merge,
    Rebase,
    Push,
}

/// The fields every kind carries, plus the kind's own (`SPEC.md`,
/// "AgentEventBase").
///
/// `seq` and `ts` are not here: they are allocated and observed by the writer
/// and live in [`SessionEvent`], which is what a client actually receives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvent {
    /// Present on everything emitted inside a subagent; the frontend groups by
    /// it under the corresponding `tool_call`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
    /// The backend's message id, when the backend provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(flatten)]
    pub body: AgentEventBody,
}

/// The per-kind half of an [`AgentEvent`].
///
/// Internally tagged on `kind` in `snake_case`, which is the wire spelling and
/// also the `events.kind` column value. The column is `TEXT` on purpose: a new
/// variant here needs no migration, so this enum deliberately has no
/// `sqlx::Type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEventBody {
    /// The CLI started (or resumed) and announced its capabilities.
    Init {
        cli_session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        tools: Vec<String>,
        mcp_servers: Vec<McpServerStatus>,
        resumed: bool,
    },
    /// A message the orchestrator wrote into the session on someone's behalf.
    UserMessage {
        text: String,
        /// `string | null` in the spec: an agent- or system-authored message
        /// serialises this as `null` rather than omitting it.
        user_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply_to: Option<i64>,
    },
    /// A partial assistant text block; only emitted with partial messages on.
    TextDelta {
        text: String,
    },
    /// A complete assistant text block.
    Text {
        text: String,
    },
    /// A thinking block. `redacted` reflects the backend's own flag, never a
    /// Mars filter (ADR 0027).
    Thinking {
        text: String,
        redacted: bool,
    },
    ToolCall {
        tool_use_id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Value,
        is_error: bool,
        truncated: bool,
    },
    PermissionDenied {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        name: String,
        reason: String,
    },
    /// A question that needs a [`SessionInput::Answer`](super::SessionInput)
    /// quoting this event's `seq`.
    Prompt {
        prompt_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<Vec<String>>,
    },
    SubagentStart {
        tool_use_id: String,
        description: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_type: Option<String>,
    },
    SubagentEnd {
        tool_use_id: String,
        is_error: bool,
    },
    /// A turn finished; `cost_usd` comes from the backend's total.
    Result {
        subtype: String,
        is_error: bool,
        num_turns: i64,
        duration_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Value>,
        permission_denials: Vec<Value>,
    },
    Error {
        message: String,
        fatal: bool,
    },
    StateChange {
        from: SessionState,
        to: SessionState,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<StopSignal>,
    },
    /// Something the launch could not honour, such as an undeclared secret.
    LaunchWarning {
        message: String,
    },
    /// The outcome of a git operation this session took part in; `detail` is
    /// one shape per `op` (`SPEC.md`, "AgentEvent", `GitDetail`).
    Git {
        op: GitOp,
        ok: bool,
        detail: Value,
    },
    /// A native line the translator had no rule for, kept verbatim.
    Raw {
        backend: AgentBackend,
        native: Value,
    },
}

impl AgentEventBody {
    /// The `kind` tag, which is also the `events.kind` column value.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Init { .. } => "init",
            Self::UserMessage { .. } => "user_message",
            Self::TextDelta { .. } => "text_delta",
            Self::Text { .. } => "text",
            Self::Thinking { .. } => "thinking",
            Self::ToolCall { .. } => "tool_call",
            Self::ToolResult { .. } => "tool_result",
            Self::PermissionDenied { .. } => "permission_denied",
            Self::Prompt { .. } => "prompt",
            Self::SubagentStart { .. } => "subagent_start",
            Self::SubagentEnd { .. } => "subagent_end",
            Self::Result { .. } => "result",
            Self::Error { .. } => "error",
            Self::StateChange { .. } => "state_change",
            Self::LaunchWarning { .. } => "launch_warning",
            Self::Git { .. } => "git",
            Self::Raw { .. } => "raw",
        }
    }
}

impl From<AgentEventBody> for AgentEvent {
    fn from(body: AgentEventBody) -> Self {
        Self::new(body)
    }
}

impl AgentEvent {
    /// An event with no subagent parent and no backend message id.
    pub fn new(body: AgentEventBody) -> Self {
        Self {
            parent_tool_use_id: None,
            message_id: None,
            body,
        }
    }

    /// The same event, emitted inside the subagent identified by `tool_use_id`.
    pub fn in_subagent(mut self, tool_use_id: impl Into<String>) -> Self {
        self.parent_tool_use_id = Some(tool_use_id.into());
        self
    }

    /// The same event, attributed to a backend message.
    pub fn with_message_id(mut self, message_id: impl Into<String>) -> Self {
        self.message_id = Some(message_id.into());
        self
    }

    /// The `kind` tag, for log fields and repository calls.
    pub fn kind(&self) -> &'static str {
        self.body.kind()
    }

    /// The `(kind, payload)` pair an `events` row is written from.
    ///
    /// The payload is the event's JSON object without `kind` — `seq` and `ts`
    /// are never part of it, being the writer's columns — plus [`OFFSET_FIELD`]
    /// when the caller knows the transcript byte offset the line this event
    /// came from ended at (`ARCHITECTURE.md`, "Durability and recovery"). The
    /// owner decides the offset; events it did not translate from the
    /// transcript pass `None`.
    pub fn into_row_parts(&self, offset: Option<u64>) -> (String, Value) {
        let kind = self.kind().to_string();

        // Every variant is a struct variant of a struct with a flattened tag,
        // so this is always a JSON object and always serialises: the only
        // non-derived member is `serde_json::Value`, which cannot fail.
        let mut payload = match serde_json::to_value(self) {
            Ok(Value::Object(fields)) => fields,
            _ => Map::new(),
        };
        payload.remove("kind");

        if let Some(offset) = offset {
            payload.insert(OFFSET_FIELD.to_string(), Value::from(offset));
        }

        (kind, Value::Object(payload))
    }
}

/// A stored event as a client receives it (`SPEC.md`, "AgentEvent").
///
/// Serialises flat — `seq`, `ts`, `kind`, then the kind's fields and the base
/// fields — which is exactly the TypeScript `AgentEvent` interface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEvent {
    /// Per-session, monotonic, starting at 1.
    pub seq: i64,
    /// RFC 3339; the orchestrator's observation time.
    pub ts: DateTime<Utc>,
    #[serde(flatten)]
    pub event: AgentEvent,
}

impl SessionEvent {
    /// The event an `events` row holds.
    ///
    /// Every top-level payload key starting with [`INTERNAL_FIELD_PREFIX`] is
    /// dropped first, so no internal field — `_offset` or any added later —
    /// can leave the orchestrator. Only the top level is filtered: a `_`-keyed
    /// field inside a tool call's arguments is the agent's data, not ours.
    ///
    /// An unknown `kind` or a payload that does not match its kind is an
    /// [`Error::Internal`]: the rows were written by this same code, so a
    /// mismatch is a bug or a hand-edited row, never something a client did.
    pub fn from_row(seq: i64, ts: DateTime<Utc>, kind: &str, payload: Value) -> Result<Self> {
        let Value::Object(fields) = payload else {
            error!(seq, kind, "stored event payload is not a JSON object");
            return Err(Error::Internal("stored event is unreadable".to_string()));
        };

        let mut fields: Map<String, Value> = fields
            .into_iter()
            .filter(|(name, _)| !name.starts_with(INTERNAL_FIELD_PREFIX))
            .collect();
        fields.insert("kind".to_string(), Value::from(kind));

        let event = serde_json::from_value::<AgentEvent>(Value::Object(fields)).map_err(|err| {
            error!(seq, kind, error = %err, "stored event does not match its kind");
            Error::Internal("stored event is unreadable".to_string())
        })?;

        Ok(Self { seq, ts, event })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn ts() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-18T10:11:12Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    /// Round trip `body` against the literal JSON of `SPEC.md`: the event
    /// serialises to it, and it deserialises back to the same event.
    fn round_trip(body: AgentEventBody, expected: Value) {
        let event = AgentEvent::new(body);
        assert_eq!(serde_json::to_value(&event).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<AgentEvent>(expected.clone()).unwrap(),
            event,
        );
        assert_eq!(event.kind(), expected["kind"].as_str().unwrap());
    }

    #[test]
    fn an_init_round_trips() {
        round_trip(
            AgentEventBody::Init {
                cli_session_id: "fake-cli-session".to_string(),
                model: Some("claude-test".to_string()),
                tools: vec!["Bash".to_string()],
                mcp_servers: vec![McpServerStatus {
                    name: "mars".to_string(),
                    status: "connected".to_string(),
                }],
                resumed: true,
            },
            json!({
                "kind": "init",
                "cli_session_id": "fake-cli-session",
                "model": "claude-test",
                "tools": ["Bash"],
                "mcp_servers": [{ "name": "mars", "status": "connected" }],
                "resumed": true,
            }),
        );
    }

    #[test]
    fn an_init_without_a_model_omits_it() {
        round_trip(
            AgentEventBody::Init {
                cli_session_id: "fake-cli-session".to_string(),
                model: None,
                tools: vec![],
                mcp_servers: vec![],
                resumed: false,
            },
            json!({
                "kind": "init",
                "cli_session_id": "fake-cli-session",
                "tools": [],
                "mcp_servers": [],
                "resumed": false,
            }),
        );
    }

    #[test]
    fn a_user_message_round_trips() {
        let user_id = Uuid::nil();
        round_trip(
            AgentEventBody::UserMessage {
                text: "hello".to_string(),
                user_id: Some(user_id),
                client_id: Some("c-1".to_string()),
                reply_to: Some(7),
            },
            json!({
                "kind": "user_message",
                "text": "hello",
                "user_id": user_id,
                "client_id": "c-1",
                "reply_to": 7,
            }),
        );
    }

    #[test]
    fn a_user_message_keeps_a_null_user_id_and_omits_the_rest() {
        round_trip(
            AgentEventBody::UserMessage {
                text: "hello".to_string(),
                user_id: None,
                client_id: None,
                reply_to: None,
            },
            json!({ "kind": "user_message", "text": "hello", "user_id": null }),
        );
    }

    #[test]
    fn a_text_delta_round_trips() {
        round_trip(
            AgentEventBody::TextDelta {
                text: "par".to_string(),
            },
            json!({ "kind": "text_delta", "text": "par" }),
        );
    }

    #[test]
    fn a_text_round_trips() {
        round_trip(
            AgentEventBody::Text {
                text: "partial".to_string(),
            },
            json!({ "kind": "text", "text": "partial" }),
        );
    }

    #[test]
    fn a_thinking_round_trips() {
        round_trip(
            AgentEventBody::Thinking {
                text: "hmm".to_string(),
                redacted: false,
            },
            json!({ "kind": "thinking", "text": "hmm", "redacted": false }),
        );
    }

    #[test]
    fn a_tool_call_round_trips() {
        round_trip(
            AgentEventBody::ToolCall {
                tool_use_id: "toolu_1".to_string(),
                name: "Bash".to_string(),
                input: json!({ "command": "ls" }),
            },
            json!({
                "kind": "tool_call",
                "tool_use_id": "toolu_1",
                "name": "Bash",
                "input": { "command": "ls" },
            }),
        );
    }

    #[test]
    fn a_tool_result_round_trips() {
        round_trip(
            AgentEventBody::ToolResult {
                tool_use_id: "toolu_1".to_string(),
                content: json!("file.txt"),
                is_error: false,
                truncated: true,
            },
            json!({
                "kind": "tool_result",
                "tool_use_id": "toolu_1",
                "content": "file.txt",
                "is_error": false,
                "truncated": true,
            }),
        );
    }

    #[test]
    fn a_permission_denied_round_trips() {
        round_trip(
            AgentEventBody::PermissionDenied {
                tool_use_id: None,
                name: "WebFetch".to_string(),
                reason: "not allowed".to_string(),
            },
            json!({ "kind": "permission_denied", "name": "WebFetch", "reason": "not allowed" }),
        );
    }

    #[test]
    fn a_prompt_round_trips() {
        round_trip(
            AgentEventBody::Prompt {
                prompt_id: "p-1".to_string(),
                text: "which one?".to_string(),
                options: Some(vec!["a".to_string(), "b".to_string()]),
            },
            json!({
                "kind": "prompt",
                "prompt_id": "p-1",
                "text": "which one?",
                "options": ["a", "b"],
            }),
        );
    }

    #[test]
    fn a_subagent_start_and_end_round_trip() {
        round_trip(
            AgentEventBody::SubagentStart {
                tool_use_id: "toolu_2".to_string(),
                description: "explore".to_string(),
                agent_type: Some("Explore".to_string()),
            },
            json!({
                "kind": "subagent_start",
                "tool_use_id": "toolu_2",
                "description": "explore",
                "agent_type": "Explore",
            }),
        );
        round_trip(
            AgentEventBody::SubagentEnd {
                tool_use_id: "toolu_2".to_string(),
                is_error: true,
            },
            json!({ "kind": "subagent_end", "tool_use_id": "toolu_2", "is_error": true }),
        );
    }

    #[test]
    fn a_result_round_trips() {
        round_trip(
            AgentEventBody::Result {
                subtype: "success".to_string(),
                is_error: false,
                num_turns: 3,
                duration_ms: 1200,
                cost_usd: Some(0.25),
                usage: Some(json!({ "input_tokens": 10 })),
                permission_denials: vec![json!({ "tool_name": "WebFetch" })],
            },
            json!({
                "kind": "result",
                "subtype": "success",
                "is_error": false,
                "num_turns": 3,
                "duration_ms": 1200,
                "cost_usd": 0.25,
                "usage": { "input_tokens": 10 },
                "permission_denials": [{ "tool_name": "WebFetch" }],
            }),
        );
    }

    #[test]
    fn an_error_round_trips() {
        round_trip(
            AgentEventBody::Error {
                message: "the cli exited".to_string(),
                fatal: true,
            },
            json!({ "kind": "error", "message": "the cli exited", "fatal": true }),
        );
    }

    #[test]
    fn a_state_change_spells_its_states_and_signal() {
        round_trip(
            AgentEventBody::StateChange {
                from: SessionState::Running,
                to: SessionState::Parked,
                reason: "stopped".to_string(),
                signal: Some(StopSignal::Sigint),
            },
            json!({
                "kind": "state_change",
                "from": "running",
                "to": "parked",
                "reason": "stopped",
                "signal": "SIGINT",
            }),
        );
        assert_eq!(
            serde_json::to_value(StopSignal::Sigterm).unwrap(),
            json!("SIGTERM"),
        );
    }

    #[test]
    fn a_launch_warning_round_trips() {
        round_trip(
            AgentEventBody::LaunchWarning {
                message: "secret FAKE_TOKEN is not declared".to_string(),
            },
            json!({ "kind": "launch_warning", "message": "secret FAKE_TOKEN is not declared" }),
        );
    }

    #[test]
    fn a_git_event_spells_every_op() {
        round_trip(
            AgentEventBody::Git {
                op: GitOp::Sync,
                ok: true,
                detail: json!({ "ref": "main", "commit": "abc123" }),
            },
            json!({
                "kind": "git",
                "op": "sync",
                "ok": true,
                "detail": { "ref": "main", "commit": "abc123" },
            }),
        );

        for (op, spelling) in [
            (GitOp::Sync, "sync"),
            (GitOp::Merge, "merge"),
            (GitOp::Rebase, "rebase"),
            (GitOp::Push, "push"),
        ] {
            assert_eq!(serde_json::to_value(op).unwrap(), json!(spelling));
        }
    }

    #[test]
    fn a_raw_event_names_its_backend() {
        round_trip(
            AgentEventBody::Raw {
                backend: AgentBackend::Claude,
                native: json!({ "type": "unknown" }),
            },
            json!({
                "kind": "raw",
                "backend": "claude",
                "native": { "type": "unknown" },
            }),
        );
    }

    #[test]
    fn the_base_fields_ride_on_every_kind_and_vanish_when_unset() {
        let event = AgentEvent::new(AgentEventBody::Text {
            text: "hi".to_string(),
        })
        .in_subagent("toolu_3")
        .with_message_id("msg_1");

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(
            json,
            json!({
                "kind": "text",
                "text": "hi",
                "parent_tool_use_id": "toolu_3",
                "message_id": "msg_1",
            }),
        );
        assert_eq!(serde_json::from_value::<AgentEvent>(json).unwrap(), event);

        let bare = AgentEvent::from(AgentEventBody::Text {
            text: "hi".to_string(),
        });
        assert_eq!(
            serde_json::to_value(&bare).unwrap(),
            json!({ "kind": "text", "text": "hi" }),
        );
    }

    #[test]
    fn a_session_event_serialises_flat() {
        let stored = SessionEvent {
            seq: 4,
            ts: ts(),
            event: AgentEvent::new(AgentEventBody::Text {
                text: "hi".to_string(),
            })
            .with_message_id("msg_1"),
        };

        let json = serde_json::to_value(&stored).unwrap();
        assert_eq!(
            json,
            json!({
                "seq": 4,
                "ts": "2026-09-18T10:11:12Z",
                "kind": "text",
                "text": "hi",
                "message_id": "msg_1",
            }),
        );
        assert_eq!(
            serde_json::from_value::<SessionEvent>(json).unwrap(),
            stored
        );
    }

    #[test]
    fn row_parts_drop_the_kind_and_carry_the_offset() {
        let event = AgentEvent::new(AgentEventBody::Text {
            text: "hi".to_string(),
        });

        let (kind, payload) = event.into_row_parts(Some(2048));
        assert_eq!(kind, "text");
        assert_eq!(payload, json!({ "text": "hi", "_offset": 2048 }));

        let (_, payload) = event.into_row_parts(None);
        assert_eq!(payload, json!({ "text": "hi" }));
        assert!(payload.get("seq").is_none());
        assert!(payload.get("ts").is_none());
    }

    #[test]
    fn a_row_round_trips_through_its_parts() {
        let event = AgentEvent::new(AgentEventBody::ToolCall {
            tool_use_id: "toolu_1".to_string(),
            name: "Bash".to_string(),
            input: json!({ "command": "ls" }),
        })
        .in_subagent("toolu_0");

        let (kind, payload) = event.into_row_parts(Some(16));
        let stored = SessionEvent::from_row(9, ts(), &kind, payload).unwrap();

        assert_eq!(stored.seq, 9);
        assert_eq!(stored.event, event);
    }

    #[test]
    fn from_row_strips_every_internal_field() {
        let stored = SessionEvent::from_row(
            1,
            ts(),
            "text",
            json!({ "text": "hi", "_offset": 512, "_future": { "anything": true } }),
        )
        .unwrap();

        assert_eq!(
            serde_json::to_value(&stored).unwrap(),
            json!({ "seq": 1, "ts": "2026-09-18T10:11:12Z", "kind": "text", "text": "hi" }),
        );
    }

    #[test]
    fn from_row_keeps_nested_underscore_keys() {
        let stored = SessionEvent::from_row(
            1,
            ts(),
            "tool_call",
            json!({
                "tool_use_id": "toolu_1",
                "name": "Bash",
                "input": { "_raw": "ls" },
                "_offset": 1,
            }),
        )
        .unwrap();

        match stored.event.body {
            AgentEventBody::ToolCall { input, .. } => assert_eq!(input, json!({ "_raw": "ls" })),
            other => panic!("wrong body: {other:?}"),
        }
    }

    #[test]
    fn from_row_rejects_an_unknown_kind() {
        let err =
            SessionEvent::from_row(1, ts(), "telepathy", json!({ "text": "hi" })).unwrap_err();
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
    }

    #[test]
    fn from_row_rejects_a_payload_that_does_not_match_its_kind() {
        let err = SessionEvent::from_row(1, ts(), "text", json!({ "message": "hi" })).unwrap_err();
        assert!(matches!(err, Error::Internal(_)), "{err:?}");

        let err = SessionEvent::from_row(1, ts(), "text", json!(["hi"])).unwrap_err();
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
    }

    #[test]
    fn from_row_rejects_an_unknown_session_state_without_panicking() {
        let err = SessionEvent::from_row(
            1,
            ts(),
            "state_change",
            json!({ "from": "running", "to": "hibernating", "reason": "stopped" }),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
    }

    #[test]
    fn the_tool_result_limit_is_the_documented_one() {
        assert_eq!(TOOL_RESULT_MAX_BYTES, 262_144);
    }
}
