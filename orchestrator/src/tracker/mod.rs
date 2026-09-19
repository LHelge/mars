//! The task-tracker domain service (`ARCHITECTURE.md`, "Orchestrator
//! internals" and "Task tracker").
//!
//! Everything above the repository and below the transport lives here: the
//! mutation context, state changes, leases, graph rules and escalation, so
//! that a REST handler, an MCP tool and a background job all reach the same
//! rules rather than each re-deriving them.
//!
//! This module starts with `dto`: the API-facing shapes every one of those
//! callers produces (`SPEC.md`, "Tasks"). They are read-only projections of
//! the row types — the loaders that assemble them are
//! `TaskRepository::load_task_dto` and its siblings in
//! `repositories/tasks/dto.rs` — and they are also what a `TaskEvent` payload
//! carries, so the task in an event and the task in a REST response are
//! literally the same struct.
//!
//! **Lock order.** git lock (outside) → project row (the mutation's own
//! transaction) → session rows → task rows; never call engine, email, git or
//! model code while a [`TrackerMutation`] is open (`ARCHITECTURE.md`, "Task
//! tracker";
//! `docs/data-model.md`, "Tracker mutation transactions"). Git preparation
//! finishes before the tracker transaction opens, and the escalation emails a
//! mutation makes due are sent after it commits, from
//! [`MutationOutcome::escalations`].

pub mod comments;
pub mod dependencies;
pub mod dto;
pub mod escalation;
pub mod graph;
pub mod leases;
pub mod mutation;
pub mod state;
pub mod tasks;

pub use comments::{CommentAuthor, add_comment};
pub use dependencies::{add_dependency, remove_dependency, resolve_dependency};
pub use dto::{
    CommentDto, DependencyRef, HandoffDto, TaskDetailDto, TaskDto, TaskSessionLinkDto, TaskSummary,
};
pub use escalation::Escalation;
pub use graph::{BlockedFlip, DeletionCapture};
pub use leases::{claim_for_launch, claim_for_profile, ready_summaries, release_by_user};
pub use mutation::{Locked, MutationOutcome, TrackerMutation};
pub use state::{StateChangeOptions, StateChangeResult, StateEventKind};
pub use tasks::{CreateTaskInput, CreatedBy, create_task};
