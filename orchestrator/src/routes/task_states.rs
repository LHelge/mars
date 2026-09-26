//! `/api/projects/{pid}/task-states` (`SPEC.md`, "Task states
//! (`/api/projects/{pid}/task-states`)").
//!
//! The board's columns over REST: the list a client draws, and the three edits
//! that change it. Like the other project sub-resources, the `{pid}` capture
//! is part of these paths rather than of a `nest`, so this router is merged
//! onto the `/projects` prefix (`routes::mod`).
//!
//! **Any signed-in user may edit the columns.** `SPEC.md` gives all four rows
//! `JWT` and no administrator gate: a board is the project's, not the
//! installation's.
//!
//! **The rules are not here.** The name pattern is
//! [`TaskStateName::parse`]'s, the placement, the re-packing and every
//! conflict are the repository's, and the composition — resolve, change, emit
//! one `states_changed` with the full list — is
//! [`tracker::states`](crate::tracker::states)'s, which an MCP tool or a test
//! can call without HTTP. What this module decides is the HTTP shape:
//!
//! - **`{name}` is a name, not an id.** It is matched against
//!   `task_states.name` under the project lock, so a UUID in the path is a
//!   404 like any other name the project does not have;
//! - **`kind` is immutable, and saying so is worth a message.** A `PUT` body
//!   carrying `kind` — even the value the state already has — is 400 `kind is
//!   immutable` rather than a silently ignored field, which is why the
//!   request type has a `kind` it never reads;
//! - **an unparsable `kind` on `POST` is 400 `invalid state kind`.** The field
//!   arrives as raw JSON and is converted here, so the caller is told which
//!   field is wrong instead of receiving serde's description of the enum;
//! - **`GET` takes no lock and writes no event**, because a board read takes
//!   no part in anyone's mutation (ADR 0021). The project's existence is
//!   checked explicitly: an unknown project and a project whose states were
//!   somehow all removed would otherwise both answer `[]`.
//!
//! A rename or a deletion needs no extra handling for the agent profiles that
//! serve the state: `profile_states` links by id, so a renamed state keeps its
//! links and a profile's `serves_states` simply reports the new name, while a
//! deleted state's links cascade away and the profile serves one state fewer
//! (`docs/data-model.md`, `profile_states`).

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use serde::Deserialize;
use uuid::Uuid;

use crate::events::TaskActor;
use crate::models::{AutoMergeInput, TaskState, TaskStateKind, TaskStateName};
use crate::prelude::*;
use crate::repositories::{ProjectRepository, TaskRepository};
use crate::routes::CurrentUser;
use crate::tracker::states::{
    NewStateInput, StateUpdate, create_state, delete_state, update_state,
};
use crate::tracker::{TrackerMutation, retry_on_serialization_failure};

/// What a `kind` that is not one of the three is told (400).
const INVALID_KIND: &str = "invalid state kind";

/// What a `PUT` body carrying `kind` is told (400).
const KIND_IMMUTABLE: &str = "kind is immutable";

/// The router merged onto `/api/projects`.
///
/// One line per path, in the order of the table in `SPEC.md`, "Task states".
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/{pid}/task-states", get(list).post(create))
        .route(
            "/{pid}/task-states/{name}",
            axum::routing::put(update).delete(remove),
        )
}

// ---- create ----

/// `POST /projects/{pid}/task-states` (`SPEC.md`, "Task states").
///
/// `kind` is raw JSON so that anything that is not one of the three values
/// becomes [`INVALID_KIND`]; `deny_unknown_fields` for the reason the other
/// route modules give.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateStateRequest {
    name: String,
    kind: serde_json::Value,
    position: Option<i32>,
    #[serde(default)]
    auto_merge: bool,
    conflict_state: Option<String>,
}

/// `POST /projects/{pid}/task-states` → the new state (201; 400 for an invalid
/// name, kind or position or an invalid auto-merge pair, 404 for an unknown
/// project, 409 for a taken name or a second `human` state).
async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(pid): Path<Uuid>,
    Json(body): Json<CreateStateRequest>,
) -> Result<(StatusCode, Json<TaskState>)> {
    let kind: TaskStateKind = serde_json::from_value(body.kind)
        .map_err(|_| Error::BadRequest(INVALID_KIND.to_string()))?;
    // Parsed before the lock, so a body that was never going to be stored does
    // not queue behind the project's mutations.
    let name = TaskStateName::parse(&body.name)?;

    let actor = TaskActor::User { user_id: user.id };
    let name: String = name.into();
    let auto_merge = AutoMergeInput {
        auto_merge: body.auto_merge,
        conflict_state: body.conflict_state,
    };
    let created = retry_on_serialization_failure("create_task_state", || async {
        let mut mutation = TrackerMutation::begin(&state.pool, pid, actor).await?;
        let created = create_state(
            &mut mutation,
            NewStateInput {
                name: name.clone(),
                kind,
                position: body.position,
                auto_merge: auto_merge.clone(),
            },
        )
        .await?;
        mutation.commit().await?;

        Ok(created)
    })
    .await?;

    info!(project_id = %pid, state_id = %created.id, "task state created");

    Ok((StatusCode::CREATED, Json(created)))
}

// ---- list ----

/// `GET /projects/{pid}/task-states` → the project's states by `position` (404
/// for an unknown project).
async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(pid): Path<Uuid>,
) -> Result<Json<Vec<TaskState>>> {
    if ProjectRepository::new(&state.pool)
        .find(pid)
        .await?
        .is_none()
    {
        return Err(Error::NotFound);
    }

    Ok(Json(
        TaskRepository::new(&state.pool).list_states(pid).await?,
    ))
}

// ---- update ----

/// `PUT /projects/{pid}/task-states/{name}` (`SPEC.md`, "Task states").
///
/// `kind` exists only to be refused: it is never read, and its presence —
/// whatever the value — is [`KIND_IMMUTABLE`]. Without it,
/// `deny_unknown_fields` would answer the same 400 with serde's message about
/// an unknown field, which does not say why.
///
/// It is deserialised through [`present`] rather than as a plain
/// `Option<Value>`, because serde reads an explicit `"kind": null` as an
/// absent field and the rule is about the *key* being there, not about its
/// value.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateStateRequest {
    name: Option<String>,
    position: Option<i32>,
    #[serde(default, deserialize_with = "present")]
    kind: Option<serde_json::Value>,
    auto_merge: Option<bool>,
    /// `Some(None)` for an explicit `"conflict_state": null`, which gives the
    /// field as much as a name does: it replaces the pair.
    #[serde(default, deserialize_with = "present")]
    conflict_state: Option<Option<String>>,
}

impl UpdateStateRequest {
    /// The auto-merge pair this body replaces, or `None` when it gives
    /// neither field. Either field replaces both, so a missing `auto_merge`
    /// beside a `conflict_state` reads as `false` (`SPEC.md`, "Task states").
    fn auto_merge(&self) -> Option<AutoMergeInput> {
        if self.auto_merge.is_none() && self.conflict_state.is_none() {
            return None;
        }
        Some(AutoMergeInput {
            auto_merge: self.auto_merge.unwrap_or(false),
            conflict_state: self.conflict_state.clone().flatten(),
        })
    }
}

/// `Some(value)` whenever the field is in the body, `null` included.
///
/// serde only calls a `deserialize_with` for a key that is present, so the
/// `None` left by `default` means "absent" and nothing else.
fn present<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// `PUT /projects/{pid}/task-states/{name}` → the updated state (400 for an
/// invalid name, a negative position, any `kind` or an invalid auto-merge
/// pair; 404 for an unknown project or state; 409 for a taken name).
///
/// An empty body is a no-op: 200 with the state unchanged and no event.
async fn update(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((pid, name)): Path<(Uuid, String)>,
    Json(body): Json<UpdateStateRequest>,
) -> Result<Json<TaskState>> {
    if body.kind.is_some() {
        return Err(Error::BadRequest(KIND_IMMUTABLE.to_string()));
    }

    let actor = TaskActor::User { user_id: user.id };
    let update = StateUpdate {
        auto_merge: body.auto_merge(),
        name: body.name,
        position: body.position,
    };
    let updated = retry_on_serialization_failure("update_task_state", || async {
        let mut mutation = TrackerMutation::begin(&state.pool, pid, actor).await?;
        let (updated, changed) = update_state(&mut mutation, &name, update.clone()).await?;

        if changed {
            mutation.commit().await?;
            info!(project_id = %pid, state_id = %updated.id, "task state updated");
        } else {
            // Nothing was written, so there is nothing to commit and nothing to
            // announce; the 200 answers with the state as it stands.
            mutation.no_change().await?;
        }

        Ok(updated)
    })
    .await?;

    Ok(Json(updated))
}

// ---- delete ----

/// `DELETE /projects/{pid}/task-states/{name}` → 204 (404 for an unknown
/// project or state; 409 for the `human` state, the last `queue` state, the
/// last `terminal` state, a state tasks are still in, or a state another
/// state names as its conflict state).
async fn remove(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((pid, name)): Path<(Uuid, String)>,
) -> Result<StatusCode> {
    let actor = TaskActor::User { user_id: user.id };
    retry_on_serialization_failure("delete_task_state", || async {
        let mut mutation = TrackerMutation::begin(&state.pool, pid, actor).await?;
        delete_state(&mut mutation, &name).await?;
        mutation.commit().await
    })
    .await?;

    info!(project_id = %pid, state = %name, "task state deleted");

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_create_body_takes_the_documented_shape() {
        let body: CreateStateRequest =
            serde_json::from_value(json!({ "name": "qa", "kind": "queue" }))
                .expect("the minimal body parses");

        assert_eq!(body.name, "qa");
        assert_eq!(body.position, None);
        assert_eq!(
            serde_json::from_value::<TaskStateKind>(body.kind).unwrap(),
            TaskStateKind::Queue
        );
    }

    /// The field that exists only to be refused.
    #[test]
    fn an_update_body_keeps_the_kind_it_will_refuse() {
        let body: UpdateStateRequest = serde_json::from_value(json!({ "kind": "queue" }))
            .expect("a body carrying kind still parses");

        assert!(body.kind.is_some());

        let empty: UpdateStateRequest =
            serde_json::from_value(json!({})).expect("an empty body parses");

        assert_eq!(empty.name, None);
        assert_eq!(empty.position, None);
        assert!(empty.kind.is_none());
        assert_eq!(empty.auto_merge(), None);
    }

    /// Either field replaces both; neither leaves them alone.
    #[test]
    fn an_update_body_pairs_auto_merge_with_its_conflict_state() {
        let parse = |body| serde_json::from_value::<UpdateStateRequest>(body).unwrap();

        assert_eq!(
            parse(json!({ "auto_merge": false })).auto_merge(),
            Some(AutoMergeInput::default())
        );
        assert_eq!(
            parse(json!({ "conflict_state": null })).auto_merge(),
            Some(AutoMergeInput::default())
        );
        assert_eq!(
            parse(json!({ "conflict_state": "ready" })).auto_merge(),
            Some(AutoMergeInput {
                auto_merge: false,
                conflict_state: Some("ready".into()),
            })
        );
        assert_eq!(
            parse(json!({ "auto_merge": true, "conflict_state": "ready" })).auto_merge(),
            Some(AutoMergeInput {
                auto_merge: true,
                conflict_state: Some("ready".into()),
            })
        );
        assert_eq!(parse(json!({ "name": "qa" })).auto_merge(), None);
    }
}
