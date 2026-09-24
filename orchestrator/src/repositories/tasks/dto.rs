//! Assembling the API-facing task shapes out of the tracker tables.
//!
//! `SPEC.md`, "Tasks" puts three things on a task that the `tasks` row does
//! not carry: its state's **name**, its dependency edges in both directions,
//! and its current hand-off. Fetching those per task is the obvious N+1, and
//! the board asks for every task in a project at once, so every loader here
//! goes through one batch assembly:
//!
//! 1. the state names, for the state ids the rows reference;
//! 2. the outgoing edges, for all the tasks at once;
//! 3. the incoming `blocks` edges, for all the tasks at once;
//! 4. the hand-off records `current_handoff_id` names, for all of them.
//!
//! Four queries whatever the number of tasks, and none at all for an empty
//! list.
//!
//! **These are reads, and reads take no lock** (ADR 0021). The `_in` variant
//! exists for the opposite reason: a mutation has to put the task *as it is
//! inside the open transaction* into its event payload, so it borrows the
//! caller's connection rather than going to the pool and seeing the
//! pre-change row.

use std::collections::HashMap;

use sqlx::PgConnection;
use uuid::Uuid;

use crate::models::{ReviewStatus, Task, TaskDependencyKind, TaskHandoff, TaskRef};
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::tracker::Locked;
use crate::tracker::{CommentDto, DependencyRef, HandoffDto, TaskDetailDto, TaskDto};

impl TaskRepository<'_> {
    /// One task as the API sends it, or `None` when this project has no such
    /// task.
    pub async fn load_task_dto(&self, project_id: Uuid, task_id: Uuid) -> Result<Option<TaskDto>> {
        let Some(task) = self.find_task(project_id, TaskRef::Id(task_id)).await? else {
            return Ok(None);
        };

        Ok(self
            .load_task_dtos(project_id, std::slice::from_ref(&task))
            .await?
            .pop())
    }

    /// The same, for a task being changed inside an open mutation.
    ///
    /// The one read here that takes the token. An event payload describes the
    /// task *after* the change, which is only visible on the connection that
    /// made it; the pool would still answer with the row as it was before the
    /// transaction commits.
    pub async fn load_task_dto_in(
        &self,
        mut conn: Locked<'_>,
        project_id: Uuid,
        task_id: Uuid,
    ) -> Result<Option<TaskDto>> {
        let Some(task) = sqlx::query_as!(
            Task,
            r#"
            SELECT id, project_id, number, title, description, state_id, priority, blocked,
                   labels, parent_id, assignee_user_id, lease_holder_session_id, lease_since,
                   attempts, rounds, needs_human_reason, current_handoff_id, created_by_user_id,
                   created_by_session_id, created_at, updated_at, closed_at
            FROM tasks
            WHERE id = $1 AND project_id = $2
            "#,
            task_id,
            project_id,
        )
        .fetch_optional(&mut *conn)
        .await?
        else {
            return Ok(None);
        };

        Ok(
            assemble(&mut conn, Some(project_id), std::slice::from_ref(&task))
                .await?
                .pop(),
        )
    }

    /// A batch of tasks from one project, in the order they were given.
    ///
    /// The list endpoints hand their `Vec<Task>` straight to this: the row
    /// query has already ordered and scoped it, and nothing here reorders.
    pub async fn load_task_dtos(&self, project_id: Uuid, tasks: &[Task]) -> Result<Vec<TaskDto>> {
        let mut conn = self.pool.acquire().await?;
        assemble(&mut conn, Some(project_id), tasks).await
    }

    /// A batch of tasks that may come from several projects.
    ///
    /// The dashboard's `GET /tasks?state_kind=human` is the caller: it lists
    /// across every project the user can see, so there is no single project to
    /// scope the state lookup by (`SPEC.md`, "Tasks").
    pub async fn load_task_dtos_any_project(&self, tasks: &[Task]) -> Result<Vec<TaskDto>> {
        let mut conn = self.pool.acquire().await?;
        assemble(&mut conn, None, tasks).await
    }

    /// Everything the detail drawer shows, or `None` when this project has no
    /// such task (`TaskDetail`, `SPEC.md`, "Tasks").
    ///
    /// `task_ref` may be a UUID or a per-project number; a number nobody
    /// carries is `None`, exactly like an unknown UUID.
    pub async fn load_task_detail(
        &self,
        project_id: Uuid,
        task_ref: TaskRef,
    ) -> Result<Option<TaskDetailDto>> {
        let Some(task) = self.find_task(project_id, task_ref).await? else {
            return Ok(None);
        };

        let Some(dto) = self
            .load_task_dtos(project_id, std::slice::from_ref(&task))
            .await?
            .pop()
        else {
            return Ok(None);
        };

        let comments = self.list_comments(project_id, task.id).await?;
        let handoffs = self.list_handoffs(project_id, task.id).await?;
        let children = self.list_children(project_id, task.id).await?;
        let sessions = self.list_task_sessions(task.id).await?;

        Ok(Some(TaskDetailDto {
            task: dto,
            comments: comments.into_iter().map(CommentDto::from).collect(),
            handoffs: handoffs.into_iter().map(HandoffDto::from).collect(),
            children: self.load_task_dtos(project_id, &children).await?,
            sessions,
        }))
    }
}

/// The batch assembly every loader above goes through.
///
/// `project_id` is `Some` for the project-scoped callers, where it is the
/// scope check on the state lookup, and `None` for the cross-project
/// dashboard. The output is one DTO per input task, in the input order.
async fn assemble(
    conn: &mut PgConnection,
    project_id: Option<Uuid>,
    tasks: &[Task],
) -> Result<Vec<TaskDto>> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }

    let task_ids: Vec<Uuid> = tasks.iter().map(|task| task.id).collect();
    let state_ids: Vec<Uuid> = tasks.iter().map(|task| task.state_id).collect();
    let handoff_ids: Vec<Uuid> = tasks
        .iter()
        .filter_map(|task| task.current_handoff_id)
        .collect();

    let state_names = load_state_names(&mut *conn, project_id, &state_ids).await?;
    let mut depends_on = load_dependencies(&mut *conn, &task_ids).await?;
    let mut blocks = load_blocks(&mut *conn, &task_ids).await?;
    let mut handoffs = load_handoffs(&mut *conn, &handoff_ids).await?;

    let mut dtos = Vec::with_capacity(tasks.len());
    for task in tasks {
        let Some(state) = state_names.get(&task.state_id) else {
            // `tasks.state_id` has a foreign key to `task_states`, and the
            // state belongs to the task's project, so this is the binary
            // disagreeing with the database rather than anything a caller did.
            error!(
                task_id = %task.id,
                project_id = %task.project_id,
                "a task references a state that is not in its project",
            );
            return Err(Error::Internal("a task has no state".to_string()));
        };

        dtos.push(TaskDto::from_parts(
            task,
            state,
            depends_on.remove(&task.id).unwrap_or_default(),
            blocks.remove(&task.id).unwrap_or_default(),
            current_handoff(task, &mut handoffs),
        ));
    }

    Ok(dtos)
}

/// The record `tasks.current_handoff_id` names, taken out of the batch.
///
/// `Task.handoff` is "the record `current_handoff_id` names, resolved, or
/// `null`" (`SPEC.md`, "Tasks"), and the pointer can only ever name a record
/// of this same task: the insert checks it, and the column is `ON DELETE SET
/// NULL`. So a pointer this batch found no record for is the two reads
/// disagreeing — a delete landing between them — and never a caller's doing.
/// The task is still sent, with `handoff: null`, and the disagreement is
/// logged rather than swallowed.
fn current_handoff(task: &Task, handoffs: &mut HashMap<Uuid, TaskHandoff>) -> Option<HandoffDto> {
    let handoff_id = task.current_handoff_id?;

    let Some(row) = handoffs.remove(&handoff_id) else {
        warn!(
            task_id = %task.id,
            handoff_id = %handoff_id,
            "a task's current hand-off names a record that is not there",
        );
        return None;
    };

    Some(HandoffDto::from(row))
}

/// The names of these states, scoped to `project_id` when there is one.
async fn load_state_names(
    conn: &mut PgConnection,
    project_id: Option<Uuid>,
    state_ids: &[Uuid],
) -> Result<HashMap<Uuid, String>> {
    let rows = sqlx::query!(
        r#"
        SELECT id, name
        FROM task_states
        WHERE id = ANY($1) AND ($2::uuid IS NULL OR project_id = $2)
        "#,
        state_ids,
        project_id,
    )
    .fetch_all(&mut *conn)
    .await?;

    Ok(rows.into_iter().map(|row| (row.id, row.name)).collect())
}

/// Every outgoing edge of these tasks, grouped by the task it belongs to.
///
/// Ordered as [`TaskRepository::list_dependencies`] orders one task's edges —
/// by the other end, then the kind — so a pair carrying both a `blocks` and a
/// `discovered_from` edge lists them together and the order is stable between
/// reads.
async fn load_dependencies(
    conn: &mut PgConnection,
    task_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<DependencyRef>>> {
    let rows = sqlx::query!(
        r#"
        SELECT task_id, depends_on_task_id, kind AS "kind: TaskDependencyKind"
        FROM task_dependencies
        WHERE task_id = ANY($1)
        ORDER BY task_id, depends_on_task_id, kind
        "#,
        task_ids,
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut grouped: HashMap<Uuid, Vec<DependencyRef>> = HashMap::new();
    for row in rows {
        grouped.entry(row.task_id).or_default().push(DependencyRef {
            task_id: row.depends_on_task_id,
            kind: row.kind,
        });
    }

    Ok(grouped)
}

/// The incoming `blocks` edges of these tasks, grouped by the task they point
/// at — `Task.blocks`, "the tasks that have a `blocks` dependency on this
/// one" (`SPEC.md`, "Tasks").
///
/// Only `blocks`: the other kinds are informational and are listed on the
/// depending task's `depends_on` alone.
async fn load_blocks(
    conn: &mut PgConnection,
    task_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<Uuid>>> {
    let rows = sqlx::query!(
        r#"
        SELECT depends_on_task_id, task_id
        FROM task_dependencies
        WHERE depends_on_task_id = ANY($1) AND kind = $2
        ORDER BY depends_on_task_id, task_id
        "#,
        task_ids,
        TaskDependencyKind::Blocks as TaskDependencyKind,
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut grouped: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for row in rows {
        grouped
            .entry(row.depends_on_task_id)
            .or_default()
            .push(row.task_id);
    }

    Ok(grouped)
}

/// The hand-off records these ids name, by id.
///
/// No scope clause: the ids come from `tasks.current_handoff_id` of rows the
/// caller already read within its scope, and a hand-off belongs to its task.
async fn load_handoffs(
    conn: &mut PgConnection,
    handoff_ids: &[Uuid],
) -> Result<HashMap<Uuid, TaskHandoff>> {
    if handoff_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query_as!(
        TaskHandoff,
        r#"
        SELECT id, task_id, source_session_id, source_branch, commit, comment_id,
               review_status AS "review_status: ReviewStatus", reviewed_by_user_id,
               reviewed_by_session_id, reviewed_at, created_by_user_id, created_by_session_id,
               created_at
        FROM task_handoffs
        WHERE id = ANY($1)
        "#,
        handoff_ids,
    )
    .fetch_all(&mut *conn)
    .await?;

    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}
