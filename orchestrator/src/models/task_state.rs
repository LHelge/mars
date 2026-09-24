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

/// A task state as the API shows it (`SPEC.md`, "Task states"): the
/// `task_states` row (`docs/data-model.md`), with `conflict_state_id` resolved
/// to the *name* of the state it references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskState {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub kind: TaskStateKind,
    pub position: i32,
    /// The orchestrator merges approved hand-offs of tasks in this state
    /// (ADR 0045). `queue` states only.
    pub auto_merge: bool,
    /// Where a task goes when its automatic merge conflicts; `Some` exactly
    /// when `auto_merge` is set.
    pub conflict_state: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A state's auto-merge configuration as a caller supplies it: the flag and
/// the raw name of its conflict state.
///
/// On `POST` it is the whole configuration; on `PUT` a body that gives either
/// field replaces both, so a missing `auto_merge` there reads as `false`
/// (`SPEC.md`, "Task states").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoMergeInput {
    /// Whether the state merges approved hand-offs.
    pub auto_merge: bool,
    /// The name of the state a conflicting merge sends a task to.
    pub conflict_state: Option<String>,
}

impl AutoMergeInput {
    /// Validate the pair for a state of `kind` called `own_name`, returning
    /// the conflict state's name when `auto_merge` is on.
    ///
    /// The rules a single body can break, in the order a caller fixes them:
    /// queue-only, the pair set together, and not the state itself. Whether
    /// the name is a `queue` state of the project is a fact about the project
    /// and is the repository's to decide; a name that cannot be a state name
    /// at all is refused here with that same message.
    pub fn validate(
        &self,
        kind: TaskStateKind,
        own_name: &str,
    ) -> TaskResult<Option<TaskStateName>> {
        if self.auto_merge && kind != TaskStateKind::Queue {
            return Err(TaskError::AutoMergeNotQueue);
        }
        match (self.auto_merge, self.conflict_state.as_deref()) {
            (false, None) => Ok(None),
            (false, Some(_)) => Err(TaskError::ConflictStateRequiresAutoMerge),
            (true, None) => Err(TaskError::AutoMergeRequiresConflictState),
            (true, Some(name)) if name == own_name => Err(TaskError::ConflictStateIsSelf),
            (true, Some(name)) => TaskStateName::parse(name)
                .map(Some)
                .map_err(|_| TaskError::ConflictStateNotQueue),
        }
    }
}

/// The caller-supplied half of a new task state.
///
/// A `None` position appends; the repository shifts the states at and after an
/// explicit one (`SPEC.md`, "Task states"). `conflict_state` is `Some` exactly
/// when the state is created with `auto_merge` on — [`AutoMergeInput::validate`]
/// has already paired them — and the repository resolves it to an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTaskState {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: TaskStateName,
    pub kind: TaskStateKind,
    pub position: Option<i32>,
    pub conflict_state: Option<TaskStateName>,
}

impl NewTaskState {
    /// A state with a fresh id and `auto_merge` off, validating the name.
    pub fn new(project_id: Uuid, name: &str, kind: TaskStateKind) -> TaskResult<Self> {
        Ok(Self {
            id: Uuid::new_v4(),
            project_id,
            name: TaskStateName::parse(name)?,
            kind,
            position: None,
            conflict_state: None,
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

    fn pair(auto_merge: bool, conflict_state: Option<&str>) -> AutoMergeInput {
        AutoMergeInput {
            auto_merge,
            conflict_state: conflict_state.map(str::to_string),
        }
    }

    #[test]
    fn auto_merge_accepts_a_queue_state_with_another_conflict_state() {
        assert_eq!(
            pair(true, Some("ready"))
                .validate(TaskStateKind::Queue, "merge")
                .unwrap()
                .map(String::from),
            Some("ready".to_string())
        );
        assert_eq!(
            pair(false, None).validate(TaskStateKind::Terminal, "done"),
            Ok(None)
        );
    }

    #[test]
    fn auto_merge_refuses_each_broken_rule_with_its_own_error() {
        for kind in [TaskStateKind::Human, TaskStateKind::Terminal] {
            assert_eq!(
                pair(true, Some("ready")).validate(kind, "x"),
                Err(TaskError::AutoMergeNotQueue)
            );
        }
        assert_eq!(
            pair(true, None).validate(TaskStateKind::Queue, "merge"),
            Err(TaskError::AutoMergeRequiresConflictState)
        );
        assert_eq!(
            pair(false, Some("ready")).validate(TaskStateKind::Queue, "merge"),
            Err(TaskError::ConflictStateRequiresAutoMerge)
        );
        assert_eq!(
            pair(true, Some("merge")).validate(TaskStateKind::Queue, "merge"),
            Err(TaskError::ConflictStateIsSelf)
        );
        assert_eq!(
            pair(true, Some("Not A State")).validate(TaskStateKind::Queue, "merge"),
            Err(TaskError::ConflictStateNotQueue)
        );
    }

    #[test]
    fn auto_merge_messages_are_the_documented_ones() {
        for (error, message) in [
            (
                TaskError::AutoMergeNotQueue,
                "auto_merge applies to queue states only",
            ),
            (
                TaskError::AutoMergeRequiresConflictState,
                "auto_merge requires conflict_state",
            ),
            (
                TaskError::ConflictStateRequiresAutoMerge,
                "conflict_state requires auto_merge",
            ),
            (
                TaskError::ConflictStateNotQueue,
                "conflict_state must name a queue state of this project",
            ),
            (
                TaskError::ConflictStateIsSelf,
                "conflict_state must be a different state",
            ),
        ] {
            assert_eq!(error.to_string(), message);
        }
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
