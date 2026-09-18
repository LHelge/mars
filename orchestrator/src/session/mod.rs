//! The `SessionOwner` task, the launcher (including launch-for-task), the
//! idle reaper and startup recovery.
//!
//! [`registry`] is the door to all of it: the in-memory map from session id to
//! the running owner's channel, which is what makes the CLI's stdin
//! single-writer (`ARCHITECTURE.md`, "Session owner task"). The input type the
//! channel carries is [`crate::events::SessionInput`], the same one the agent
//! backend encodes (`SPEC.md`, "WebSocket: session stream").
//!
//! [`prepare`] is what a launch does before it asks the engine for anything:
//! the session's directories under `DATA_DIR/sessions/<sid>/`, a fresh MCP
//! bearer token from [`token`] and the `mcp.json` that carries it, written
//! atomically after the matching hash has committed (ADR 0029).

pub mod prepare;
pub mod registry;
pub mod token;

pub use prepare::{SessionDirs, initial_token, rotate_token, write_mcp_json};
pub use registry::{
    LaunchGuard, OwnerCommand, OwnerRx, Phase, QueuedInput, SessionRegistry, SubmitResult,
};
pub use token::{MCP_TOKEN_CHARS, McpToken, hash_mcp_token};
