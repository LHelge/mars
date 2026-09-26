//! `/api/projects/{pid}/shared-dirs` (`SPEC.md`, "Shared directories
//! (`/api/projects/{pid}/shared-dirs`)").
//!
//! The four endpoints a project's shared directories are managed through, and
//! the two places they touch: the `project_shared_dirs` rows, through
//! [`ProjectRepository`], and the directories under
//! `DATA_DIR/projects/<pid>/shared/`, through [`ProjectLayout`]. The rules
//! themselves live elsewhere — the name and path rules in
//! [`NewSharedDir`](crate::models::NewSharedDir), the two 409s in the table's
//! primary key and unique index, and every path join in the layout — so what is
//! decided here is the order in which they are applied.
//!
//! **A create writes only the row.** The directory is created lazily at the
//! next launch (`ARCHITECTURE.md`, "Storage"; `docs/data-model.md`,
//! `project_shared_dirs`), so a project in `cloning` or `error` may be given
//! one: it takes effect when a session of the project next starts.
//!
//! **Clear and delete are refused while a session is live** — `running` or
//! `creating` — because a build in progress may hold files in the directory
//! open (`ARCHITECTURE.md`, "Storage"; `README.md`, "Operating notes"). That
//! refusal is a database check under the project row lock, and it is the whole
//! reason these two handlers open a transaction at all:
//!
//! ```text
//! BEGIN → lock_project_exclusive (missing → 404) → find_shared_dir (none → 404)
//!       → count_live_for_project (> 0 → rollback, 409)
//!       → delete_shared_dir (delete only) → COMMIT → filesystem
//! ```
//!
//! The filesystem work happens **after** the commit, never inside the
//! transaction: a `rm -rf` of a build directory can take seconds, and holding
//! the project row lock across it would block every tracker mutation of the
//! project for as long as it runs (ADR 0021). What that costs is one narrow
//! window, and both sides tolerate it — see [`clear`].
//!
//! **`{name}` is validated before it is used for anything**
//! ([`SharedDirName::parse`]), so a segment carrying `..` or a separator is the
//! model's 400 and never reaches a path join; [`ProjectLayout`] checks it a
//! second time anyway, because a stored name is only as trustworthy as the row
//! it came from.
//!
//! No task events and no git lock: a shared directory is not part of the
//! project's repository, and nothing in the tracker refers to one.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use serde::Deserialize;
use uuid::Uuid;

use crate::models::{NewSharedDir, SharedDir, SharedDirName};
use crate::prelude::*;
use crate::projects::ProjectLayout;
use crate::repositories::{ProjectRepository, SessionRepository};
use crate::routes::CurrentUser;

/// What a clear or a delete on a project with a live session is told (409).
///
/// One message for both, and for `running` and `creating` alike: which session
/// is in the way is the session list's to show (`SPEC.md`, "Shared
/// directories").
const RUNNING_SESSIONS: &str = "project has running sessions";

/// The router merged onto `/api/projects`.
///
/// The `{pid}` capture is part of these paths rather than of the `nest`, for
/// the reason [`crate::routes::git`] gives: axum panics on two `nest`s at one
/// path, so every project sub-resource carries its own prefix and is merged
/// beside the projects router (`routes::mod`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/{pid}/shared-dirs", get(list).post(create))
        .route("/{pid}/shared-dirs/{name}", delete(remove))
        .route("/{pid}/shared-dirs/{name}/clear", post(clear))
}

// ---- list ----

/// `GET /projects/{pid}/shared-dirs` → the project's shared directories, by
/// name (404 unknown project).
///
/// The project is looked up although the listing does not need it: an unknown
/// project has no rows, and answering `[]` for one would tell a client that a
/// project they cannot see is empty rather than absent (`SPEC.md`, "Shared
/// directories").
///
/// [`SharedDir`] is the response shape itself rather than a projection: it
/// serialises as exactly the documented `{ name, container_path, created_at }`,
/// because `project_id` — already in the URL — is `#[serde(skip)]` on the
/// model.
async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(pid): Path<Uuid>,
) -> Result<Json<Vec<SharedDir>>> {
    let projects = ProjectRepository::new(&state.pool);
    projects.find(pid).await?.ok_or(Error::NotFound)?;

    Ok(Json(projects.list_shared_dirs(pid).await?))
}

// ---- create ----

/// `POST /projects/{pid}/shared-dirs` (`{ name, container_path }`).
///
/// `deny_unknown_fields` for the reason the other route modules give: a client
/// that sends `created_at` here has misunderstood the endpoint — it is the
/// table's — and is told so rather than silently ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSharedDirBody {
    name: String,
    container_path: String,
}

/// `POST /projects/{pid}/shared-dirs` → the stored row (201; 400 for an
/// invalid name or path, 404 unknown project, 409 for a name or a mount point
/// already used in the project).
///
/// Nothing is created on disk: the row is configuration and the directory
/// appears at the next launch. Both halves are validated by
/// [`NewSharedDir::new`] before the transaction opens, and both 409s are the
/// repository's, decided by the constraint the insert broke rather than by a
/// lookup that another request could invalidate between the check and the
/// write.
async fn create(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(pid): Path<Uuid>,
    Json(body): Json<CreateSharedDirBody>,
) -> Result<(StatusCode, Json<SharedDir>)> {
    let dir = NewSharedDir::new(&body.name, &body.container_path)?;

    let mut tx = state.pool.begin().await?;
    let inserted = ProjectRepository::new(&state.pool)
        .insert_shared_dir(&mut tx, pid, &dir)
        .await?;
    tx.commit().await?;

    info!(project_id = %pid, name = %inserted.name, "shared directory added");

    Ok((StatusCode::CREATED, Json(inserted)))
}

// ---- clear ----

/// `POST /projects/{pid}/shared-dirs/{name}/clear` → 204 (400 for a name that
/// is not one, 404 unknown project or row, 409 while a session is live).
///
/// The directory is emptied and kept, which is what a caller reclaiming disk
/// space asks for; a directory that was never created — a row added since the
/// project's last launch — is the same 204, because the outcome the caller
/// asked for already holds.
///
/// **The narrow window.** A launch that committed its session row before
/// [`live_check`] read the count is seen and refuses this request. A launch
/// that commits *after* it may run [`ProjectLayout::ensure_shared_dir`] while
/// this handler is emptying the directory, because the filesystem work happens
/// after the commit released the project lock. Both sides tolerate that:
/// `ensure_shared_dir` is idempotent and `clear_shared_dir` removes whatever it
/// finds, so the worst outcome is a directory that exists and is empty, which
/// is what both callers wanted. It stays inside the documented contract —
/// "a running session keeps the mounts it started with" (`SPEC.md`, "Shared
/// directories") — because the session in that race has not started yet.
async fn clear(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((pid, name)): Path<(Uuid, String)>,
) -> Result<StatusCode> {
    let name = SharedDirName::parse(&name)?;

    live_check(&state, pid, &name, Removal::KeepRow).await?;

    let layout = state.config.project_layout(pid);
    if let Err(err) = layout.clear_shared_dir(name.as_str()).await {
        // The layout logged the path and the failing operation; this line adds
        // what it does not know — which project and which directory — so the
        // operator does not have to map a path back to a project id.
        log_failure(&layout, pid, &name, "clear", &err);

        // The caller can retry: the row is untouched and the directory is in
        // whatever state the failure left it (`SPEC.md`, "Shared directories").
        return Err(err);
    }

    info!(project_id = %pid, name = %name, "shared directory cleared");

    Ok(StatusCode::NO_CONTENT)
}

// ---- delete ----

/// `DELETE /projects/{pid}/shared-dirs/{name}` → 204 (400 for a name that is
/// not one, 404 unknown project or row, 409 while a session is live).
///
/// The row is deleted in the locked transaction and the directory is removed
/// after it commits. A filesystem failure here is **not** an error the caller
/// can do anything with — the row is gone, so a retry is a 404 — so it is
/// logged and answered 204: leftovers under `shared/` are an operator concern
/// (`ARCHITECTURE.md`, "Storage").
async fn remove(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((pid, name)): Path<(Uuid, String)>,
) -> Result<StatusCode> {
    let name = SharedDirName::parse(&name)?;

    live_check(&state, pid, &name, Removal::DeleteRow).await?;

    let layout = state.config.project_layout(pid);
    if let Err(err) = layout.remove_shared_dir(name.as_str()).await {
        log_failure(&layout, pid, &name, "remove", &err);
    }

    info!(project_id = %pid, name = %name, "shared directory deleted");

    Ok(StatusCode::NO_CONTENT)
}

// ---- the locked check both destructive endpoints share ----

/// Whether [`live_check`] also deletes the row it checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Removal {
    /// `clear`: the row stays and only the contents go.
    KeepRow,
    /// `delete`: the row goes in the same transaction that checked it.
    DeleteRow,
}

/// The transaction clear and delete share: lock the project, find the row,
/// refuse while a session is live, and — for a delete — remove the row.
///
/// Returns once the transaction has committed, so the caller does its
/// filesystem work with no database lock held.
///
/// The order is the documented one and each step owns one answer: an unknown
/// project is [`ProjectRepository::lock_project_exclusive`]'s 404, an unknown name is the
/// lookup's, and a live session is the 409 — so a clear of a name that does not
/// exist is 404 whether or not a session is running.
///
/// The lookup runs on the pool rather than on this transaction's connection,
/// which is the one read here that is not serialised by the lock. It does not
/// need to be: the project row is already locked, so the only thing that can
/// race it is a create or a delete of this very row, and both of those are
/// decided again below — a delete by [`ProjectRepository::delete_shared_dir`]
/// reporting that no row matched, and a clear by the filesystem, where an
/// empty or missing directory is the same 204.
async fn live_check(
    state: &AppState,
    pid: Uuid,
    name: &SharedDirName,
    removal: Removal,
) -> Result<()> {
    let projects = ProjectRepository::new(&state.pool);

    let mut tx = state.pool.begin().await?;
    projects.lock_project_exclusive(&mut tx, pid).await?;

    if projects
        .find_shared_dir(pid, name.as_str())
        .await?
        .is_none()
    {
        return Err(Error::NotFound);
    }

    let live = SessionRepository::new(&state.pool)
        .count_live_for_project(&mut tx, pid)
        .await?;
    if live > 0 {
        tx.rollback().await?;

        debug!(project_id = %pid, name = %name, live, "shared directory refused: live sessions");
        return Err(Error::Conflict(RUNNING_SESSIONS.to_string()));
    }

    if removal == Removal::DeleteRow
        && !projects
            .delete_shared_dir(&mut tx, pid, name.as_str())
            .await?
    {
        // The row was there a moment ago and is not any more: whoever deleted
        // it did this caller's work, and the answer is the one a second delete
        // of the same name gets.
        return Err(Error::NotFound);
    }

    tx.commit().await?;

    Ok(())
}

/// Log a filesystem failure with the two things the layout's own line lacks.
///
/// The path is an internal detail rather than a secret, and it belongs in the
/// log rather than in the response (`ARCHITECTURE.md`, "Orchestrator
/// internals", Errors); the error carries the layout's generic message, which
/// is what a 500 answers with.
fn log_failure(
    layout: &ProjectLayout,
    pid: Uuid,
    name: &SharedDirName,
    operation: &'static str,
    err: &Error,
) {
    let path = layout
        .shared_dir(name.as_str())
        .map(|path| path.display().to_string())
        .unwrap_or_default();

    error!(
        project_id = %pid,
        name = %name,
        path = %path,
        operation,
        error = %err,
        "shared directory filesystem operation failed"
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The documented body, and nothing else: a client sending a field the
    /// table owns is told rather than silently ignored.
    #[test]
    fn a_create_body_takes_the_documented_shape() {
        let body: CreateSharedDirBody = serde_json::from_value(json!({
            "name": "target",
            "container_path": "/session/work/target",
        }))
        .expect("the documented shape parses");

        assert_eq!(body.name, "target");
        assert_eq!(body.container_path, "/session/work/target");

        for raw in [
            json!({ "name": "target" }),
            json!({ "container_path": "/session/work/target" }),
            json!({
                "name": "target",
                "container_path": "/session/work/target",
                "created_at": "1970-01-01T00:00:00Z",
            }),
        ] {
            serde_json::from_value::<CreateSharedDirBody>(raw.clone())
                .expect_err(&format!("accepted {raw}"));
        }
    }

    /// The path segment goes through the model, so `..` is a 400 and never a
    /// directory name.
    #[test]
    fn a_path_segment_that_is_not_a_name_is_a_bad_request() {
        for raw in ["..", ".", "Target", "-x", "a/b", ""] {
            let error =
                Error::from(SharedDirName::parse(raw).expect_err(&format!("accepted {raw:?}")));

            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{raw:?}");
        }
    }
}
