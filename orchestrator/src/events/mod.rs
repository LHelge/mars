//! The `AgentEvent` and `TaskEvent` types and the notify fan-out.
//!
//! `agent_event` is the session transcript's one schema across backends
//! (`SPEC.md`, "AgentEvent"), `input` is what a client may send back
//! (`SPEC.md`, "WebSocket: session stream"). `TaskEvent` joins them in
//! `task_event.rs` with the tracker epic.

pub mod agent_event;
pub mod input;

pub use agent_event::{
    AgentEvent, AgentEventBody, GitOp, McpServerStatus, SessionEvent, StopSignal,
    TOOL_RESULT_MAX_BYTES,
};
pub use input::{EMPTY_TEXT, LONG_TEXT, MAX_TEXT_BYTES, SessionInput};
