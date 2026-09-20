//! Dependency edges between tasks.
//!
//! `docs/data-model.md`, `task_dependencies` and `SPEC.md`, "Tasks". An edge is
//! identified by `(task_id, depends_on_task_id, kind)`, so the same pair can
//! carry both a `blocks` edge and its provenance. Only `blocks` affects
//! `tasks.blocked` and only `blocks` edges are checked for cycles, which needs
//! the other rows and therefore happens in the repository under the project
//! lock.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::task::{TaskError, TaskResult};
// The crate convention (`CLAUDE.md`, "Backend conventions"); see `task.rs`.
#[allow(unused_imports)]
use crate::prelude::*;

/// What one task's dependency on another means.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, Default, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "task_dependency_kind", rename_all = "snake_case")]
pub enum TaskDependencyKind {
    /// `task_id` is not claimable until `depends_on_task_id` is terminal. The
    /// column default, and the API's default for a new edge.
    #[default]
    Blocks,
    /// `task_id` was created while working on `depends_on_task_id`;
    /// provenance only.
    DiscoveredFrom,
    /// Informational.
    Related,
}

impl TaskDependencyKind {
    /// Does this edge participate in `blocked` and in the cycle check?
    pub fn is_blocking(self) -> bool {
        matches!(self, Self::Blocks)
    }
}

/// A `task_dependencies` row, column for column (`docs/data-model.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskDependency {
    pub task_id: Uuid,
    pub depends_on_task_id: Uuid,
    pub kind: TaskDependencyKind,
}

impl TaskDependency {
    /// An edge, rejecting the one rule that needs no other row: a task cannot
    /// depend on itself (`CHECK (task_id <> depends_on_task_id)`).
    ///
    /// Same-project membership and `blocks` cycles are checked in the
    /// repository, which has the other tasks and the project lock.
    pub fn new(
        task_id: Uuid,
        depends_on_task_id: Uuid,
        kind: TaskDependencyKind,
    ) -> TaskResult<Self> {
        let edge = Self {
            task_id,
            depends_on_task_id,
            kind,
        };
        edge.validate()?;
        Ok(edge)
    }

    /// The self-dependency rule, for edges built field by field.
    pub fn validate(&self) -> TaskResult<()> {
        if self.task_id == self.depends_on_task_id {
            return Err(TaskError::SelfDependency);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn dependency_kinds_serialise_in_snake_case() {
        assert_eq!(
            serde_json::to_value(TaskDependencyKind::Blocks).unwrap(),
            json!("blocks")
        );
        assert_eq!(
            serde_json::to_value(TaskDependencyKind::DiscoveredFrom).unwrap(),
            json!("discovered_from")
        );
        assert_eq!(
            serde_json::to_value(TaskDependencyKind::Related).unwrap(),
            json!("related")
        );
        assert_eq!(
            serde_json::from_value::<TaskDependencyKind>(json!("discovered_from")).unwrap(),
            TaskDependencyKind::DiscoveredFrom
        );
    }

    #[test]
    fn blocks_is_the_default_and_the_only_blocking_kind() {
        assert_eq!(TaskDependencyKind::default(), TaskDependencyKind::Blocks);
        assert!(TaskDependencyKind::Blocks.is_blocking());
        assert!(!TaskDependencyKind::DiscoveredFrom.is_blocking());
        assert!(!TaskDependencyKind::Related.is_blocking());
    }

    #[test]
    fn an_edge_between_two_tasks_is_accepted() {
        let task_id = Uuid::new_v4();
        let depends_on_task_id = Uuid::new_v4();
        let edge =
            TaskDependency::new(task_id, depends_on_task_id, TaskDependencyKind::Blocks).unwrap();
        assert_eq!(edge.task_id, task_id);
        assert_eq!(edge.depends_on_task_id, depends_on_task_id);
        assert_eq!(edge.kind, TaskDependencyKind::Blocks);
    }

    #[test]
    fn a_task_cannot_depend_on_itself() {
        let task_id = Uuid::new_v4();
        for kind in [
            TaskDependencyKind::Blocks,
            TaskDependencyKind::DiscoveredFrom,
            TaskDependencyKind::Related,
        ] {
            assert_eq!(
                TaskDependency::new(task_id, task_id, kind).unwrap_err(),
                TaskError::SelfDependency
            );
        }
    }
}
