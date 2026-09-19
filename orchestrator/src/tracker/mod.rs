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

pub mod dto;

pub use dto::{CommentDto, DependencyRef, HandoffDto, TaskDetailDto, TaskDto, TaskSessionLinkDto};
