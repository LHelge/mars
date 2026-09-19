//! Where a session-created task came from (`ARCHITECTURE.md`, "Task tracker"
//! → "Discovery provenance"; `SPEC.md`, "MCP tool contracts" → `create_task`;
//! ADR 0023).
//!
//! An agent that files work it stumbled over while working on something else
//! is the common case, and the tracker records that link rather than asking
//! the agent to describe it: the new task gets a `discovered_from` edge to the
//! task the discovery came out of. The rule is one sentence per situation:
//!
//! - the session holds **exactly one** task → that task is the origin, with
//!   nothing to supply;
//! - the session holds **several** → it must say which one, because guessing
//!   would file a wrong provenance silently;
//! - the session holds **none** → there is no origin, and the task is simply
//!   created;
//! - an origin it **did** supply must be a task of this project that it
//!   currently holds, so provenance cannot be asserted about somebody else's
//!   work.
//!
//! Two things make this a module of its own rather than a branch inside
//! [`create_task`](crate::tracker::create_task). It is validation, so it runs
//! *under the project lock and before the insert* — a rejected creation leaves
//! no task and no edges — and the held-task read it decides from is only
//! authoritative inside that lock. And the decision it makes is not "which
//! task is the origin" but "which edge, if any, to record": when the origin is
//! the new task's parent, the parent link already says where the task came
//! from and a `discovered_from` edge beside it would be a second spelling of
//! the same fact. Both of those are in one function, [`resolve_origin`], whose
//! return value is exactly the edge the creation then inserts.

use uuid::Uuid;

use crate::models::TaskRef;
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::tracker::TrackerMutation;

/// What a session holding several tasks is told when it names none of them.
///
/// The message an agent reads, so it says what to do next rather than what
/// went wrong (`SPEC.md`, `create_task`; the MCP layer answers it as
/// `invalid_argument`).
pub const AMBIGUOUS_ORIGIN: &str =
    "this session holds several tasks; pass discovered_from to name the originating task";

/// What naming an origin the caller does not hold is told.
pub const ORIGIN_NOT_HELD: &str = "discovered_from must be a task this session currently holds";

/// The `discovered_from` edge a session's creation owes, or `None`.
///
/// Called under the project lock, before the task row exists, with the parent
/// the creation asked for. `None` means "record no provenance edge", which
/// covers both of the cases where there is nothing to record: the session
/// holds nothing and named nothing, or the origin is the parent and the parent
/// link already records it.
///
/// Every failure here is a failure of the whole creation: the caller's
/// mutation rolls back, so a rejected request creates neither a task nor an
/// edge. An explicit `discovered_from` naming nothing in this project is
/// [`Error::NotFound`] — it is a reference to a task, answered as a missing
/// task is — while naming a task the session does not hold is
/// [`Error::BadRequest`] with [`ORIGIN_NOT_HELD`]: the task exists, the claim
/// about it does not.
///
/// The origin task itself is never read for modification and never modified:
/// provenance is a fact about the *new* task.
pub async fn resolve_origin(
    m: &mut TrackerMutation<'_>,
    session_id: Uuid,
    explicit: Option<TaskRef>,
    parent: Option<Uuid>,
) -> Result<Option<Uuid>> {
    let project_id = m.project_id();
    let repository = TaskRepository::new(m.pool());

    // Read under the lock: a lease released between a pool read and this
    // mutation would otherwise be inferred from (`rows.rs`,
    // `list_by_lease_holder_in`).
    let held = repository
        .list_by_lease_holder_in(m.conn(), project_id, session_id)
        .await?;

    let origin = match explicit {
        Some(reference) => {
            let named = repository
                .find_task_for_update(m.conn(), project_id, reference)
                .await?
                .ok_or(Error::NotFound)?;

            if !held.iter().any(|task| task.id == named.id) {
                return Err(Error::BadRequest(ORIGIN_NOT_HELD.into()));
            }

            named.id
        }
        None => match held.as_slice() {
            [] => return Ok(None),
            [only] => only.id,
            _ => return Err(Error::BadRequest(AMBIGUOUS_ORIGIN.into())),
        },
    };

    // The parent link is provenance already; a second edge saying the same
    // thing is not recorded (ADR 0023). A `blocks` edge to the same origin is
    // a different statement and does coexist — which is why only the parent is
    // compared here.
    if parent == Some(origin) {
        debug!(
            project_id = %project_id,
            session_id = %session_id,
            origin_task_id = %origin,
            "discovery provenance is the parent link",
        );

        return Ok(None);
    }

    Ok(Some(origin))
}
