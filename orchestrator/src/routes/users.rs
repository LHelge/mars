//! `/api/users` (`SPEC.md`, "Users (`/api/users`)").
//!
//! Six routes: the two a user points at themselves — `GET /users/me`, which is
//! exempt from the password-change gate so a user who has to change their
//! password can still be rendered, and `PATCH /users/me`, which sets the one
//! field they own — and the four an administrator points at anybody, plus the
//! read every authenticated user is allowed ("every user sees every user in
//! v1").
//!
//! The interesting part is the administrator-membership invariant: "At least
//! one administrator must remain. Both deleting an administrator and changing
//! `admin` from true to false are rejected with 409 if they would remove the
//! last administrator" (`SPEC.md`, "Users"). A count read before the
//! transaction proves nothing — two requests each demoting one of the final
//! two administrators would both see two — so both mutations follow the
//! sequence `docs/data-model.md`, "Users and authentication" prescribes:
//!
//! 1. `BEGIN`,
//! 2. [`UserRepository::lock_admin_membership`], the transaction-scoped
//!    advisory lock, as the *first* statement and before any user-row lock,
//! 3. [`UserRepository::lock_user`] on the target,
//! 4. [`UserRepository::count_admins`], read under the lock,
//! 5. the mutation, and `COMMIT` — or a rejection, which drops `tx` and rolls
//!    the whole thing back with no field changed.
//!
//! The second request to reach the lock therefore re-reads a count of one and
//! answers 409. Self-demotion is allowed when another administrator remains;
//! self-deletion is refused outright, before the transaction is even opened,
//! because no count can make it legal.
//!
//! Changing `admin` deliberately touches neither `auth_version` nor the
//! target's refresh tokens: a demotion takes effect through the current-user
//! check the extractors make on the next request, not by revoking a login (ADR
//! 0025). The demoted administrator keeps their tokens and their session; what
//! they lose is [`AdminUser`].

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use serde::Deserialize;
use uuid::Uuid;

use crate::models::{User, UserUpdate, Username};
use crate::prelude::*;
use crate::repositories::UserRepository;
use crate::routes::{AdminUser, CurrentUser, UngatedUser};

/// The 409 an administrator gets for demoting the last one (`SPEC.md`,
/// "Users").
const LAST_ADMIN_DEMOTION: &str = "cannot demote the last administrator";

/// The 409 an administrator gets for deleting the last one.
const LAST_ADMIN_DELETION: &str = "cannot delete the last administrator";

/// The 409 an administrator gets for deleting their own account. Separate from
/// [`LAST_ADMIN_DELETION`]: self-deletion is refused even when ten other
/// administrators remain.
const SELF_DELETION: &str = "cannot delete yourself";

/// The router nested under `/api/users`.
///
/// `/me` is registered before `/{id}`, so the literal segment wins over the
/// parameter and `GET /users/me` is never read as a lookup of the user whose
/// id is the string `me` (which would be a 400 from the `Uuid` path
/// rejection). The invites routes of the next task register their literal
/// `/invites` in front of `/{id}` for the same reason.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me", get(me).patch(update_me))
        .route("/", get(list))
        .route("/{id}", get(find).put(replace).delete(remove))
}

/// `GET /users/me` → the signed-in user.
///
/// [`UngatedUser`], because this is one of the two routes `SPEC.md`,
/// "Authentication" exempts from the password-change gate: the frontend reads
/// it to render the user it is about to send to the change-password page.
async fn me(UngatedUser(user): UngatedUser) -> Json<User> {
    Json(user)
}

/// `PATCH /users/me` (`{ notify_email? }`).
///
/// The only field a user may change about themselves here; `username` and
/// `admin` are an administrator's to set through [`replace`], and the password
/// has its own route. `deny_unknown_fields` is what makes that a 400 rather
/// than a silently ignored key — a client that sends `admin: true` here is
/// told it is wrong.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateMeRequest {
    notify_email: Option<bool>,
}

/// `PATCH /users/me` → the updated user.
///
/// An empty body (`{}`) is a no-op that answers the current row rather than
/// writing one: there is nothing to set, and a write would still move
/// `updated_at`.
async fn update_me(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Json(body): Json<UpdateMeRequest>,
) -> Result<Json<User>> {
    let update = UserUpdate {
        notify_email: body.notify_email,
        ..UserUpdate::default()
    };

    if update.is_empty() {
        return Ok(Json(user));
    }

    let users = UserRepository::new(&state.pool);
    let mut tx = state.pool.begin().await?;
    // The row was loaded by the extractor a moment ago, so a `None` here means
    // the user deleted themselves between the two; 404 is the honest answer.
    let updated = users
        .update(&mut tx, user.id, &update)
        .await?
        .ok_or(Error::NotFound)?;
    tx.commit().await?;

    debug!(user_id = %user.id, "profile updated");

    Ok(Json(updated))
}

/// `GET /users` → every user (administrators only).
async fn list(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> Result<Json<Vec<User>>> {
    Ok(Json(UserRepository::new(&state.pool).list().await?))
}

/// `GET /users/{id}` → one user, or 404.
///
/// Readable by any authenticated user: every user sees every user in v1, which
/// is what lets the UI name the creator of a session or the assignee of a task
/// without an administrator round trip.
async fn find(
    State(state): State<AppState>,
    CurrentUser(_caller): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<User>> {
    UserRepository::new(&state.pool)
        .find(id)
        .await?
        .map(Json)
        .ok_or(Error::NotFound)
}

/// `PUT /users/{id}` (`{ username, admin }`).
///
/// Both fields are required — this is a `PUT`, so the body is the new state of
/// the two administrator-settable fields, not a patch. `notify_email` is not
/// among them: it belongs to the user (see [`UpdateMeRequest`]).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceUserRequest {
    username: String,
    admin: bool,
}

/// `PUT /users/{id}` → the updated user (administrators only).
///
/// The sequence is the one in the module documentation, and its order is the
/// contract: the advisory lock first, then the target row, then the count, all
/// inside the transaction that performs the update. A rejection returns before
/// `commit`, so `tx` rolls back and the username in the same request is not
/// applied either.
///
/// The count is only consulted for a true → false transition. Promoting,
/// renaming or writing the same two values back cannot reduce the number of
/// administrators, so they never fail this check — but they still take the
/// lock, which is what makes a concurrent demotion wait for them rather than
/// counting around them.
async fn replace(
    State(state): State<AppState>,
    AdminUser(caller): AdminUser,
    Path(id): Path<Uuid>,
    Json(body): Json<ReplaceUserRequest>,
) -> Result<Json<User>> {
    let users = UserRepository::new(&state.pool);
    let mut tx = state.pool.begin().await?;

    users.lock_admin_membership(&mut tx).await?;

    let Some(current) = users.lock_user(&mut tx, id).await? else {
        return Err(Error::NotFound);
    };

    if current.admin && !body.admin && users.count_admins(&mut tx).await? <= 1 {
        debug!(user_id = %id, actor_id = %caller.id, "demotion refused: last administrator");
        return Err(Error::Conflict(LAST_ADMIN_DEMOTION.to_string()));
    }

    let update = UserUpdate {
        username: Some(Username::parse(&body.username)?),
        admin: Some(body.admin),
        ..UserUpdate::default()
    };

    // `lock_user` already proved the row exists and holds it, so `None` is
    // unreachable; `NotFound` rather than a panic all the same.
    let updated = users
        .update(&mut tx, id, &update)
        .await?
        .ok_or(Error::NotFound)?;
    tx.commit().await?;

    debug!(user_id = %id, actor_id = %caller.id, admin = updated.admin, "user updated");

    Ok(Json(updated))
}

/// `DELETE /users/{id}` → 204 (administrators only).
///
/// Self-deletion is refused before the transaction opens: it is not a question
/// about administrator membership — an ordinary administrator among five may
/// not delete themselves either — so there is nothing to lock or count.
///
/// Everything the deleted user owns follows the schema's foreign keys: their
/// refresh and password-reset tokens cascade away, while their invites,
/// projects, sessions, tasks and comments keep their rows with the user
/// reference set to NULL (`docs/data-model.md`). Their access tokens are not
/// revoked and do not have to be — the extractors fail to load the row on the
/// next request and answer 401 (ADR 0025). Sessions they started keep running;
/// deletion touches no container.
async fn remove(
    State(state): State<AppState>,
    AdminUser(caller): AdminUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    if id == caller.id {
        debug!(actor_id = %caller.id, "deletion refused: self");
        return Err(Error::Conflict(SELF_DELETION.to_string()));
    }

    let users = UserRepository::new(&state.pool);
    let mut tx = state.pool.begin().await?;

    users.lock_admin_membership(&mut tx).await?;

    let Some(target) = users.lock_user(&mut tx, id).await? else {
        return Err(Error::NotFound);
    };

    if target.admin && users.count_admins(&mut tx).await? <= 1 {
        debug!(user_id = %id, actor_id = %caller.id, "deletion refused: last administrator");
        return Err(Error::Conflict(LAST_ADMIN_DELETION.to_string()));
    }

    if !users.delete(&mut tx, id).await? {
        return Err(Error::NotFound);
    }
    tx.commit().await?;

    debug!(user_id = %id, actor_id = %caller.id, "user deleted");

    Ok(StatusCode::NO_CONTENT)
}
