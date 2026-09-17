//! The stored half of the project-scoped `TaskEvent` stream.
//!
//! `docs/data-model.md`, `task_events` and `SPEC.md`, "TaskEvent". This module
//! is the row and its kind strings; the delivered `TaskEvent` shape — actor,
//! `task`, `comment`, `from`/`to`, `reason`, `states` — is built in
//! `events/` and travels in [`payload`](TaskEventRow::payload).
//!
//! `seq` is allocated as `MAX(seq)+1` for the project while the project row is
//! locked, so it is not part of [`NewTaskEvent`]: only the repository may set
//! it, inside the mutation transaction (ADR 0021, 0028).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// The crate convention (`CLAUDE.md`, "Backend conventions"); see `task.rs`.
#[allow(unused_imports)]
use crate::prelude::*;

/// The `task_events.kind` strings (`SPEC.md`, "TaskEvent").
///
/// The column is deliberately `TEXT`: the set of kinds is owned by these
/// constants, and adding one must not need a migration.
pub mod kind {
    /// A task was created.
    pub const CREATED: &str = "created";
    /// A task's fields changed without a state change.
    pub const UPDATED: &str = "updated";
    /// A hand-off: the task moved to a different state.
    pub const STATE_CHANGED: &str = "state_changed";
    /// A session took the lease.
    pub const CLAIMED: &str = "claimed";
    /// The lease was cleared without a state change.
    pub const RELEASED: &str = "released";
    /// The task moved into the project's human state.
    pub const ESCALATED: &str = "escalated";
    /// The task's `blocked` flag turned on.
    pub const BLOCKED: &str = "blocked";
    /// The task's `blocked` flag turned off.
    pub const UNBLOCKED: &str = "unblocked";
    /// A comment was written.
    pub const COMMENTED: &str = "commented";
    /// A dependency edge was added.
    pub const DEPENDENCY_ADDED: &str = "dependency_added";
    /// A dependency edge was removed.
    pub const DEPENDENCY_REMOVED: &str = "dependency_removed";
    /// A task was deleted; the row keeps its original `task_id`.
    pub const DELETED: &str = "deleted";
    /// The project's state list changed; `task_id` is NULL.
    pub const STATES_CHANGED: &str = "states_changed";

    /// Every kind, in the order `SPEC.md` lists them.
    pub const ALL: [&str; 13] = [
        CREATED,
        UPDATED,
        STATE_CHANGED,
        CLAIMED,
        RELEASED,
        ESCALATED,
        BLOCKED,
        UNBLOCKED,
        COMMENTED,
        DEPENDENCY_ADDED,
        DEPENDENCY_REMOVED,
        DELETED,
        STATES_CHANGED,
    ];
}

/// A `task_events` row, column for column (`docs/data-model.md`).
///
/// `task_id` has no foreign key on purpose: it keeps the original task UUID
/// after the task is deleted, and is NULL only for project-wide events such as
/// `states_changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskEventRow {
    pub project_id: Uuid,
    pub seq: i64,
    pub ts: DateTime<Utc>,
    pub task_id: Option<Uuid>,
    pub kind: String,
    pub payload: Value,
}

/// An event about to be appended, without the `seq` the repository allocates
/// under the project lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTaskEvent {
    pub ts: DateTime<Utc>,
    pub task_id: Option<Uuid>,
    pub kind: String,
    pub payload: Value,
}

impl NewTaskEvent {
    /// An event about one task, timestamped now.
    pub fn about(task_id: Uuid, kind: &str, payload: Value) -> Self {
        Self {
            ts: Utc::now(),
            task_id: Some(task_id),
            kind: kind.to_string(),
            payload,
        }
    }

    /// A project-wide event, such as `states_changed`, timestamped now.
    pub fn project_wide(kind: &str, payload: Value) -> Self {
        Self {
            ts: Utc::now(),
            task_id: None,
            kind: kind.to_string(),
            payload,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_kinds_are_the_spec_list() {
        assert_eq!(
            kind::ALL,
            [
                "created",
                "updated",
                "state_changed",
                "claimed",
                "released",
                "escalated",
                "blocked",
                "unblocked",
                "commented",
                "dependency_added",
                "dependency_removed",
                "deleted",
                "states_changed",
            ]
        );
    }

    #[test]
    fn the_kinds_are_distinct() {
        let unique: std::collections::BTreeSet<&str> = kind::ALL.into_iter().collect();
        assert_eq!(unique.len(), kind::ALL.len());
    }

    #[test]
    fn an_event_about_a_task_carries_its_id() {
        let task_id = Uuid::new_v4();
        let event = NewTaskEvent::about(task_id, kind::CLAIMED, json!({ "actor": "session" }));
        assert_eq!(event.task_id, Some(task_id));
        assert_eq!(event.kind, "claimed");
        assert_eq!(event.payload, json!({ "actor": "session" }));
    }

    #[test]
    fn a_project_wide_event_has_no_task() {
        let event = NewTaskEvent::project_wide(kind::STATES_CHANGED, json!({ "states": [] }));
        assert!(event.task_id.is_none());
        assert_eq!(event.kind, "states_changed");
    }

    #[test]
    fn a_row_round_trips_through_serde() {
        let row = TaskEventRow {
            project_id: Uuid::new_v4(),
            seq: 7,
            ts: Utc::now(),
            task_id: None,
            kind: kind::STATES_CHANGED.to_string(),
            payload: json!({ "states": ["backlog", "ready"] }),
        };
        let encoded = serde_json::to_string(&row).unwrap();
        assert_eq!(serde_json::from_str::<TaskEventRow>(&encoded).unwrap(), row);
    }
}
