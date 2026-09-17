//! Task states: the per-project board columns and what each one means to the
//! orchestrator.
//!
//! `docs/data-model.md`, `task_states` (including the default set created with
//! every project) and `SPEC.md`, "Task states". The states themselves are
//! rows; only their [`TaskStateKind`] is an enum.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::task::{TaskError, TaskResult};
// The crate convention (`CLAUDE.md`, "Backend conventions"); see `task.rs`.
#[allow(unused_imports)]
use crate::prelude::*;

/// Longest accepted state name (and label), in characters.
pub const MAX_STATE_NAME_CHARS: usize = 32;

/// What a project-defined state means to the orchestrator.
///
/// Immutable after creation: there is no setter here and the repository
/// refuses updates (`SPEC.md`, "Task states").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "task_state_kind", rename_all = "snake_case")]
pub enum TaskStateKind {
    /// Agents claim from it.
    Queue,
    /// Agents never claim from it; escalations land here. At most one per
    /// project.
    Human,
    /// Closes the task and satisfies dependencies.
    Terminal,
}

/// Does `raw` match the state-name pattern, `[a-z0-9][a-z0-9_-]*` at 1–32
/// characters?
///
/// One function for both rules: task state names and task labels share the
/// pattern (`docs/data-model.md`, `task_states` and `tasks`). Hand-written on
/// purpose — a regular expression crate would be the only user of itself here.
pub fn is_state_name(raw: &str) -> bool {
    let mut characters = raw.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    if raw.chars().count() > MAX_STATE_NAME_CHARS {
        return false;
    }
    characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// A validated task state name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct TaskStateName(String);

impl TaskStateName {
    /// Accept `raw` when it is 1–32 characters matching
    /// `[a-z0-9][a-z0-9_-]*`.
    pub fn parse(raw: &str) -> TaskResult<Self> {
        if is_state_name(raw) {
            Ok(Self(raw.to_string()))
        } else {
            Err(TaskError::InvalidStateName)
        }
    }

    /// The state name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<TaskStateName> for String {
    fn from(name: TaskStateName) -> Self {
        name.0
    }
}

impl std::fmt::Display for TaskStateName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A `task_states` row, column for column (`docs/data-model.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskState {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub kind: TaskStateKind,
    pub position: i32,
    pub created_at: DateTime<Utc>,
}

/// The caller-supplied half of a new task state.
///
/// A `None` position appends; the repository shifts the states at and after an
/// explicit one (`SPEC.md`, "Task states").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTaskState {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: TaskStateName,
    pub kind: TaskStateKind,
    pub position: Option<i32>,
}

impl NewTaskState {
    /// A state with a fresh id, validating the name.
    pub fn new(project_id: Uuid, name: &str, kind: TaskStateKind) -> TaskResult<Self> {
        Ok(Self {
            id: Uuid::new_v4(),
            project_id,
            name: TaskStateName::parse(name)?,
            kind,
            position: None,
        })
    }
}

/// The states every project is created with, in board order
/// (`docs/data-model.md`, `task_states`).
///
/// `backlog` is the lowest-positioned queue state and therefore the default
/// state of a new task; `needs_human` is the project's single human state.
pub const DEFAULT_TASK_STATES: [(&str, TaskStateKind, i32); 7] = [
    ("backlog", TaskStateKind::Queue, 0),
    ("ready", TaskStateKind::Queue, 1),
    ("review", TaskStateKind::Queue, 2),
    ("merge", TaskStateKind::Queue, 3),
    ("needs_human", TaskStateKind::Human, 4),
    ("done", TaskStateKind::Terminal, 5),
    ("cancelled", TaskStateKind::Terminal, 6),
];

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn state_kinds_serialise_in_snake_case() {
        assert_eq!(
            serde_json::to_value(TaskStateKind::Queue).unwrap(),
            json!("queue")
        );
        assert_eq!(
            serde_json::to_value(TaskStateKind::Human).unwrap(),
            json!("human")
        );
        assert_eq!(
            serde_json::to_value(TaskStateKind::Terminal).unwrap(),
            json!("terminal")
        );
        assert_eq!(
            serde_json::from_value::<TaskStateKind>(json!("terminal")).unwrap(),
            TaskStateKind::Terminal
        );
        assert!(serde_json::from_value::<TaskStateKind>(json!("Queue")).is_err());
    }

    #[test]
    fn names_accept_the_documented_pattern() {
        for name in [
            "backlog",
            "needs_human",
            "in-review",
            "a",
            "0",
            "9x_y-z",
            "state1",
        ] {
            assert!(TaskStateName::parse(name).is_ok(), "rejected {name}");
        }
    }

    #[test]
    fn names_accept_one_and_thirty_two_characters_and_reject_thirty_three() {
        assert_eq!(TaskStateName::parse("a").unwrap().as_str(), "a");
        let longest = "s".repeat(MAX_STATE_NAME_CHARS);
        assert_eq!(TaskStateName::parse(&longest).unwrap().as_str(), longest);
        assert_eq!(
            TaskStateName::parse(&"s".repeat(MAX_STATE_NAME_CHARS + 1)),
            Err(TaskError::InvalidStateName)
        );
    }

    #[test]
    fn names_reject_everything_else() {
        for name in [
            "",
            " ",
            "_leading",
            "-leading",
            "Backlog",
            "needs human",
            "needs.human",
            "réady",
            "ready\n",
            "ready!",
        ] {
            assert_eq!(
                TaskStateName::parse(name),
                Err(TaskError::InvalidStateName),
                "accepted {name:?}"
            );
        }
    }

    #[test]
    fn a_new_state_validates_its_name_and_appends_by_default() {
        let project_id = Uuid::new_v4();
        let state = NewTaskState::new(project_id, "review", TaskStateKind::Queue).unwrap();
        assert_eq!(state.project_id, project_id);
        assert_eq!(state.name.as_str(), "review");
        assert_eq!(state.kind, TaskStateKind::Queue);
        assert!(state.position.is_none());

        assert_eq!(
            NewTaskState::new(project_id, "Review", TaskStateKind::Queue).unwrap_err(),
            TaskError::InvalidStateName
        );
    }

    #[test]
    fn the_default_set_is_the_documented_one() {
        assert_eq!(
            DEFAULT_TASK_STATES,
            [
                ("backlog", TaskStateKind::Queue, 0),
                ("ready", TaskStateKind::Queue, 1),
                ("review", TaskStateKind::Queue, 2),
                ("merge", TaskStateKind::Queue, 3),
                ("needs_human", TaskStateKind::Human, 4),
                ("done", TaskStateKind::Terminal, 5),
                ("cancelled", TaskStateKind::Terminal, 6),
            ]
        );
    }

    #[test]
    fn the_default_set_is_valid_and_has_one_human_state() {
        for (name, _, _) in DEFAULT_TASK_STATES {
            assert!(TaskStateName::parse(name).is_ok(), "invalid default {name}");
        }
        let human = DEFAULT_TASK_STATES
            .iter()
            .filter(|(_, kind, _)| *kind == TaskStateKind::Human)
            .count();
        assert_eq!(human, 1);
    }
}
