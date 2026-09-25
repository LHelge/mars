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
//! transaction) → task rows → session rows; never call engine, email, git or
//! model code while a [`TrackerMutation`] is open (`ARCHITECTURE.md`, "Task
//! tracker" → "Lock order";
//! `docs/data-model.md`, "Tracker mutation transactions"). The session rows
//! come last because a mutation reaches them through the foreign keys of what
//! it writes — a `task_sessions` link, a hand-off's `source_session_id` — and
//! a transaction that takes a session row first and a tracker row second is
//! the deadlock that order exists to prevent, which is why session deletion
//! locks the project row too ([`repositories::SessionRepository::delete`]).
//! [`retry_on_serialization_failure`] is the backstop for the cycles no single
//! project lock can order. Git preparation
//! finishes before the tracker transaction opens, and the escalation emails a
//! mutation makes due are sent after it commits, from
//! [`MutationOutcome::escalations`] — which is what [`commit_and_notify`] does
//! in one call.

pub mod comments;
pub mod dependencies;
pub mod drop_handoff;
pub mod dto;
pub mod escalation;
pub mod graph;
pub mod handoffs;
pub mod hooks;
pub mod leases;
pub mod mutation;
pub mod provenance;
pub mod retry;
pub mod rounds;
pub mod state;
pub mod states;
pub mod tasks;

use crate::prelude::*;

pub use comments::{CommentAuthor, add_comment};
pub use dependencies::{add_dependency, remove_dependency, resolve_dependency};
pub use drop_handoff::{NO_CURRENT_HANDOFF, drop_handoff};
pub use dto::{
    CommentDto, DependencyRef, HandoffDto, TaskDetailDto, TaskDto, TaskSessionLinkDto, TaskSummary,
};
pub use escalation::Escalation;
pub use graph::{BlockedFlip, DeletionCapture};
// `prepare` and `discard_prepared` stay behind `handoffs::`: a bare `prepare`
// at the tracker root would say nothing about what it prepares.
pub use handoffs::{HandoffService, PreparedHandoff, ReviewCarry};
pub use hooks::{on_session_dead, session_ended_hook};
pub use leases::{
    NOT_SERVED, ReleaseReason, claim_for_launch, claim_for_profile, needs_human, ready_summaries,
    release_by_agent, release_by_user, release_leases_for_session,
};
pub use mutation::{Locked, MutationOutcome, TrackerMutation};
pub use provenance::resolve_origin;
pub use retry::retry_on_serialization_failure;
pub use rounds::{RoundLimitRedirect, send_back, send_back_redirect};
pub use state::{StateChangeOptions, StateChangeResult, StateEventKind};
pub use states::{NewStateInput, StateUpdate, create_state, delete_state, update_state};
pub use tasks::{CreateTaskInput, CreatedBy, create_task};
pub use tasks::{UpdateOutcome, UpdateTaskInput, delete_task, update_task};

/// Commit a mutation and send the escalation emails it made due.
///
/// The pairing every transport uses: a REST handler, an MCP tool and a
/// background job all finish a tracker mutation here, so "email goes out after
/// the commit, and only after a commit" is one line at each call site instead
/// of a rule to remember (`ARCHITECTURE.md`, "Task tracker" → "One mutation at
/// a time per project" and "Notification").
///
/// A mutation that owes no email — which is nearly all of them — pays a loop
/// over an empty vector and nothing else. The outcome is returned whole, so a
/// caller that wants the sequences still has them; the escalations stay in it
/// as the record of what was sent.
///
/// Sending cannot fail this: [`escalation::notify`] returns `()` and logs, so
/// the only error this can answer with is the commit's own. A caller that
/// rolled back calls [`TrackerMutation::no_change`] instead, and sends nothing.
pub async fn commit_and_notify(
    m: TrackerMutation<'_>,
    state: &AppState,
) -> Result<MutationOutcome> {
    let outcome = m.commit().await?;

    escalation::notify(state, outcome.escalations.clone()).await;

    Ok(outcome)
}
