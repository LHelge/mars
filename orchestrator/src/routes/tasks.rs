//! `/api/projects/{pid}/tasks` and the dashboard's `/api/tasks`
//! (`SPEC.md`, "Tasks").
//!
//! The read side of the tracker and the writes this module carries:
//!
//! | Endpoint | What it is |
//! | --- | --- |
//! | `POST /projects/{pid}/tasks` | a [`create_task`] mutation, 201 with the task |
//! | `GET /projects/{pid}/tasks` | the board read, filtered and ordered by priority then number |
//! | `GET /projects/{pid}/tasks/{id}` | the detail drawer's `TaskDetail` |
//! | `POST /projects/{pid}/tasks/{id}/dependencies` | one edge added, 200 with the dependant |
//! | `DELETE /projects/{pid}/tasks/{id}/dependencies/{dep}?kind=` | one edge of that kind removed, 200 with the dependant |
//! | `POST /projects/{pid}/tasks/{id}/comments` | one comment written, 201 with it |
//! | `GET /tasks?state_kind=` | the dashboard's cross-project list |
//!
//! **The three lists take no lock.** A board read takes no part in anyone's
//! mutation and wants the committed state (ADR 0021), so they go straight to
//! the pool and assemble their DTOs through the batch loaders, which cost four
//! queries whatever the number of tasks. There is no pagination in v1:
//! `SPEC.md` types the response `Task[]`.
//!
//! **The create does nothing but fill a mutation.** Every rule it applies —
//! the state name, the priority, the labels, the parent, the cycle check, the
//! `blocked` recomputation and the events — is
//! [`tracker::tasks::create_task`](crate::tracker::tasks::create_task)'s,
//! shared with the MCP `create_task` tool; what this module decides is the
//! HTTP shape around it. `assignee_user_id` is deliberately not in the body:
//! `SPEC.md` gives it to `PUT` alone.
//!
//! **`{id}` and `?parent=` accept either reference.** A run of digits is a
//! per-project number and anything else has to be a UUID
//! ([`TaskRef`]); a value that is neither addresses no task and is therefore
//! 404, not 400 — the same answer a well-formed id nobody carries gets, so a
//! caller learns nothing from the difference.

use std::str::FromStr;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use serde::Deserialize;
use uuid::Uuid;

use crate::events::TaskActor;
use crate::models::{Priority, Task, TaskDependencyKind, TaskRef, TaskStateKind};
use crate::prelude::*;
use crate::repositories::{ProjectRepository, TaskFilter, TaskRepository};
use crate::routes::{CurrentUser, Path, Query};
use crate::tracker::state::resolve_state_in_pool;
use crate::tracker::tasks::{CreateTaskInput, CreatedBy, create_task};
use crate::tracker::{
    CommentAuthor, CommentDto, TaskDetailDto, TaskDto, TrackerMutation, add_comment, dependencies,
};

/// What `GET /tasks` without a `state_kind` is told (400).
const STATE_KIND_REQUIRED: &str = "state_kind is required";

/// What a `kind` that is not one of the three is told (400).
const INVALID_KIND: &str = "invalid dependency kind";

/// What `DELETE .../dependencies/{dep}` without a `?kind=` is told (400).
///
/// Required rather than defaulted: removing "the dependency" is ambiguous once
/// a pair can carry three edges, and guessing `blocks` would silently drop the
/// blocker of a caller who meant the provenance (`SPEC.md`, "Tasks").
const KIND_REQUIRED: &str = "kind is required";

/// The router nested at `/api/tasks`: the dashboard's cross-project list.
///
/// One route, because `SPEC.md` gives `/tasks` exactly one: everything else
/// about a task is project-scoped and lives in [`project_routes`].
pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(dashboard))
}

/// The router merged onto `/api/projects`.
///
/// The `{pid}` capture is part of these paths rather than of the `nest`, for
/// the reason the other project sub-resources give: axum takes one `nest` per
/// prefix, so every project-scoped router carries its own `{pid}/…`
/// (`routes::mod`).
pub fn project_routes() -> Router<AppState> {
    Router::new()
        .route("/{pid}/tasks", get(list).post(create))
        .route("/{pid}/tasks/{id}", get(detail))
        .route("/{pid}/tasks/{id}/dependencies", post(add_dependency))
        .route(
            "/{pid}/tasks/{id}/dependencies/{dep}",
            delete(remove_dependency),
        )
        .route("/{pid}/tasks/{id}/comments", post(comment))
}

// ---- create ----

/// `POST /projects/{pid}/tasks` (`SPEC.md`, "Tasks").
///
/// `deny_unknown_fields` for the reason the other route modules give: a client
/// sending `assignee_user_id` here has misunderstood the endpoint — that field
/// is `PUT`'s — and is told so rather than silently ignored.
///
/// `depends_on` entries are strings rather than UUIDs because a task is
/// addressed by its UUID *or* its per-project number, and a number is only
/// resolvable under the project lock, which is where [`create_task`] resolves
/// it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTaskRequest {
    title: String,
    description: Option<String>,
    state: Option<String>,
    priority: Option<i16>,
    labels: Option<Vec<String>>,
    parent_id: Option<Uuid>,
    depends_on: Option<Vec<String>>,
}

/// `POST /projects/{pid}/tasks` → the created task (201).
///
/// 400 for an unknown state, a priority outside 0–3, a label that is not one,
/// a title that is not one, a parent that is not a top-level task of this
/// project and a `depends_on` entry that names no task of it; 409 for a
/// dependency that would close a cycle; 404 for an unknown project, raised by
/// the mutation's own lock before anything is written.
///
/// The whole creation is one transaction, so a refusal at any of those points
/// leaves no task, no edges and no events behind.
async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(pid): Path<Uuid>,
    Json(body): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<TaskDto>)> {
    let depends_on = body
        .depends_on
        .unwrap_or_default()
        .iter()
        .map(|raw| TaskRef::from_str(raw))
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let input = CreateTaskInput {
        title: body.title,
        description: body.description,
        state: body.state,
        priority: body.priority,
        labels: body.labels.unwrap_or_default(),
        parent: body.parent_id,
        depends_on,
        created_by: CreatedBy::User(user.id),
    };

    let mut mutation =
        TrackerMutation::begin(&state.pool, pid, TaskActor::User { user_id: user.id }).await?;
    let task = create_task(&mut mutation, input).await?;
    mutation.commit().await?;

    Ok((StatusCode::CREATED, Json(task)))
}

// ---- project list ----

/// `?state=&label=&priority=&parent=&held=` (`SPEC.md`, "Tasks").
///
/// Every field is a `String` or a plain scalar rather than the resolved type:
/// `state` is a name this project may not have, `parent` is either of the two
/// task references, and `priority` has a documented range of its own. Each is
/// turned into its part of a [`TaskFilter`] below, with the documented answer
/// when it is not usable.
#[derive(Debug, Deserialize)]
struct ListQuery {
    state: Option<String>,
    label: Option<String>,
    priority: Option<i16>,
    parent: Option<String>,
    held: Option<bool>,
}

/// `GET /projects/{pid}/tasks` → the project's tasks, priority then number.
///
/// 404 for an unknown project and for a `?parent=` naming no task of it; 400
/// for an unknown `?state=` — with the project's valid names — and for a
/// `?priority=` outside 0–3. An unknown `?label=` matches nothing rather than
/// failing: a filter is a question, not a validated field.
///
/// Closed tasks are included; the board shows every column, `done` among them.
async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(pid): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<TaskDto>>> {
    let projects = ProjectRepository::new(&state.pool);
    projects.find(pid).await?.ok_or(Error::NotFound)?;

    let tasks = TaskRepository::new(&state.pool);

    let state_id = match query.state.as_deref() {
        Some(name) => Some(resolve_state_in_pool(&state.pool, pid, name).await?.id),
        None => None,
    };

    let priority = match query.priority {
        Some(priority) => Some(Priority::try_from(priority)?.get()),
        None => None,
    };

    let parent_id = match query.parent.as_deref() {
        Some(raw) => Some(
            tasks
                .find_task(pid, task_ref(raw)?)
                .await?
                .ok_or(Error::NotFound)?
                .id,
        ),
        None => None,
    };

    let filter = TaskFilter {
        state_id,
        label: query.label,
        priority,
        parent_id,
        held: query.held,
    };

    let rows = tasks.list_tasks(pid, &filter).await?;

    Ok(Json(tasks.load_task_dtos(pid, &rows).await?))
}

// ---- detail ----

/// `GET /projects/{pid}/tasks/{id}` → the task with its comments, hand-offs,
/// children and sessions (404 for an unknown project or task).
///
/// One 404 for both: a task of another project is not this caller's to
/// distinguish from one that does not exist.
async fn detail(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((pid, id)): Path<(Uuid, String)>,
) -> Result<Json<TaskDetailDto>> {
    let detail = TaskRepository::new(&state.pool)
        .load_task_detail(pid, task_ref(&id)?)
        .await?
        .ok_or(Error::NotFound)?;

    Ok(Json(detail))
}

// ---- dependencies ----

/// `POST /projects/{pid}/tasks/{id}/dependencies` (`SPEC.md`, "Tasks").
///
/// `depends_on` is a string rather than a UUID for the reason `POST
/// /tasks`'s `depends_on` entries are: a task is addressed by its UUID *or*
/// its per-project number. `kind` is a string rather than a
/// [`TaskDependencyKind`] so that an unrecognised value is the documented
/// `invalid dependency kind` and not serde's own message about a variant name
/// — the same reason `?state_kind=` below is a `String`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddDependencyRequest {
    depends_on: String,
    kind: Option<String>,
}

/// `POST /projects/{pid}/tasks/{id}/dependencies` → the dependant task (200).
///
/// `kind` defaults to `blocks`. 400 for an unknown kind, for a self-edge and
/// for a `depends_on` UUID naming a task of another project; 404 for an
/// unknown project, an unknown `{id}` and a `depends_on` naming no task at
/// all; 409 for a `blocks` edge that would close a cycle and for an edge that
/// is already there, kind and all.
///
/// Both ends are resolved *inside* the mutation, under the project lock: a
/// per-project number resolved before the lock could name a different task by
/// the time the edge is inserted.
async fn add_dependency(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((pid, id)): Path<(Uuid, String)>,
    Json(body): Json<AddDependencyRequest>,
) -> Result<Json<TaskDto>> {
    let dependant = task_ref(&id)?;
    let prerequisite = task_ref(&body.depends_on)?;
    let kind = match body.kind.as_deref() {
        Some(raw) => dependency_kind(raw)?,
        None => TaskDependencyKind::default(),
    };

    let mut mutation =
        TrackerMutation::begin(&state.pool, pid, TaskActor::User { user_id: user.id }).await?;

    let task = locked_task(&mut mutation, pid, dependant).await?;
    let depends_on = dependencies::resolve_dependency(&mut mutation, prerequisite).await?;
    let task = dependencies::add_dependency(&mut mutation, &task, &depends_on, kind).await?;

    mutation.commit().await?;

    Ok(Json(task))
}

/// `?kind=` on `DELETE .../dependencies/{dep}` (`SPEC.md`, "Tasks").
///
/// Optional in the type and required by the handler, so that leaving it out is
/// the documented 400 rather than a rejection from the extractor.
#[derive(Debug, Deserialize)]
struct KindQuery {
    kind: Option<String>,
}

/// `DELETE /projects/{pid}/tasks/{id}/dependencies/{dep}?kind=` → the
/// dependant task (200).
///
/// Removes that kind alone: a pair joined by `blocks` and `discovered_from`
/// keeps the provenance when the blocker goes. 400 for a missing or unknown
/// `kind`; 404 for an unknown project, an unknown `{id}` or `{dep}`, and for
/// an edge of that kind that is not there.
///
/// 200 with a body rather than the usual 204 for a delete: `SPEC.md` types the
/// response `Task`, because removing a blocker can unblock the task and the
/// caller wants the flag it now has.
async fn remove_dependency(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((pid, id, dep)): Path<(Uuid, String, String)>,
    Query(query): Query<KindQuery>,
) -> Result<Json<TaskDto>> {
    let dependant = task_ref(&id)?;
    let prerequisite = task_ref(&dep)?;
    let kind = match query.kind.as_deref() {
        Some(raw) => dependency_kind(raw)?,
        None => return Err(Error::BadRequest(KIND_REQUIRED.into())),
    };

    let mut mutation =
        TrackerMutation::begin(&state.pool, pid, TaskActor::User { user_id: user.id }).await?;

    let task = locked_task(&mut mutation, pid, dependant).await?;
    let depends_on = dependencies::resolve_dependency(&mut mutation, prerequisite).await?;
    let task = dependencies::remove_dependency(&mut mutation, &task, &depends_on, kind).await?;

    mutation.commit().await?;

    Ok(Json(task))
}

// ---- comments ----

/// `POST /projects/{pid}/tasks/{id}/comments` (`SPEC.md`, "Tasks").
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommentRequest {
    body: String,
}

/// `POST /projects/{pid}/tasks/{id}/comments` → the comment (201).
///
/// 201 by the general create rule (`CLAUDE.md`, "API conventions"); `SPEC.md`
/// types the response `Comment` and leaves the status to that rule.
///
/// 400 for a body that is empty after trimming; 404 for an unknown project or
/// task. There is no lease to hold: commenting is open to anyone who can see
/// the project.
async fn comment(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((pid, id)): Path<(Uuid, String)>,
    Json(body): Json<CommentRequest>,
) -> Result<(StatusCode, Json<CommentDto>)> {
    let reference = task_ref(&id)?;

    let mut mutation =
        TrackerMutation::begin(&state.pool, pid, TaskActor::User { user_id: user.id }).await?;

    let task = locked_task(&mut mutation, pid, reference).await?;
    let comment = add_comment(
        &mut mutation,
        &task,
        CommentAuthor::User(user.id),
        &body.body,
    )
    .await?;

    mutation.commit().await?;

    Ok((StatusCode::CREATED, Json(comment)))
}

/// The `{id}` of a dependency or comment path, read under the mutation's lock.
///
/// The three handlers above all start here rather than with a pool read: the
/// task they are about to change has to be the row as it is inside the lock,
/// and a per-project number resolved outside it can name a different task by
/// the time the change lands. An address that names no task of this project is
/// 404, whichever of the two forms it took.
async fn locked_task(
    mutation: &mut TrackerMutation<'_>,
    pid: Uuid,
    reference: TaskRef,
) -> Result<Task> {
    TaskRepository::new(mutation.pool())
        .find_task_for_update(mutation.conn(), pid, reference)
        .await?
        .ok_or(Error::NotFound)
}

/// One of the three dependency kinds, or 400.
///
/// The strings are the ones [`TaskDependencyKind`] serialises to, matched by
/// hand so that an unrecognised value is the documented message rather than
/// serde's.
fn dependency_kind(raw: &str) -> Result<TaskDependencyKind> {
    match raw {
        "blocks" => Ok(TaskDependencyKind::Blocks),
        "discovered_from" => Ok(TaskDependencyKind::DiscoveredFrom),
        "related" => Ok(TaskDependencyKind::Related),
        _ => Err(Error::BadRequest(INVALID_KIND.into())),
    }
}

// ---- dashboard list ----

/// `?state_kind=` on `GET /tasks` (`SPEC.md`, "Tasks", "Frontend" →
/// "Dashboard").
///
/// A `String` rather than a [`TaskStateKind`], because an unrecognised value
/// is a 400 naming the three kinds and a typed field would instead answer
/// serde's own message about a variant name.
#[derive(Debug, Deserialize)]
struct StateKindQuery {
    state_kind: Option<String>,
}

/// `GET /tasks?state_kind=human` → the tasks in states of that kind, across
/// every project (400 for a missing or unknown kind).
///
/// The one tracker read that is not project-scoped: what is waiting for a
/// person spans the projects a user works. Required rather than defaulted,
/// because "every task in the installation" is not a question the dashboard
/// asks and not a list this endpoint is shaped to answer.
async fn dashboard(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Query(query): Query<StateKindQuery>,
) -> Result<Json<Vec<TaskDto>>> {
    let kind = match query.state_kind.as_deref() {
        Some(raw) => state_kind(raw)?,
        None => return Err(Error::BadRequest(STATE_KIND_REQUIRED.into())),
    };

    let tasks = TaskRepository::new(&state.pool);
    let rows = tasks.list_tasks_by_state_kind(kind).await?;

    Ok(Json(tasks.load_task_dtos_any_project(&rows).await?))
}

// ---- shared ----

/// A path or query value addressing a task, or 404.
///
/// [`TaskRef`]'s own rejection is 400 — it is a model rejecting malformed
/// input — but a URL segment that is neither a UUID nor a number addresses no
/// task, and `SPEC.md` answers an address that names nothing with 404.
fn task_ref(raw: &str) -> Result<TaskRef> {
    TaskRef::from_str(raw).map_err(|_| Error::NotFound)
}

/// One of the three state kinds, or 400 naming them.
fn state_kind(raw: &str) -> Result<TaskStateKind> {
    match raw {
        "queue" => Ok(TaskStateKind::Queue),
        "human" => Ok(TaskStateKind::Human),
        "terminal" => Ok(TaskStateKind::Terminal),
        other => Err(Error::BadRequest(format!(
            "unknown state kind \"{other}\"; valid kinds are: queue, human, terminal"
        ))),
    }
}
