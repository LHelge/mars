//! `/api/users` (`SPEC.md`, "Users (`/api/users`)").
//!
//! The two routes a user points at themselves — `GET /users/me`, which is
//! exempt from the password-change gate so a user who has to change their
//! password can still be rendered, and `PATCH /users/me`, which sets the one
//! field they own — the ones an administrator points at anybody, the read
//! every authenticated user is allowed ("every user sees every user in v1"),
//! and `POST /users/{id}/password`, which is either of the first two kinds
//! depending on whose id it is given and is the other route the gate exempts.
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
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::routing::post;
use axum_extra::extract::CookieJar;
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::models::{OpaqueToken, Password, User, UserUpdate, Username};
use crate::prelude::*;
use crate::repositories::UserRepository;
use crate::routes::auth::{TokenPairResponse, hash_blocking, verify_blocking};
use crate::routes::cookies::refresh_cookie;
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
        .route("/{id}/password", post(change_password))
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

// ---- password ----

/// The 403 a non-administrator gets for aiming this route at somebody else.
///
/// The same string `routes::extractors` uses for an administrator-only route:
/// this one is not administrator-only — it is self-service *or* administrator
/// — so the check is in the handler rather than in [`AdminUser`], but a caller
/// must not be able to tell the two situations apart.
const ADMIN_REQUIRED: &str = "admin required";

/// The 401 for a caller whose row disappeared between the extractor and the
/// lock; the same string every other failed authentication answers with.
const AUTHENTICATION_REQUIRED: &str = "authentication required";

/// The 400 for a self-service change that left `current_password` out.
const CURRENT_PASSWORD_REQUIRED: &str = "current password required";

/// The 400 for a self-service change whose `current_password` did not verify
/// against the locked row.
const CURRENT_PASSWORD_INCORRECT: &str = "current password is incorrect";

/// `POST /users/{id}/password` (`{ current_password?, password }`).
///
/// `current_password` is required when `id` is the caller's own id and ignored
/// otherwise: an administrator setting somebody else's password does not know
/// the old one, which is the point of the route (`SPEC.md`, "Authentication").
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangePasswordRequest {
    current_password: Option<String>,
    password: String,
}

/// `POST /users/{id}/password` → `{ user, access_token }` (self) or 204
/// (administrator).
///
/// [`UngatedUser`], because this is the route that *clears* the
/// password-change gate: the seeded administrator's first login ends here, and
/// a user sent to the change-password page has to be able to reach it
/// (`SPEC.md`, "Authentication"). A gated administrator may also change
/// somebody else's password — the exemption is listed without qualification —
/// though the frontend never offers it.
///
/// Two flows behind one path, split on `id`, because `SPEC.md` gives them one
/// row in "Users (`/api/users`)" and two different answers. What they share is
/// [`UserRepository::apply_password_change`]: whichever one runs, the hash,
/// `auth_version`, `must_change_password`, every refresh token and every
/// outstanding reset link move together or not at all (ADR 0025).
///
/// The response type is [`Response`] rather than a tuple because the two
/// answers do not have one shape: 200 with a body and a `Set-Cookie`, or 204
/// with neither.
async fn change_password(
    State(state): State<AppState>,
    UngatedUser(caller): UngatedUser,
    jar: CookieJar,
    Path(id): Path<Uuid>,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<Response> {
    if id == caller.id {
        change_own_password(&state, jar, &caller, body).await
    } else {
        change_another_password(&state, &caller, id, body).await
    }
}

/// The self-service change: 200 `{ user, access_token }` and a fresh
/// `refresh_token` cookie.
///
/// "A self-service change requires `current_password` and creates a
/// replacement refresh token in that transaction, then returns its cookie and
/// a matching access token after commit; this browser stays signed in"
/// (`SPEC.md`, "Authentication"). The replacement is created *by*
/// [`UserRepository::apply_password_change`], after its blanket revocation and
/// inside the same transaction, which is why this handler does not call
/// `auth::issue_pair`: a token inserted before the revocation would revoke
/// itself, and one inserted after the commit would leave a window in which the
/// browser holds no usable token at all.
///
/// The order is the one `docs/data-model.md` requires of a credential
/// mutation, and the expensive part is deliberately outside the lock:
///
/// 1. the new password validated and hashed on the blocking pool;
/// 2. `BEGIN` and the user-row lock;
/// 3. `current_password` verified against the row the lock returned — not
///    against the copy the extractor read, which may predate a password change
///    that committed while this request waited (ADR 0025);
/// 4. the mutation, `COMMIT`, and only then the token and the cookie.
///
/// Step 3 spends an Argon2 verification while holding the row lock. That is
/// the sanctioned cost: the alternative is verifying twice, and the lock is
/// per user, so what waits behind it is that one user's own concurrent
/// credential work.
async fn change_own_password(
    state: &AppState,
    jar: CookieJar,
    caller: &User,
    body: ChangePasswordRequest,
) -> Result<Response> {
    let Some(current_password) = body.current_password else {
        debug!(user_id = %caller.id, "password change refused: no current password");
        return Err(Error::BadRequest(CURRENT_PASSWORD_REQUIRED.to_string()));
    };

    let password = Password::parse(&body.password)?;
    let password_hash = hash_blocking(password).await?;

    let users = UserRepository::new(&state.pool);
    let mut tx = state.pool.begin().await?;

    let Some(locked) = users.lock_user(&mut tx, caller.id).await? else {
        // Deleted between the extractor's read and the lock.
        return Err(Error::Unauthorized(AUTHENTICATION_REQUIRED.to_string()));
    };

    if !verify_blocking(locked.password_hash.clone(), current_password).await? {
        debug!(user_id = %caller.id, "password change refused: wrong current password");
        return Err(Error::BadRequest(CURRENT_PASSWORD_INCORRECT.to_string()));
    }

    let replacement = OpaqueToken::generate();
    let updated = users
        .apply_password_change(&mut tx, locked.id, &password_hash, Some(&replacement.hash))
        .await?;
    tx.commit().await?;

    // From the committed row, so the claims carry the new `auth_version` and
    // the cleared `must_change_password` — which is what makes the token
    // minted a moment ago stop working and this one start.
    let access_token = Claims::for_user(&updated, Utc::now()).encode(&state.config)?;

    info!(user_id = %updated.id, "password changed");

    Ok((
        StatusCode::OK,
        jar.add(refresh_cookie(&state.config, &replacement.raw)),
        Json(TokenPairResponse {
            user: updated,
            access_token,
        }),
    )
        .into_response())
}

/// An administrator setting somebody else's password: 204, no cookie, no body.
///
/// "Changing another user's password returns 204 without changing the acting
/// admin's credentials" (`SPEC.md`, "Authentication"), so nothing here touches
/// the caller's row or tokens — only the target's, whose logins are all revoked
/// and whose `must_change_password` is cleared by the shared statement.
/// `current_password` is ignored rather than validated: the whole point is
/// that the administrator does not know it.
///
/// The role is read from `caller`, which the extractor loaded from the
/// database, never from the access token's `admin` claim (ADR 0025). The
/// administrator-membership advisory lock is not taken: a password is not
/// `admin`, and this cannot change how many administrators exist.
async fn change_another_password(
    state: &AppState,
    caller: &User,
    id: Uuid,
    body: ChangePasswordRequest,
) -> Result<Response> {
    if !caller.admin {
        debug!(user_id = %id, actor_id = %caller.id, "password change refused: not an administrator");
        return Err(Error::Forbidden(ADMIN_REQUIRED.to_string()));
    }

    let password = Password::parse(&body.password)?;
    let password_hash = hash_blocking(password).await?;

    let users = UserRepository::new(&state.pool);
    let mut tx = state.pool.begin().await?;

    let Some(target) = users.lock_user(&mut tx, id).await? else {
        return Err(Error::NotFound);
    };

    users
        .apply_password_change(&mut tx, target.id, &password_hash, None)
        .await?;
    tx.commit().await?;

    info!(user_id = %id, actor_id = %caller.id, "password changed by an administrator");

    Ok(StatusCode::NO_CONTENT.into_response())
}
