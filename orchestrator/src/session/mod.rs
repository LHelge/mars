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
//!
//! [`launcher`] is the sequence around those: the mirror fetch, the session
//! clone, the secrets resolution, the container specification, the engine calls
//! and the owner spawn, with every failure routed to `failed` and
//! `sessions.error` (`ARCHITECTURE.md`, "Launch sequence").
//!
//! [`owner`] is the loop itself: the one reader of `log/stream.jsonl` and the
//! one writer of the CLI's stdin, committing each complete native line's
//! events, offset and counters together (`ARCHITECTURE.md`, "Session owner
//! task", "Durability and recovery").
//!
//! [`service`] is the other half of the lifecycle: what a *user* asks of a
//! session — input, stop, end, retry, sync and delete — in the one place the
//! REST routes, the WebSocket handler and the cron jobs share, so that all
//! three apply the same rules (`ARCHITECTURE.md`, "Session lifecycle", "Stop
//! semantics"; `SPEC.md`, "Sessions").
//!
//! [`recovery`] is what runs once at startup, before anything serves: it adopts
//! the containers a restart left running, parks the sessions whose container is
//! gone and fails the ones the restart caught mid-creation
//! (`ARCHITECTURE.md`, "Restart procedure").

pub mod launcher;
pub mod owner;
pub mod prepare;
pub mod recovery;
pub mod registry;
pub mod service;
pub mod token;

pub use launcher::{
    BOTH_CREDENTIALS_ERROR, FRESH_FETCH_MAX_AGE, LAUNCH_FAILED_REASON, LAUNCHED_REASON, LaunchMode,
    Launcher,
};
pub use owner::{
    COST_ACCOUNTING, CostAccounting, MARS_MCP_SERVER, OwnerContext, ResultSummary, SessionOwner,
    TAIL_POLL_INTERVAL,
};
pub use prepare::{SessionDirs, initial_token, rotate_token, write_mcp_json};
pub use recovery::{CREATING_REASON, MISSING_CONTAINER_REASON, RecoveryReport, recover};
pub use registry::{
    LaunchGuard, OwnerCommand, OwnerRx, Phase, QueuedInput, SessionRegistry, SubmitResult,
};
pub use service::SessionService;
pub use token::{MCP_TOKEN_CHARS, McpToken, hash_mcp_token};
