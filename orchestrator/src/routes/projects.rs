//! `/api/projects` (`SPEC.md`, "Projects (`/api/projects`)").
//!
//! The project's own endpoints: the list, the create, the read, the edit, the
//! delete, the clone retry, the on-demand mirror fetch and the branch listing.
//! The git sub-resource lives in [`crate::routes::git`] and is merged onto the
//! same `/projects` prefix (`routes::mod`).
//!
//! Almost nothing is decided here. Creating a project is one transaction in
//! [`create_project`], which validates every field, seeds the task states and
//! the default profile and stores the credential; the background clone is
//! [`clone_job::spawn`], started **after** that transaction committed, because
//! the job re-reads the row it is about to work on. `PUT` hands its fields to
//! the models, which are what turn a bad name, branch or attempt budget into
//! the documented 400, and `retry-clone` is [`ProjectRepository::mark_cloning_from_error`],
//! whose `status = 'error'` guard is in the `WHERE` clause so two concurrent
//! retries cannot both start a job. `DELETE` is [`delete_project`], which takes
//! the project git lock, refuses with 409 while a session of the project is
//! live, and removes the rows and the directories under that lock.
//!
//! What this module does decide:
//!
//! - an empty `PUT` body is a 200 that returns the current row and writes
//!   nothing, which is what a client re-sending an unedited form sends;
//! - a `PUT` that moves `default_branch` on a `ready` project takes the project
//!   git lock, checks the new name against the repository
//!   ([`verify_default_branch`]), writes the row in a transaction, moves the
//!   bare `HEAD` ([`set_default_branch`]) and only then commits, so a row write
//!   that still fails — a taken name, a project deleted in between — cannot
//!   leave `HEAD` naming a branch the row does not, and a `HEAD` that cannot be
//!   moved rolls the row back. The lock is taken before the transaction opens,
//!   never while one is (ADR 0021). On a `cloning` or `error` project the
//!   value is stored unchecked, because there may be no repository yet and the
//!   clone job validates it against the fetched heads;
//! - `mark_cloning_from_error` answering `None` is 404 or 409, told apart by a
//!   follow-up read: the row is either gone or in another status;
//! - `GET .../branches` refuses a project that is not `ready` with the same
//!   409 the fetch routine gives, and holds the project git lock across the
//!   one listing command so a half-seeded clone is never reported.
//!
//! **The credential never comes back out.** It reaches [`NewProjectRequest`]
//! and is sealed into the project-scoped `GIT_CREDENTIAL` secret; the response
//! is [`ProjectDto`], which has no field for it, and [`CreateProjectBody`]
//! derives no [`std::fmt::Debug`], so no log line, span field or `#[instrument]`
//! on this path can render one (`CLAUDE.md`, rule 3; `SPEC.md`, "Projects").

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::git::{
    DataPaths, GitActor, GitError, fetch_project, list_branches, set_default_branch,
    verify_default_branch,
};
use crate::models::{
    Branch, BranchName, MaxAttempts, Project, ProjectName, ProjectStatus, ProjectUpdate,
};
use crate::prelude::*;
use crate::projects::{NewProjectRequest, clone_job, create_project, delete_project};
use crate::repositories::ProjectRepository;
use crate::routes::{CurrentUser, Path};

/// What a `retry-clone` on a project that is not in `error` is told (409).
const NOT_IN_ERROR: &str = "project is not in error state";

/// What a request needing a repository on disk is told while there may be none
/// (409).
///
/// The same words [`fetch_project`] answers a `cloning` or `error` project
/// with, so `POST .../fetch` and `GET .../branches` cannot drift apart in how
/// they say the same thing.
const NOT_READY: &str = "project is not ready";

/// The router nested under `/api/projects`.
///
/// One line per path, in the order of the table in `SPEC.md`, "Projects"; the
/// git routes carry their own `{pid}/git/…` paths and are merged onto this
/// prefix beside these (`routes::mod`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(fetch).put(update).delete(remove))
        .route("/{id}/retry-clone", post(retry_clone))
        .route("/{id}/fetch", post(fetch_now))
        .route("/{id}/branches", get(branches))
}

/// `Project = { id, name, remote_url, default_branch, status, status_message,
/// last_fetched_at, max_attempts, created_at, has_credential }` (`SPEC.md`,
/// "Projects").
///
/// A projection rather than the row, which also carries `created_by`,
/// `next_task_number` and `updated_at`: the first two are internal bookkeeping
/// and the third is not part of the documented shape, so the response is
/// written from the contract rather than from whatever columns the table
/// happens to have. There is no `credential` field to forget to remove.
#[derive(Debug, Serialize)]
struct ProjectDto {
    id: Uuid,
    name: String,
    remote_url: String,
    default_branch: Option<String>,
    status: ProjectStatus,
    status_message: Option<String>,
    last_fetched_at: Option<DateTime<Utc>>,
    max_attempts: i16,
    created_at: DateTime<Utc>,
    has_credential: bool,
}

impl From<Project> for ProjectDto {
    fn from(project: Project) -> Self {
        Self {
            id: project.id,
            name: project.name,
            remote_url: project.remote_url,
            default_branch: project.default_branch,
            status: project.status,
            status_message: project.status_message,
            last_fetched_at: project.last_fetched_at,
            max_attempts: project.max_attempts,
            created_at: project.created_at,
            has_credential: project.has_credential,
        }
    }
}

// ---- list ----

/// `GET /projects` → every project, oldest first.
async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
) -> Result<Json<Vec<ProjectDto>>> {
    let projects = ProjectRepository::new(&state.pool).list().await?;

    Ok(Json(projects.into_iter().map(ProjectDto::from).collect()))
}

// ---- create ----

/// `POST /projects` (`{ name, remote_url, default_branch?, credential? }`).
///
/// No `Debug`, deliberately: `credential` is the remote's password or token,
/// and a derived one would put it into every `?`-formatted tracing field that
/// ever carried this struct (rule 3). [`NewProjectRequest`] has a hand-written
/// one for the same reason, and nothing on this path is `#[instrument]`ed.
///
/// `deny_unknown_fields` for the reason the other route modules give: a client
/// that sends `status` or `max_attempts` here has misunderstood the endpoint —
/// the first is the clone job's and the second is `PUT`'s — and is told so
/// rather than silently ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateProjectBody {
    name: String,
    remote_url: String,
    /// Absent means "discover it from the remote's `HEAD`", which the clone job
    /// does (`SPEC.md`, "Projects").
    default_branch: Option<String>,
    /// Stored as the project-scoped, orchestrator-only secret `GIT_CREDENTIAL`
    /// and never returned. Allowed on a public repository too; it is simply
    /// stored.
    credential: Option<String>,
}

/// `POST /projects` → the created project, `cloning` (201; 400 for any invalid
/// field, 409 for a name that is taken).
///
/// The clone job is spawned after [`create_project`] committed and named with
/// the requesting user, who becomes the `secret_uses` actor of its credential
/// lookup (`docs/data-model.md`, `secret_uses`).
async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<CreateProjectBody>,
) -> Result<(StatusCode, Json<ProjectDto>)> {
    let project = create_project(
        &state,
        NewProjectRequest {
            name: body.name,
            remote_url: body.remote_url,
            default_branch: body.default_branch,
            credential: body.credential,
        },
        user.id,
    )
    .await?;

    clone_job::spawn(state.clone(), project.id, Some(user.id));
    info!(project_id = %project.id, "project clone started");

    Ok((StatusCode::CREATED, Json(project.into())))
}

// ---- fetch ----

/// `GET /projects/{id}` → the project, or 404.
async fn fetch(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ProjectDto>> {
    let project = ProjectRepository::new(&state.pool)
        .find(id)
        .await?
        .ok_or(Error::NotFound)?;

    Ok(Json(project.into()))
}

// ---- update ----

/// `PUT /projects/{id}` (`{ name?, default_branch?, max_attempts? }`).
///
/// Every field optional and `None` meaning "leave it alone", so `{}` is legal
/// and answers the current row. `remote_url` is not among them — the mirror on
/// disk was cloned from it, so pointing a project at another remote is a new
/// project — and neither are `status`, `status_message` and `last_fetched_at`,
/// which are the clone and fetch jobs' (`SPEC.md`, "Projects"). Sending one of
/// those is a 400 rather than a silent no-op on a field the caller believes
/// they just changed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateProjectBody {
    name: Option<String>,
    default_branch: Option<String>,
    max_attempts: Option<i16>,
}

impl UpdateProjectBody {
    /// The validated update, or the model's own 400.
    ///
    /// The three models are where the rules live, so a name of 101 characters,
    /// a branch git would not store and an attempt budget outside 1–20 are
    /// rejected here with the same message they would get anywhere else
    /// (`CLAUDE.md`, "Backend conventions").
    fn resolve(&self) -> Result<ProjectUpdate> {
        Ok(ProjectUpdate {
            name: self.name.as_deref().map(ProjectName::parse).transpose()?,
            default_branch: self
                .default_branch
                .as_deref()
                .map(BranchName::parse)
                .transpose()?,
            max_attempts: self.max_attempts.map(MaxAttempts::parse).transpose()?,
        })
    }
}

/// `PUT /projects/{id}` → the stored project (400 invalid, 404 unknown, 409 a
/// name that is taken).
///
/// The order: validate the body, load the project, and — when the update moves
/// `default_branch` on a `ready` project — take the project git lock, check the
/// new name against the repository, write the row inside a transaction, move
/// the bare `HEAD` and only then commit. Nothing irreversible happens before
/// the row write can no longer fail: a colliding name (409) or a project
/// deleted in between (404) aborts with `HEAD` untouched, and a `HEAD` that
/// cannot be moved rolls the row write back, so the repository and the row
/// cannot disagree.
///
/// The git lock is taken before the transaction opens and held across it, so no
/// transaction is ever open while *waiting* for a git lock and the lock order
/// stays git-before-database (ADR 0021; `ARCHITECTURE.md`, "Git model",
/// Serialization). Holding it also settles the race with `DELETE`, which takes
/// the same lock first: once this handler holds it the deletion either already
/// happened, and the row write answers 404, or it waits.
async fn update(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateProjectBody>,
) -> Result<Json<ProjectDto>> {
    let update = body.resolve()?;

    let projects = ProjectRepository::new(&state.pool);
    let project = projects.find(id).await?.ok_or(Error::NotFound)?;

    // Nothing to write: answer the row as it is rather than moving
    // `updated_at` for an edit that changes no field.
    if update.is_empty() {
        return Ok(Json(project.into()));
    }

    let moved = moved_default_branch(&project, &update);
    let paths = DataPaths::from_config(&state.config);

    // The lock is only taken when there is git work, and always before the
    // transaction below opens.
    let guard = match moved {
        Some(branch) => {
            let guard = state.git_locks.lock(id).await;

            verify_default_branch(&guard, &paths, branch)
                .await
                .map_err(|error| match error {
                    // The repository has no such integration head. The name is
                    // the caller's, so this is their 400 and not the git
                    // layer's 500.
                    GitError::UnknownRef(_) => Error::BadRequest(format!(
                        "default_branch {branch:?} is not an integration head of this project"
                    )),
                    other => Error::from(other),
                })?;

            Some(guard)
        }
        None => None,
    };

    let mut tx = state.pool.begin().await?;

    // A 409 on a taken name or a 404 on a row deleted in between leaves the
    // transaction to roll back with `HEAD` never touched.
    let updated = projects
        .update(&mut tx, id, &update)
        .await?
        .ok_or(Error::NotFound)?;

    if let (Some(branch), Some(guard)) = (moved, guard.as_ref())
        && let Err(error) = set_default_branch(guard, &paths, branch).await
    {
        tx.rollback().await?;
        return Err(Error::from(error));
    }

    tx.commit().await?;

    info!(project_id = %id, "project updated");

    Ok(Json(updated.into()))
}

/// The integration head this update moves `HEAD` to, or `None` when there is no
/// git work to do.
///
/// Three ways there is none: the body named no `default_branch`, it named the
/// one the project already has, or the project is not `ready` — which means
/// there may be no repository on disk at all, and the clone job is what
/// validates the stored name against the fetched heads (`SPEC.md`, "Projects").
fn moved_default_branch<'a>(project: &Project, update: &'a ProjectUpdate) -> Option<&'a str> {
    let branch = update.default_branch.as_ref()?.as_str();

    (project.status == ProjectStatus::Ready && project.default_branch.as_deref() != Some(branch))
        .then_some(branch)
}

// ---- retry-clone ----

/// `POST /projects/{id}/retry-clone` → the project, `cloning` again (404
/// unknown, 409 from any other status).
///
/// The transition is one guarded `UPDATE`, so two concurrent retries cannot
/// both win: the second matches no row, finds the project `cloning`, and is the
/// documented 409. The job is spawned after the transaction committed, for the
/// reason [`create`] gives.
async fn retry_clone(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ProjectDto>> {
    let projects = ProjectRepository::new(&state.pool);

    let mut tx = state.pool.begin().await?;
    let retried = projects.mark_cloning_from_error(&mut tx, id).await?;

    let Some(project) = retried else {
        tx.rollback().await?;

        // `None` is "no row matched", which is either of two answers; the row
        // itself says which.
        return Err(match projects.find(id).await? {
            Some(_) => Error::Conflict(NOT_IN_ERROR.to_string()),
            None => Error::NotFound,
        });
    };
    tx.commit().await?;

    clone_job::spawn(state.clone(), project.id, Some(user.id));
    info!(project_id = %id, "project clone retried");

    Ok(Json(project.into()))
}

// ---- fetch and branches ----

/// `POST /projects/{id}/fetch` → the project with a fresh `last_fetched_at`
/// (404 unknown, 409 not `ready`, and the git layer's own status when the
/// fetch itself failed).
///
/// One call into [`fetch_project`], which is the routine the cron mirror-fetch
/// job and a fresh session launch run too (`ARCHITECTURE.md`, "Git model",
/// Project clone): the readiness check before the lock, the git lock held for
/// exactly the `git fetch --prune origin`, the credential lookup recorded
/// against this user in `secret_uses`, and the `last_fetched_at` write in a
/// short transaction after the lock was released (ADR 0021).
///
/// `max_age` is `None`, deliberately. It is what the launcher passes a budget
/// for; an explicit `POST .../fetch` is a user asking for upstream *now* and
/// must never be answered by a fetch somebody else ran a moment ago.
///
/// The row is re-read rather than returned by the fetch, which answers a
/// timestamp and not a project. A project deleted between the two is the
/// documented 404, which is also what a client racing a deletion should see.
async fn fetch_now(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ProjectDto>> {
    fetch_project(&state, id, &GitActor::User(user.id), None).await?;

    let project = ProjectRepository::new(&state.pool)
        .find(id)
        .await?
        .ok_or(Error::NotFound)?;

    Ok(Json(project.into()))
}

/// `GET /projects/{id}/branches` → every ref the API calls a branch (404
/// unknown, 409 not `ready`).
///
/// [`list_branches`] is the whole answer: the integration heads, the
/// upstream-tracking refs and the session refs, in that order and by name
/// within each, with tags, hand-off refs and `origin/HEAD` left out
/// (`SPEC.md`, "Projects").
///
/// The status check comes first and is the same 409 a fetch gives, because a
/// project that is still `cloning` or in `error` may have no repository to
/// read at all. The project git lock is then held across the one
/// `for-each-ref`, so the listing cannot catch the clone job part-way through
/// seeding the integration heads (`ARCHITECTURE.md`, "Git model",
/// Serialization). No transaction is open while that lock is waited for: the
/// row was read and dropped before it (ADR 0021).
async fn branches(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Branch>>> {
    let project = ProjectRepository::new(&state.pool)
        .find(id)
        .await?
        .ok_or(Error::NotFound)?;

    if project.status != ProjectStatus::Ready {
        return Err(Error::Conflict(NOT_READY.to_string()));
    }

    let paths = DataPaths::from_config(&state.config);
    let guard = state.git_locks.lock(id).await;
    let listed = list_branches(&paths, id).await;
    drop(guard);

    Ok(Json(listed?))
}

// ---- delete ----

/// `DELETE /projects/{id}` → 204 (404 unknown, 409 while any session of the
/// project is `running` or `creating`).
///
/// Named `remove` because `delete` is the routing method this handler is
/// registered with. Everything it does is [`delete_project`]: the git lock,
/// the one transaction that refuses or deletes, and the directories afterwards
/// (`SPEC.md`, "Projects"; `ARCHITECTURE.md`, "Storage").
async fn remove(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    delete_project(&state, id).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A project row with the fields these tests care about; the rest are the
    /// defaults a freshly created project has.
    fn project(status: ProjectStatus, default_branch: Option<&str>) -> Project {
        Project {
            id: Uuid::from_u128(1),
            name: "mars".to_string(),
            remote_url: "https://example.invalid/org/repo.git".to_string(),
            default_branch: default_branch.map(str::to_string),
            status,
            status_message: None,
            created_by: None,
            last_fetched_at: None,
            max_attempts: 3,
            next_task_number: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            has_credential: false,
        }
    }

    /// The documented ten fields, and none of the three the row adds.
    #[test]
    fn the_response_carries_exactly_the_documented_fields() {
        let row = project(ProjectStatus::Cloning, None);
        let rendered =
            serde_json::to_value(ProjectDto::from(row.clone())).expect("the projection serialises");

        assert_eq!(
            rendered,
            json!({
                "id": row.id,
                "name": "mars",
                "remote_url": row.remote_url,
                "default_branch": null,
                "status": "cloning",
                "status_message": null,
                "last_fetched_at": null,
                "max_attempts": 3,
                "created_at": row.created_at,
                "has_credential": false,
            })
        );
    }

    /// The create body holds a credential, so the one thing a unit test can
    /// prove about it without a database is that the documented shape parses
    /// and that the optional halves may be left out. The value is an obviously
    /// fake credential (rule 3).
    #[test]
    fn a_create_body_takes_the_documented_shape() {
        let minimal: CreateProjectBody = serde_json::from_value(json!({
            "name": "mars",
            "remote_url": "https://example.invalid/org/repo.git",
        }))
        .expect("the minimal body parses");

        assert_eq!(minimal.name, "mars");
        assert_eq!(minimal.default_branch, None);
        assert_eq!(minimal.credential, None);

        let full: CreateProjectBody = serde_json::from_value(json!({
            "name": "mars",
            "remote_url": "https://example.invalid/org/repo.git",
            "default_branch": "release/2.0",
            "credential": "fake-git-credential-for-tests",
        }))
        .expect("the full body parses");

        assert_eq!(full.default_branch.as_deref(), Some("release/2.0"));
        assert_eq!(
            full.credential.as_deref(),
            Some("fake-git-credential-for-tests")
        );
    }

    /// An empty `PUT` body is legal and changes nothing.
    #[test]
    fn an_empty_update_body_resolves_to_an_empty_update() {
        let body: UpdateProjectBody = serde_json::from_value(json!({})).expect("`{}` parses");

        assert!(body.resolve().expect("nothing to validate").is_empty());
    }

    /// Each field goes through its own model, which is where the 400 comes
    /// from.
    #[test]
    fn every_invalid_field_is_a_bad_request() {
        for raw in [
            json!({ "name": "  " }),
            json!({ "name": "m".repeat(101) }),
            json!({ "default_branch": "refs/heads/main" }),
            json!({ "max_attempts": 0 }),
            json!({ "max_attempts": 21 }),
        ] {
            let body: UpdateProjectBody =
                serde_json::from_value(raw.clone()).expect("the body parses");
            let error = body.resolve().expect_err("the value is refused");

            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{raw}");
        }
    }

    /// The git half of `PUT` runs only for a `ready` project whose
    /// `default_branch` actually moves.
    #[test]
    fn the_head_moves_only_for_a_ready_project_that_changed_branch() {
        let change = UpdateProjectBody {
            name: None,
            default_branch: Some("release/2.0".to_string()),
            max_attempts: None,
        }
        .resolve()
        .expect("the branch name is valid");

        assert_eq!(
            moved_default_branch(&project(ProjectStatus::Ready, Some("main")), &change),
            Some("release/2.0")
        );

        // The same value it already has, and a project that has no repository
        // to check against yet.
        assert_eq!(
            moved_default_branch(&project(ProjectStatus::Ready, Some("release/2.0")), &change),
            None
        );
        for status in [ProjectStatus::Cloning, ProjectStatus::Error] {
            assert_eq!(
                moved_default_branch(&project(status, Some("main")), &change),
                None,
                "{status:?}"
            );
        }

        // An update that names no branch at all.
        let rename = UpdateProjectBody {
            name: Some("phobos".to_string()),
            default_branch: None,
            max_attempts: None,
        }
        .resolve()
        .expect("the name is valid");
        assert_eq!(
            moved_default_branch(&project(ProjectStatus::Ready, Some("main")), &rename),
            None
        );
    }
}
