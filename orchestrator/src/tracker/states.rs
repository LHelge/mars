//! Editing the board's columns: create, rename, reorder and delete a task
//! state (`SPEC.md`, "Task states"; `docs/data-model.md`, `task_states`;
//! `ARCHITECTURE.md`, "Task tracker" → "State is a queue, defined per
//! project").
//!
//! Three functions, each taking an open [`TrackerMutation`], for the reason
//! the rest of this module gives: a REST handler, an MCP tool and a test all
//! reach the same rules without HTTP, and the caller owns the commit.
//!
//! What lives here is only the *composition*, and it is the same three steps
//! every time:
//!
//! ```text
//! resolve under the lock → the repository's change → list the states again
//!   → emit one `states_changed` carrying that list
//! ```
//!
//! Everything a caller could get wrong belongs elsewhere: the name pattern to
//! [`TaskStateName::parse`], the auto-merge pairing rules to
//! [`AutoMergeInput::validate`], the placement and re-packing rules, the
//! conflict state's resolution and all the conflicts — a taken name, a second
//! `human` state, and the five deletion refusals — to [`TaskRepository`]'s
//! state helpers, which already answer the documented [`Error`]. This module
//! adds the one rule that is neither: **a change that changes nothing emits
//! nothing**, so a rename to the current name, a move to the current position
//! and an auto-merge pair equal to the current one leave the stream alone
//! (`SPEC.md`, "Tasks" states the same no-op rule for task updates).
//!
//! The list is read back through [`TaskRepository::list_states_in`] rather
//! than from the pool, because the change is still uncommitted; the event is
//! therefore the board as it will be, which is what a connected client
//! redraws from (ADR 0022, 0028).
//!
//! **Positions are the board order.** The repository keeps them packed as
//! `0..n` after every change, so a state's `position` *is* its index in the
//! list, which is what makes the no-op comparison below a comparison of
//! integers rather than of whole lists.

use uuid::Uuid;

use crate::models::{AutoMergeInput, NewTaskState, TaskState, TaskStateKind, TaskStateName};
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::tracker::TrackerMutation;

/// A new column, as a caller supplies it.
///
/// `name` is still raw: it is parsed here, so the REST body and the MCP
/// arguments map onto this without either re-deriving the pattern. A `None`
/// position appends and an explicit one shifts the states at and after it
/// (`SPEC.md`, "Task states").
#[derive(Debug, Clone)]
pub struct NewStateInput {
    /// The state's name, validated by [`TaskStateName::parse`].
    pub name: String,
    /// What the state means to the orchestrator; immutable afterwards.
    pub kind: TaskStateKind,
    /// Where it lands, or `None` to append.
    pub position: Option<i32>,
    /// `auto_merge` and its conflict state; the default is off.
    pub auto_merge: AutoMergeInput,
}

/// What a caller may change about an existing column.
///
/// `kind` is deliberately absent: a state's kind is immutable, and the
/// transport answers 400 when one is supplied, so there is nothing here to
/// carry it.
#[derive(Debug, Clone, Default)]
pub struct StateUpdate {
    /// The new name, or `None` to keep the current one.
    pub name: Option<String>,
    /// The new position, or `None` to keep the current one.
    pub position: Option<i32>,
    /// The new auto-merge pair, or `None` to keep the current one. The two
    /// fields travel together: a body that gives either replaces both
    /// (`SPEC.md`, "Task states").
    pub auto_merge: Option<AutoMergeInput>,
}

impl StateUpdate {
    /// Does this ask for anything at all?
    fn is_empty(&self) -> bool {
        self.name.is_none() && self.position.is_none() && self.auto_merge.is_none()
    }
}

/// Add a column to the board, inside an open mutation.
///
/// The name is parsed first, so an invalid one is a 400 that never reaches the
/// database. The insert decides the position and answers the two documented
/// conflicts — `state name already taken` and `project already has a human
/// state` — and a negative position is its `position must not be negative`
/// 400.
///
/// One `states_changed` is emitted: a create always changes the board.
pub async fn create_state(m: &mut TrackerMutation<'_>, input: NewStateInput) -> Result<TaskState> {
    let project_id = m.project_id();

    let name = TaskStateName::parse(&input.name)?;
    let conflict_state = input.auto_merge.validate(input.kind, name.as_str())?;
    let new_state = NewTaskState {
        id: Uuid::new_v4(),
        project_id,
        name,
        kind: input.kind,
        position: input.position,
        conflict_state,
    };

    let repository = TaskRepository::new(m.pool());
    let inserted = repository
        .insert_state(m.conn(), project_id, &new_state)
        .await?;

    announce(m, &repository).await?;

    Ok(inserted)
}

/// Rename, move and/or reconfigure the auto-merge of the column called
/// `name`, inside an open mutation.
///
/// The auto-merge pair is validated against the state's kind and the name it
/// will have, and written before the rename, so a conflict state named by the
/// state's old name is refused as the state itself. A refusal after a write
/// rolls the whole request back with the mutation.
///
/// The state is resolved under the lock ([`Error::NotFound`] when the project
/// has no column by that name), and then only what actually differs is
/// written: renaming to the current name and moving to the current position
/// are both no-ops, and a request that asks for neither change leaves the
/// board and the stream untouched. The returned flag says whether anything
/// happened, so a caller that also wants to roll back an empty request can.
///
/// A rename renames the state everywhere at once, because tasks, agent
/// profiles and the `profile_states` links all reference it by id: a task in
/// the renamed state reports the new name and a profile that served it goes on
/// serving it under the new name, with no extra work here.
///
/// A move re-packs every position to `0..n`
/// ([`TaskRepository::move_state`]), so a position past the end moves the
/// column last rather than leaving a gap; that clamp is applied to the
/// comparison too, which is what keeps "move the last column to position 99"
/// a no-op instead of an event about nothing.
pub async fn update_state(
    m: &mut TrackerMutation<'_>,
    name: &str,
    update: StateUpdate,
) -> Result<(TaskState, bool)> {
    let project_id = m.project_id();
    let repository = TaskRepository::new(m.pool());

    // Before anything is written, so a request that asks for a rename *and* an
    // impossible position is refused whole rather than half applied and rolled
    // back.
    if let Some(position) = update.position
        && position < 0
    {
        return Err(Error::BadRequest("position must not be negative".into()));
    }

    let states = repository.list_states_in(m.conn(), project_id).await?;
    let mut current = states
        .iter()
        .find(|state| state.name == name)
        .cloned()
        .ok_or(Error::NotFound)?;

    if update.is_empty() {
        return Ok((current, false));
    }

    let mut changed = false;

    // Before the rename, so a conflict state named by the state's old name
    // still resolves to this state and is refused as itself, and the model
    // compares against the name the state will have.
    if let Some(auto_merge) = &update.auto_merge {
        let own_name = update.name.as_deref().unwrap_or(&current.name);
        let conflict_state = auto_merge.validate(current.kind, own_name)?;
        let unchanged = current.auto_merge == conflict_state.is_some()
            && current.conflict_state.as_deref() == conflict_state.as_ref().map(|n| n.as_str());
        if !unchanged {
            current = repository
                .set_auto_merge(m.conn(), project_id, current.id, conflict_state.as_ref())
                .await?;
            changed = true;
        }
    }

    if let Some(new_name) = update.name.as_deref()
        && new_name != current.name
    {
        let parsed = TaskStateName::parse(new_name)?;
        current = repository
            .rename_state(m.conn(), project_id, current.id, &parsed)
            .await?;
        changed = true;
    }

    // Packed positions make `position` the index, and the repository clamps a
    // position past the end to the last one.
    let last = states.len().saturating_sub(1) as i32;
    if let Some(position) = update.position
        && position.min(last) != current.position
    {
        current = repository
            .move_state(m.conn(), project_id, current.id, position)
            .await?;
        changed = true;
    }

    if changed {
        announce(m, &repository).await?;
    }

    Ok((current, changed))
}

/// Remove the column called `name`, inside an open mutation.
///
/// [`Error::NotFound`] when the project has no such column, and otherwise the
/// repository's five documented 409s: the `human` state, the last `queue`
/// state, the last `terminal` state, a state a task is still in and a state
/// another state names as its conflict state. Each is
/// read under this mutation's lock, so a task moved into the column while the
/// request was in flight either waits for the lock or is counted.
///
/// A profile that served the column simply serves one state fewer afterwards:
/// `profile_states` references the state by id and cascades on delete.
pub async fn delete_state(m: &mut TrackerMutation<'_>, name: &str) -> Result<()> {
    let project_id = m.project_id();
    let repository = TaskRepository::new(m.pool());

    let state = repository
        .find_state_by_name_in(m.conn(), project_id, name)
        .await?
        .ok_or(Error::NotFound)?;

    repository
        .delete_state(m.conn(), project_id, state.id)
        .await?;

    announce(m, &repository).await?;

    Ok(())
}

/// Emit the one `states_changed` a board edit owes, carrying the list as it
/// now stands under the lock.
async fn announce(m: &mut TrackerMutation<'_>, repository: &TaskRepository<'_>) -> Result<()> {
    let project_id = m.project_id();
    let states = repository.list_states_in(m.conn(), project_id).await?;

    m.emit_states_changed(&states)
}
