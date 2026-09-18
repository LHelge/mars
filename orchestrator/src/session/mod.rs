//! The `SessionOwner` task, the launcher (including launch-for-task), the
//! idle reaper and startup recovery.
//!
//! [`registry`] is the door to all of it: the in-memory map from session id to
//! the running owner's channel, which is what makes the CLI's stdin
//! single-writer (`ARCHITECTURE.md`, "Session owner task"). The input type the
//! channel carries is [`crate::events::SessionInput`], the same one the agent
//! backend encodes (`SPEC.md`, "WebSocket: session stream").

pub mod registry;

pub use registry::{
    LaunchGuard, OwnerCommand, OwnerRx, Phase, QueuedInput, SessionRegistry, SubmitResult,
};
