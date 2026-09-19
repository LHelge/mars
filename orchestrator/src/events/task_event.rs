//! The delivered half of the project-scoped task stream.
//!
//! `SPEC.md`, "TaskEvent" is the wire contract, field for field: [`TaskEvent`]
//! *is* what an SSE `data:` frame carries, so renaming a field here changes
//! the API. The stored half — the `task_events` row and its kind strings —
//! is [`TaskEventRow`] in `models/task_event.rs`; this module is the
//! translation between the two.
//!
//! The split follows the columns. `seq`, `ts`, `task_id` and `kind` are
//! columns, because the stream is paged, replayed and scoped by them; the
//! rest of the event — the actor, the full task, the comment, the state names,
//! the reason, the project's state list — is [`TaskEventPayload`], stored as
//! one `JSONB` value and handed back unchanged. [`TaskEvent::from_row`] and
//! [`TaskEvent::to_row_parts`] are the round trip.
//!
//! Optional sections are *omitted* rather than sent as `null`
//! (`#[serde(skip_serializing_if = "Option::is_none")]`): `SPEC.md` writes
//! them `task?`, `comment?`, `from?`, `to?`, `reason?`, `states?`, so a
//! `deleted` event has no `task` key at all. The task DTO's own nullable
//! fields are the opposite — they stay as explicit `null` (see
//! [`TaskDto`](crate::tracker::TaskDto)).
//!
//! An event keeps its identity after the task is gone: `deleted` carries the
//! original UUID in `task_id` and omits `task`, and earlier events keep the
//! payloads they were written with (ADR 0022).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::models::task_event::kind;
use crate::models::{NewTaskEvent, TaskEventRow, TaskState};
use crate::prelude::*;
use crate::tracker::{CommentDto, TaskDto};

/// Who caused a task event (`SPEC.md`, "TaskEvent").
///
/// Serialised as an internally tagged union on `kind`:
/// `{"kind":"user","user_id":"..."}`, `{"kind":"session","session_id":"..."}`
/// or the bare `{"kind":"system"}` — the orchestrator acting on its own, as
/// the stuck-task reaper and automatic parent closure do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskActor {
    /// A signed-in user, through REST.
    User {
        /// The acting user.
        user_id: Uuid,
    },
    /// An agent session, through MCP.
    Session {
        /// The acting session.
        session_id: Uuid,
    },
    /// The orchestrator itself: reapers, parent closure, recovery.
    System,
}

/// What happened (`SPEC.md`, "TaskEvent").
///
/// The variants serialise to — and [`TaskEventKind::as_str`] returns — exactly
/// the strings stored in `task_events.kind`, which are the constants in
/// [`crate::models::task_event::kind`]. The two lists are asserted equal in
/// this module's tests, so the column and the wire can never drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventKind {
    /// A task was created.
    Created,
    /// A task's fields changed without a state change.
    Updated,
    /// A hand-off: the task moved to a different state.
    StateChanged,
    /// A session took the lease.
    Claimed,
    /// The lease was cleared without a state change.
    Released,
    /// The task moved into the project's human state.
    Escalated,
    /// The task's `blocked` flag turned on.
    Blocked,
    /// The task's `blocked` flag turned off.
    Unblocked,
    /// A comment was written.
    Commented,
    /// A dependency edge was added.
    DependencyAdded,
    /// A dependency edge was removed.
    DependencyRemoved,
    /// A task was deleted; the event keeps its original `task_id`.
    Deleted,
    /// The project's state list changed; `task_id` is NULL.
    StatesChanged,
}

impl TaskEventKind {
    /// Every kind, in the order `SPEC.md` lists them.
    pub const ALL: [Self; 13] = [
        Self::Created,
        Self::Updated,
        Self::StateChanged,
        Self::Claimed,
        Self::Released,
        Self::Escalated,
        Self::Blocked,
        Self::Unblocked,
        Self::Commented,
        Self::DependencyAdded,
        Self::DependencyRemoved,
        Self::Deleted,
        Self::StatesChanged,
    ];

    /// The string stored in `task_events.kind` and sent on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => kind::CREATED,
            Self::Updated => kind::UPDATED,
            Self::StateChanged => kind::STATE_CHANGED,
            Self::Claimed => kind::CLAIMED,
            Self::Released => kind::RELEASED,
            Self::Escalated => kind::ESCALATED,
            Self::Blocked => kind::BLOCKED,
            Self::Unblocked => kind::UNBLOCKED,
            Self::Commented => kind::COMMENTED,
            Self::DependencyAdded => kind::DEPENDENCY_ADDED,
            Self::DependencyRemoved => kind::DEPENDENCY_REMOVED,
            Self::Deleted => kind::DELETED,
            Self::StatesChanged => kind::STATES_CHANGED,
        }
    }

    /// The kind a stored `task_events.kind` names, or `None` if the column
    /// holds a string this build does not know.
    ///
    /// The column is `TEXT` precisely so a new kind needs no migration, which
    /// means an older binary can meet a newer row; the caller decides what to
    /// do about it rather than being handed a wrong variant.
    pub fn from_kind(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == raw)
    }
}

impl std::fmt::Display for TaskEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Everything about an event that is not one of its columns.
///
/// This is the shape stored in `task_events.payload`. It is deliberately the
/// same field names, in the same order, as the matching part of
/// [`TaskEvent`]: the delivered event is the row's four columns with this
/// value spread over it, and nothing is translated on the way out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskEventPayload {
    /// Who caused the change.
    pub actor: TaskActor,
    /// The full task after the change; absent on `deleted` and
    /// `states_changed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskDto>,
    /// The comment, on `commented`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<CommentDto>,
    /// The state moved out of, on `state_changed` and `escalated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The state moved into, on `state_changed` and `escalated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Why, on `released` and `escalated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The project's full state list after the change, on `states_changed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub states: Option<Vec<TaskState>>,
}

impl TaskEventPayload {
    /// A payload carrying nothing but its actor.
    ///
    /// The starting point for the kinds that describe themselves — `claimed`,
    /// `blocked`, `deleted` — which the caller then fills in field by field.
    pub fn new(actor: TaskActor) -> Self {
        Self {
            actor,
            task: None,
            comment: None,
            from: None,
            to: None,
            reason: None,
            states: None,
        }
    }
}

/// A [`TaskEvent`] taken apart into the shape a `task_events` row is written
/// from: `seq`, `ts`, `task_id`, `kind`, and everything else as one `JSONB`
/// value.
pub type TaskEventRowParts = (i64, DateTime<Utc>, Option<Uuid>, String, Value);

/// One event on a project's task stream (`SPEC.md`, "TaskEvent").
///
/// Built from a row with [`TaskEvent::from_row`] on the way out of the
/// database, and taken apart with [`TaskEvent::to_row_parts`] on the way in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskEvent {
    /// Per-project, monotonic, allocated under the project lock.
    pub seq: i64,
    /// When the change committed.
    pub ts: DateTime<Utc>,
    /// The original task UUID, retained after deletion; `null` on
    /// `states_changed`.
    pub task_id: Option<Uuid>,
    /// Who caused the change.
    pub actor: TaskActor,
    /// What happened.
    pub kind: TaskEventKind,
    /// The full task after the change; absent on `deleted` and
    /// `states_changed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskDto>,
    /// The comment, on `commented`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<CommentDto>,
    /// The state moved out of, on `state_changed` and `escalated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The state moved into, on `state_changed` and `escalated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Why, on `released` and `escalated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The project's full state list after the change, on `states_changed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub states: Option<Vec<TaskState>>,
}

impl TaskEvent {
    /// Assemble an event from its columns and its payload.
    pub fn new(
        seq: i64,
        ts: DateTime<Utc>,
        task_id: Option<Uuid>,
        kind: TaskEventKind,
        payload: TaskEventPayload,
    ) -> Self {
        Self {
            seq,
            ts,
            task_id,
            actor: payload.actor,
            kind,
            task: payload.task,
            comment: payload.comment,
            from: payload.from,
            to: payload.to,
            reason: payload.reason,
            states: payload.states,
        }
    }

    /// The payload half of this event, as it is stored.
    pub fn payload(&self) -> TaskEventPayload {
        TaskEventPayload {
            actor: self.actor,
            task: self.task.clone(),
            comment: self.comment.clone(),
            from: self.from.clone(),
            to: self.to.clone(),
            reason: self.reason.clone(),
            states: self.states.clone(),
        }
    }

    /// Rebuild the delivered event from a stored row.
    ///
    /// A `kind` this build does not know, or a payload that is not a
    /// [`TaskEventPayload`], is a database the binary disagrees with rather
    /// than anything a caller did: it logs and answers [`Error::Internal`].
    pub fn from_row(row: TaskEventRow) -> Result<Self> {
        let Some(kind) = TaskEventKind::from_kind(&row.kind) else {
            error!(
                project_id = %row.project_id,
                seq = row.seq,
                kind = %row.kind,
                "unknown task event kind in task_events",
            );
            return Err(Error::Internal("unknown task event kind".to_string()));
        };

        let payload: TaskEventPayload = serde_json::from_value(row.payload).map_err(|err| {
            error!(
                project_id = %row.project_id,
                seq = row.seq,
                error = %err,
                "task event payload does not match the documented shape",
            );
            Error::Internal("malformed task event payload".to_string())
        })?;

        Ok(Self::new(row.seq, row.ts, row.task_id, kind, payload))
    }

    /// Take the event apart into the row it is stored as: the four columns,
    /// then the payload.
    ///
    /// `project_id` is not among them: the row's project is the stream the
    /// event is appended to, and the repository knows it.
    pub fn to_row_parts(&self) -> Result<TaskEventRowParts> {
        let payload = serde_json::to_value(self.payload()).map_err(|err| {
            error!(error = %err, "a task event payload failed to serialise");
            Error::Internal("malformed task event payload".to_string())
        })?;

        Ok((
            self.seq,
            self.ts,
            self.task_id,
            self.kind.as_str().to_string(),
            payload,
        ))
    }

    /// The same event as an append, without the `seq` the repository
    /// allocates under the project lock.
    pub fn to_new_event(&self) -> Result<NewTaskEvent> {
        let (_, ts, task_id, kind, payload) = self.to_row_parts()?;
        Ok(NewTaskEvent {
            ts,
            task_id,
            kind,
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;
    use crate::models::TaskStateKind;

    fn actor() -> TaskActor {
        TaskActor::System
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0).single().expect("a timestamp")
    }

    #[test]
    fn the_kinds_are_the_stored_strings() {
        let wire: Vec<&str> = TaskEventKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(wire, kind::ALL.to_vec());
    }

    #[test]
    fn a_kind_serialises_as_its_stored_string() {
        for k in TaskEventKind::ALL {
            assert_eq!(serde_json::to_value(k).unwrap(), json!(k.as_str()));
            assert_eq!(
                serde_json::from_value::<TaskEventKind>(json!(k.as_str())).unwrap(),
                k
            );
            assert_eq!(k.to_string(), k.as_str());
            assert_eq!(TaskEventKind::from_kind(k.as_str()), Some(k));
        }
        assert_eq!(TaskEventKind::from_kind("reticulated"), None);
        assert!(serde_json::from_value::<TaskEventKind>(json!("Created")).is_err());
    }

    #[test]
    fn a_system_actor_is_the_bare_tag() {
        assert_eq!(
            serde_json::to_value(TaskActor::System).unwrap(),
            json!({ "kind": "system" })
        );
        assert_eq!(
            serde_json::from_value::<TaskActor>(json!({ "kind": "system" })).unwrap(),
            TaskActor::System
        );
    }

    #[test]
    fn a_user_and_a_session_actor_carry_their_id() {
        let user_id = Uuid::new_v4();
        assert_eq!(
            serde_json::to_value(TaskActor::User { user_id }).unwrap(),
            json!({ "kind": "user", "user_id": user_id })
        );
        let session_id = Uuid::new_v4();
        assert_eq!(
            serde_json::to_value(TaskActor::Session { session_id }).unwrap(),
            json!({ "kind": "session", "session_id": session_id })
        );
        assert_eq!(
            serde_json::from_value::<TaskActor>(
                json!({ "kind": "session", "session_id": session_id })
            )
            .unwrap(),
            TaskActor::Session { session_id }
        );
    }

    #[test]
    fn a_deleted_event_omits_the_task() {
        let task_id = Uuid::new_v4();
        let event = TaskEvent::new(
            4,
            at(1_700_000_000),
            Some(task_id),
            TaskEventKind::Deleted,
            TaskEventPayload::new(actor()),
        );
        let encoded = serde_json::to_value(&event).unwrap();

        assert_eq!(
            encoded,
            json!({
                "seq": 4,
                "ts": "2023-11-14T22:13:20Z",
                "task_id": task_id,
                "actor": { "kind": "system" },
                "kind": "deleted",
            })
        );
        let object = encoded.as_object().unwrap();
        for absent in ["task", "comment", "from", "to", "reason", "states"] {
            assert!(!object.contains_key(absent), "{absent} was serialised");
        }
    }

    #[test]
    fn a_states_changed_event_has_a_null_task_id_and_a_states_array() {
        let project_id = Uuid::new_v4();
        let state = TaskState {
            id: Uuid::new_v4(),
            project_id,
            name: "backlog".to_string(),
            kind: TaskStateKind::Queue,
            position: 0,
            created_at: at(1_700_000_000),
        };
        let mut payload = TaskEventPayload::new(actor());
        payload.states = Some(vec![state.clone()]);

        let event = TaskEvent::new(
            9,
            at(1_700_000_100),
            None,
            TaskEventKind::StatesChanged,
            payload,
        );
        let encoded = serde_json::to_value(&event).unwrap();

        assert_eq!(encoded["task_id"], json!(null));
        assert_eq!(encoded["kind"], json!("states_changed"));
        assert_eq!(encoded["states"], json!([state]));
        assert!(!encoded.as_object().unwrap().contains_key("task"));
    }

    #[test]
    fn an_event_round_trips_through_its_row() {
        let project_id = Uuid::new_v4();
        let task_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let mut payload = TaskEventPayload::new(TaskActor::User { user_id });
        payload.from = Some("ready".to_string());
        payload.to = Some("review".to_string());
        payload.reason = Some("handed off".to_string());

        let event = TaskEvent::new(
            12,
            at(1_700_000_200),
            Some(task_id),
            TaskEventKind::StateChanged,
            payload,
        );

        let (seq, ts, row_task_id, kind, payload) = event.to_row_parts().unwrap();
        assert_eq!(seq, 12);
        assert_eq!(kind, "state_changed");
        assert_eq!(
            payload["actor"],
            json!({ "kind": "user", "user_id": user_id })
        );

        let row = TaskEventRow {
            project_id,
            seq,
            ts,
            task_id: row_task_id,
            kind,
            payload,
        };
        assert_eq!(TaskEvent::from_row(row).unwrap(), event);
    }

    #[test]
    fn an_append_keeps_everything_but_the_sequence() {
        let task_id = Uuid::new_v4();
        let event = TaskEvent::new(
            3,
            at(1_700_000_300),
            Some(task_id),
            TaskEventKind::Claimed,
            TaskEventPayload::new(actor()),
        );

        let new = event.to_new_event().unwrap();
        assert_eq!(new.ts, event.ts);
        assert_eq!(new.task_id, Some(task_id));
        assert_eq!(new.kind, "claimed");
        assert_eq!(new.payload, json!({ "actor": { "kind": "system" } }));
    }

    #[test]
    fn an_unknown_kind_is_an_internal_error() {
        let row = TaskEventRow {
            project_id: Uuid::new_v4(),
            seq: 1,
            ts: at(1_700_000_400),
            task_id: None,
            kind: "reticulated".to_string(),
            payload: json!({ "actor": { "kind": "system" } }),
        };
        assert!(matches!(TaskEvent::from_row(row), Err(Error::Internal(_))));
    }

    #[test]
    fn a_payload_that_is_not_the_documented_shape_is_an_internal_error() {
        let row = TaskEventRow {
            project_id: Uuid::new_v4(),
            seq: 1,
            ts: at(1_700_000_500),
            task_id: None,
            kind: kind::STATES_CHANGED.to_string(),
            payload: json!({ "actor": "system" }),
        };
        assert!(matches!(TaskEvent::from_row(row), Err(Error::Internal(_))));
    }
}
