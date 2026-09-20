//! The tool contract as data: names, descriptions and input/output shapes.
//!
//! Everything in here is pure: no state, no database, no git. The dispatcher
//! and the handlers of the following tasks read [`ToolName::ALL`] to list the
//! tools, [`ToolName::description`] for the text `SPEC.md` fixes, and the
//! structs in [`types`] to parse arguments and shape answers. Keeping the
//! contract in one place is what lets `tests/mcp_descriptions.rs` compare it
//! with the document byte for byte.

pub mod descriptions;
pub mod names;
pub mod types;

pub use names::ToolName;
#[allow(
    unused_imports,
    reason = "the dispatcher and the tool handlers of the following tasks use these"
)]
pub use types::{
    BranchesOutput, CommentInput, CommentOutput, CommitOutput, CreateTaskInput, EmptyInput,
    McpMergeInput, NeedsHumanInput, PushInput, PushOutput, ReadyInput, ReadyOutput, RebaseInput,
    ReleaseInput, TASK_ARG_MESSAGE, TaskArg, TaskDetailOutput, TaskOnlyInput, TaskOutput,
    UpdateInput, non_empty, validate_handoff, validate_limit, validate_priority,
};
