//! Publishing a code hand-off — the extension point, not yet the thing.
//!
//! `SPEC.md`, "Code hand-offs and review" gives `PUT
//! /projects/{pid}/tasks/{id}` a `handoff` field: a revision publishes a
//! commit under an internal immutable ref and moves the task, a forward
//! re-uses the current hand-off and optionally records a review decision. All
//! of that — the git work outside the project lock, the `task_handoffs` row,
//! the comment, the review status — belongs to the Code hand-offs epic.
//!
//! Until that epic lands, [`publish`] is the one place the rest of the tracker
//! reaches for it, and it refuses with a 400 saying so. The route already
//! enforces the two input rules that are the *caller's* to get right
//! (`SPEC.md`: a hand-off "requires a different target `state` in the same
//! update (400 otherwise) and a non-empty `comment`"), so a client that shapes
//! its request correctly gets the "not available yet" answer and a client that
//! does not is told what is wrong with it either way.
//!
//! **What replaces this**: the hand-off epic gives `publish` the typed
//! `HandoffInput`, does its git preparation *before* the tracker mutation
//! opens (the lock order is git, then the project row) and then writes the
//! hand-off, the comment and the state move inside the one mutation this
//! function is handed.

use crate::models::Task;
use crate::prelude::*;
use crate::tracker::TrackerMutation;

/// What a `handoff` is answered with until the Code hand-offs epic lands.
pub const HANDOFFS_UNAVAILABLE: &str = "code hand-offs are not available yet";

/// Publish a hand-off for this task inside the caller's mutation.
///
/// The stub: every call is [`Error::BadRequest`] with
/// [`HANDOFFS_UNAVAILABLE`], and nothing is written — the caller's mutation
/// rolls back with the refusal, so a rejected hand-off leaves neither a task
/// change nor an event (ADR 0021).
///
/// The signature is deliberately the shape the real one needs: the open
/// mutation, the task as it is under the lock, and the caller's comment.
/// Whatever the hand-off epic adds to it — the typed input, the prepared
/// commit — it adds beside these rather than instead of them.
#[allow(unused_variables)]
pub async fn publish(m: &mut TrackerMutation<'_>, task: &Task, comment: &str) -> Result<()> {
    Err(Error::BadRequest(HANDOFFS_UNAVAILABLE.into()))
}
