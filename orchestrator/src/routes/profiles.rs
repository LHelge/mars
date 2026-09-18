//! `/api/projects/{pid}/profiles` (`SPEC.md`, "Agent profiles
//! (`/api/projects/{pid}/profiles`)").
//!
//! The five endpoints that turn the seeded `default` profile into a planner, a
//! reviewer or an ephemeral implementer, and add the others beside it. Like
//! [`crate::routes::git`], the `{pid}` capture is part of these paths rather
//! than of a `nest`, so this router is merged onto the `/projects` prefix
//! beside the projects and git routers (`routes::mod`).
//!
//! Almost nothing is decided here either. Every field rule — `bypass` and
//! nothing else, known MCP tool names, environment-variable secret names, the
//! lengths and the request defaults — belongs to [`ProfileInput::resolve`],
//! which is called **before** any transaction opens, so a body that was never
//! going to be stored does not queue behind the project lock. Every rule that
//! spans rows belongs to the repositories: the duplicate name and the
//! default-profile transfer to
//! [`ProjectRepository::update_profile`], the two refusals of a delete to
//! [`ProjectRepository::delete_profile`], and the "is that a `queue` state of
//! this project?" check to
//! [`TaskRepository::set_profile_states_by_name`]. Each already answers the
//! crate-wide [`Error`], whose `status()` is the documented code.
//!
//! What this module does decide:
//!
//! - **one mutation, one transaction, project row locked first.**
//!   [`TaskRepository::begin_mutation`] opens it and takes the lock, and an
//!   unknown project is its [`Error::NotFound`] before anything is written
//!   (`docs/data-model.md`, "Tracker mutation transactions", which lists
//!   profile served states among the mutations that belong under the lock).
//!   The link rows are always replaced, even when the caller sent the same
//!   names, so there is one path through both handlers;
//! - **`serves_states` is written after the row and read back after the
//!   commit.** `insert_profile` returns a row whose `serves_states` is empty
//!   and `update_profile` one whose links are the *old* ones, because both run
//!   before the link table is written. The response is therefore a re-read, so
//!   what the caller gets back is what is stored, in board order;
//! - **a profile of another project is 404, never 403** (`SPEC.md`, "REST
//!   API"): every repository call takes the project id as well as the profile
//!   id and puts both in the `WHERE` clause, so a foreign id simply matches no
//!   row. On `PUT` that answer is decided by `update_profile` *before* the
//!   states are resolved, which is what keeps a cross-project request with an
//!   unknown state name a 404 rather than a 400.
//!
//! No `task_events` row is written by any of this: a profile is project
//! configuration, not a tracker row, and the board has nothing to redraw
//! (`SPEC.md`, "TaskEvent").

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use uuid::Uuid;

use crate::models::{AgentProfile, ProfileInput};
use crate::prelude::*;
use crate::repositories::{ProjectRepository, TaskRepository};
use crate::routes::{CurrentUser, Path};

/// The router nested under `/api/projects`.
///
/// One line per path, in the order of the table in `SPEC.md`, "Agent
/// profiles".
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/{pid}/profiles", get(list).post(create))
        .route(
            "/{pid}/profiles/{id}",
            get(fetch).put(update).delete(remove),
        )
}

/// The `Profile` of `SPEC.md`, "Agent profiles", as the row itself.
///
/// There is no projection here, unlike `routes::projects`: [`AgentProfile`]
/// serialises to exactly `{ id, project_id, name, kind, backend, model,
/// system_prompt, permission_mode, image, runtime, mcp_tools, secrets,
/// serves_states, partial_messages, idle_timeout_secs, is_default, created_at,
/// updated_at }` — every documented field, no field the contract does not have
/// and nothing skipped — so a DTO would be the same eighteen fields written
/// twice. `tests/profiles.rs` asserts the key set against a real response, so
/// a column added to the model without a line in `SPEC.md` fails there.
type Profile = AgentProfile;

// ---- list ----

/// `GET /projects/{pid}/profiles` → the project's profiles, oldest first (404
/// for an unknown project).
///
/// The existence check is explicit because an unknown project and a project
/// without profiles would otherwise both answer `[]`, and only one of them is
/// the documented answer.
async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(pid): Path<Uuid>,
) -> Result<Json<Vec<Profile>>> {
    let projects = ProjectRepository::new(&state.pool);
    if projects.find(pid).await?.is_none() {
        return Err(Error::NotFound);
    }

    Ok(Json(projects.list_profiles(pid).await?))
}

// ---- create ----

/// `POST /projects/{pid}/profiles` (`ProfileInput`) → the stored profile (201;
/// 400 for any invalid field or a `serves_states` entry that is not a `queue`
/// state, 404 for an unknown project, 409 for a name that is taken).
///
/// The body is resolved first, so the 400s cost no lock, and the insert and
/// the served states are one transaction: a profile that is stored without its
/// link rows would serve nothing until someone edited it again.
async fn create(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(pid): Path<Uuid>,
    Json(body): Json<ProfileInput>,
) -> Result<(StatusCode, Json<Profile>)> {
    let profile = body.resolve_new(pid, &state.config)?;

    let projects = ProjectRepository::new(&state.pool);
    let tasks = TaskRepository::new(&state.pool);

    let mut tx = tasks.begin_mutation(pid).await?;
    let inserted = projects.insert_profile(&mut tx, &profile).await?;
    tasks
        .set_profile_states_by_name(&mut tx, pid, inserted.id, &profile.serves_states)
        .await?;
    tx.commit().await?;

    info!(project_id = %pid, profile_id = %inserted.id, "profile created");

    Ok((
        StatusCode::CREATED,
        Json(stored(&state, pid, inserted.id).await?),
    ))
}

// ---- fetch ----

/// `GET /projects/{pid}/profiles/{id}` → the profile, or 404.
///
/// The project id is half the `WHERE` clause, so a profile of another project
/// is the same 404 as one that does not exist.
async fn fetch(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((pid, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Profile>> {
    let profile = ProjectRepository::new(&state.pool)
        .find_profile(pid, id)
        .await?
        .ok_or(Error::NotFound)?;

    Ok(Json(profile))
}

// ---- update ----

/// `PUT /projects/{pid}/profiles/{id}` (`ProfileInput`) → the stored profile
/// (400 invalid, 404 unknown, 409 a name that is taken or a default that would
/// be cleared).
///
/// A **full replacement**: the same `ProfileInput` as `POST`, so an omitted
/// field takes its documented default again rather than keeping the stored
/// value — `serves_states` goes back to `["ready"]`, `mcp_tools` and `secrets`
/// to empty, `model` and `runtime` to null. `is_default` is the one exception
/// and [`ProjectRepository::update_profile`] owns it: absent leaves the flag,
/// `true` takes it from the current holder in this transaction, `false` on the
/// current default is the documented 409 (`SPEC.md`, "Agent profiles").
async fn update(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((pid, id)): Path<(Uuid, Uuid)>,
    Json(body): Json<ProfileInput>,
) -> Result<Json<Profile>> {
    let resolved = body.resolve(&state.config)?;

    let projects = ProjectRepository::new(&state.pool);
    let tasks = TaskRepository::new(&state.pool);

    let mut tx = tasks.begin_mutation(pid).await?;
    // Before the states are resolved, so a profile of another project is a 404
    // and not the 400 an unknown state name would otherwise win.
    let updated = projects
        .update_profile(&mut tx, pid, id, &resolved)
        .await?
        .ok_or(Error::NotFound)?;
    tasks
        .set_profile_states_by_name(&mut tx, pid, updated.id, &resolved.serves_states)
        .await?;
    tx.commit().await?;

    info!(project_id = %pid, profile_id = %id, "profile updated");

    Ok(Json(stored(&state, pid, id).await?))
}

// ---- delete ----

/// `DELETE /projects/{pid}/profiles/{id}` → 204 (404 unknown, 409 for the
/// default profile or one that has sessions).
///
/// Both refusals are read and answered inside the transaction that holds the
/// project lock, so a session launched on the profile while the request was in
/// flight is either counted or waits for the lock — it cannot slip between the
/// count and the `DELETE`.
async fn remove(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path((pid, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let projects = ProjectRepository::new(&state.pool);
    let tasks = TaskRepository::new(&state.pool);

    let mut tx = tasks.begin_mutation(pid).await?;
    if !projects.delete_profile(&mut tx, pid, id).await? {
        // Dropping the transaction rolls it back and releases the lock.
        return Err(Error::NotFound);
    }
    tx.commit().await?;

    info!(project_id = %pid, profile_id = %id, "profile deleted");

    Ok(StatusCode::NO_CONTENT)
}

/// The committed profile, read back after a mutation.
///
/// Both writes leave the row they return with the wrong `serves_states` — the
/// insert with none, because the link rows are written after it, and the
/// update with the ones the profile served *before* the same statement pair
/// replaced them. One extra read is what makes the response the stored truth
/// in board order rather than an assembly of what the handler asked for.
async fn stored(state: &AppState, pid: Uuid, id: Uuid) -> Result<Profile> {
    ProjectRepository::new(&state.pool)
        .find_profile(pid, id)
        .await?
        .ok_or(Error::NotFound)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::json;

    use super::*;
    use crate::models::{AgentBackend, PERMISSION_MODE_BYPASS, ProfileKind};

    /// Not a real image: the stub the tests use everywhere (`CLAUDE.md`, rule
    /// 3).
    const TEST_IMAGE: &str = "mars-session-stub:test";

    /// The documented eighteen fields, and nothing else.
    ///
    /// The response type is the model itself, so this is the assertion that it
    /// may stay that way: a column added to [`AgentProfile`] without a line in
    /// `SPEC.md`, "Agent profiles" shows up here as an extra key.
    #[test]
    fn the_response_carries_exactly_the_documented_fields() {
        let row = Profile {
            id: Uuid::from_u128(1),
            project_id: Uuid::from_u128(2),
            name: "planner".to_string(),
            kind: ProfileKind::Conversational,
            backend: AgentBackend::Claude,
            model: None,
            system_prompt: Some("plan carefully".to_string()),
            permission_mode: PERMISSION_MODE_BYPASS.to_string(),
            image: TEST_IMAGE.to_string(),
            runtime: None,
            mcp_tools: vec!["ready".to_string()],
            secrets: Vec::new(),
            serves_states: vec!["backlog".to_string()],
            partial_messages: true,
            idle_timeout_secs: 1800,
            is_default: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let rendered = serde_json::to_value(&row).expect("a profile serialises");

        assert_eq!(
            rendered,
            json!({
                "id": row.id,
                "project_id": row.project_id,
                "name": "planner",
                "kind": "conversational",
                "backend": "claude",
                "model": null,
                "system_prompt": "plan carefully",
                "permission_mode": "bypass",
                "image": TEST_IMAGE,
                "runtime": null,
                "mcp_tools": ["ready"],
                "secrets": [],
                "serves_states": ["backlog"],
                "partial_messages": true,
                "idle_timeout_secs": 1800,
                "is_default": false,
                "created_at": row.created_at,
                "updated_at": row.updated_at,
            })
        );
    }

    /// The body is the model's own, so the only thing to assert here is that
    /// the documented minimal form reaches it: a name and nothing else.
    #[test]
    fn a_create_body_takes_the_documented_shape() {
        let minimal: ProfileInput =
            serde_json::from_value(json!({ "name": "planner" })).expect("the minimal body parses");

        assert_eq!(minimal.name, "planner");
        assert_eq!(minimal.kind, None);
        assert_eq!(minimal.serves_states, None);
        assert_eq!(minimal.is_default, None);
    }
}
