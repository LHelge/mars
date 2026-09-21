//! The `AgentEvent` and `TaskEvent` types and the notify fan-out.
//!
//! `agent_event` is the session transcript's one schema across backends
//! (`SPEC.md`, "AgentEvent"), `input` is what a client may send back
//! (`SPEC.md`, "WebSocket: session stream"). `task_event` is the tracker's
//! stream (`SPEC.md`, "TaskEvent"): the delivered shape and its round trip
//! with the stored `task_events` row.
//!
//! `fanout` is the in-process mirror of the Postgres notifications
//! (`ARCHITECTURE.md`, "Event delivery"; ADR 0005): the broadcast channels
//! `AppState` hands to every WebSocket and SSE subscriber. `listener` is what
//! fills it: the process's single Postgres `LISTEN` connection.

pub mod agent_event;
pub mod fanout;
pub mod input;
pub mod listener;
pub mod task_event;

pub use agent_event::{
    AgentEvent, AgentEventBody, GitOp, McpServerStatus, SessionEvent, StopSignal,
    TOOL_RESULT_MAX_BYTES,
};
pub use fanout::{AnyNotice, Channel, EventFanout, Notice, PayloadError, parse_payload};
pub use input::{
    CLIENT_ID_TOO_LONG, EMPTY_TEXT, LONG_TEXT, MAX_CLIENT_ID_BYTES, MAX_TEXT_BYTES, SessionInput,
    validate_client_id,
};
pub use listener::spawn_listener;
pub use task_event::{TaskActor, TaskEvent, TaskEventKind, TaskEventPayload, TaskEventRowParts};
