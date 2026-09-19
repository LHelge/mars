//! `POST /api/auth/login`, `/refresh`, `/logout`, `/accept-invite`,
//! `/request-password-reset` and `/reset-password`, plus `GET
//! /api/auth/invite/{token}` (`SPEC.md`, "Auth (`/api/auth`)").
//!
//! The seven routes nobody has to be signed in to reach: the three that manage
//! an ordinary sign-in's `refresh_tokens` rows, the two halves of the
//! reset-by-email flow, which hand out none, and the two halves of invite
//! acceptance — the lookup the accept page previews the invitation with, and
//! the acceptance itself, which is the only way a user who was not seeded
//! comes into existence (ADR 0013) and signs them in as it creates them.
//!
//! All seven are HTTP translation and nothing else: parse the body, call
//! [`Credentials`], shape the answer. The locking, the revalidation, the
//! commit-before-issue order, the lifetimes, the cookie and the throttle live
//! in [`crate::auth`], which is the only thing in the crate that issues a
//! credential — so no route here can get that order wrong, because no route
//! here writes it down (`ARCHITECTURE.md`, "User authentication and
//! revocation"; ADR 0025).
//!
//! None of the seven takes an extractor from `routes::extractors`: login
//! authenticates with a password, refresh and logout with the cookie, the two
//! reset routes and the two invite routes with nothing at all, and a user with
//! `must_change_password` reaches all of them (`SPEC.md`, "Authentication").
//! The *other* password mutation — `POST /users/{id}/password`, the one a
//! signed-in user or an administrator performs — lives in
//! [`crate::routes::users`] and shares this module's [`TokenPairResponse`].

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Router};
use axum_extra::extract::CookieJar;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::auth::credentials::invalid_invite;
use crate::auth::{Credentials, IssuedPair};
use crate::models::{OpaqueToken, User};
use crate::prelude::*;
use crate::repositories::UserInviteRepository;
use crate::routes::throttle::client_addr;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/refresh", post(refresh))
        .route("/logout", post(logout))
        .route("/invite/{token}", get(lookup_invite))
        .route("/accept-invite", post(accept_invite))
        .route("/request-password-reset", post(request_password_reset))
        .route("/reset-password", post(reset_password))
}

/// `POST /auth/login` (`SPEC.md`, "Auth (`/api/auth`)").
///
/// A missing or malformed field is 400 through the [`Json`] extractor; the
/// username is trimmed before anything is looked up and the password is used
/// exactly as sent.
#[derive(Debug, Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

/// `{ user, access_token }`: what login, invite acceptance and a self-service
/// password change all answer with (`SPEC.md`, "Authentication").
///
/// `user` serialises through the [`User`] model, which skips `password_hash`,
/// `auth_version` and `updated_at`, so the body is the documented `User` DTO
/// and the refresh token is not in it — that one only ever travels in the
/// cookie.
///
/// `pub(crate)` because the accept-invite, password-change and test-fixture
/// routes return the same body and must not restate its shape.
#[derive(Debug, Serialize)]
pub(crate) struct TokenPairResponse {
    pub user: User,
    pub access_token: String,
}

/// Exchange a username and password for a token pair.
///
/// The trimming and the client address are the only decisions here; the
/// throttle, the hashing, the lock and the issuing are
/// [`Credentials::login`]'s.
async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(body): Json<LoginRequest>,
) -> Result<(StatusCode, CookieJar, Json<TokenPairResponse>)> {
    let username = body.username.trim();
    let addr = client_addr(&headers, peer.map(|Extension(info)| info));

    let IssuedPair {
        user,
        access_token,
        refresh_cookie,
    } = Credentials::new(&state)
        .login(username, &body.password, addr)
        .await?;

    Ok((
        StatusCode::OK,
        jar.add(refresh_cookie),
        Json(TokenPairResponse { user, access_token }),
    ))
}

/// Rotate the refresh cookie and mint a new pair from the current user row.
///
/// Every rejection is the same 401 as a missing cookie, and every 401 also
/// clears the cookie (`SPEC.md`, "Authentication"): a browser holding a token
/// the database will never accept again should stop sending it. A 5xx does
/// *not* clear it — a refresh that failed because Postgres was briefly
/// unreachable is a retry, not a sign-out, and clearing the cookie would turn
/// one transient failure into a forced login.
///
/// This is the one handler in the crate that builds its own [`Response`]
/// rather than returning [`Result`]: the failure path has to carry a
/// `Set-Cookie` header as well as the error, and the crate-wide [`Error`] is
/// deliberately a plain enum with no room for one. The clearing cookie itself
/// comes from [`Credentials::clearing_cookie`]; nothing here builds one.
async fn refresh(State(state): State<AppState>, jar: CookieJar) -> Response {
    let credentials = Credentials::new(&state);
    let rotated = credentials.refresh(&jar).await;

    match rotated {
        Ok(IssuedPair {
            user,
            access_token,
            refresh_cookie,
        }) => (
            StatusCode::OK,
            jar.add(refresh_cookie),
            Json(TokenPairResponse { user, access_token }),
        )
            .into_response(),
        Err(err) if err.status() == StatusCode::UNAUTHORIZED => {
            (jar.add(credentials.clearing_cookie()), err).into_response()
        }
        Err(err) => err.into_response(),
    }
}

/// Revoke the presented refresh token and clear the cookie.
///
/// Always 204, with or without a cookie and whether or not the token was still
/// usable: logging out is not an operation that can be refused, and telling a
/// caller that their cookie named nothing would be an oracle over stored
/// tokens.
async fn logout(State(state): State<AppState>, jar: CookieJar) -> Result<(StatusCode, CookieJar)> {
    let cleared = Credentials::new(&state).logout(&jar).await?;

    Ok((StatusCode::NO_CONTENT, jar.add(cleared)))
}

/// `POST /auth/request-password-reset` (`{ identifier }`).
///
/// `identifier` is "your username or email": the form cannot know which the
/// user typed, and neither can this handler (`SPEC.md`, "Authentication").
#[derive(Debug, Deserialize)]
struct RequestPasswordResetRequest {
    identifier: String,
}

/// Mail a reset link, or quietly do nothing — 204 either way.
///
/// The response is a constant: 204 with an empty body for a known identifier,
/// an unknown one, a rate-limited one and a failed delivery alike (`SPEC.md`,
/// "Auth (`/api/auth`)": "→ 204 (always)"). Anything else would turn this
/// endpoint into an account-enumeration oracle, which is why
/// [`Credentials::request_password_reset`] answers `Ok(())` to all four.
async fn request_password_reset(
    State(state): State<AppState>,
    Json(body): Json<RequestPasswordResetRequest>,
) -> Result<StatusCode> {
    Credentials::new(&state)
        .request_password_reset(body.identifier.trim())
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /auth/reset-password` (`{ token, password }`).
///
/// `token` is the raw value out of the emailed link; only its SHA-256 hex is
/// stored, so it is hashed inside the module and compared as bytes.
#[derive(Debug, Deserialize)]
struct ResetPasswordRequest {
    token: String,
    password: String,
}

/// Spend a reset link and set a new password. 204, no cookie, no body.
///
/// A reset does not log anybody in (`SPEC.md`, "Authentication": "Reset by
/// link returns 204 without logging the user in; they then log in with the new
/// password"), which is why this returns no pair to shape.
async fn reset_password(
    State(state): State<AppState>,
    Json(body): Json<ResetPasswordRequest>,
) -> Result<StatusCode> {
    Credentials::new(&state)
        .reset_password(&body.token, &body.password)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /auth/invite/{token}` → `{ email, admin, expires_at }`.
///
/// The preview the accept page renders before anybody types anything: it is
/// what lets the form say *which* address was invited and whether accepting
/// makes an administrator, neither of which the invitee can be asked to
/// retype.
///
/// The response carries those three fields and nothing else — not the invite
/// id, not `invited_by`, not `created_at`. A [`UserInvite`] serialises to the
/// six-key admin `Invite` DTO, and answering that here would hand an
/// unauthenticated caller the identity of the administrator who sent it, so
/// this is a DTO of its own rather than the row.
///
/// Unlocked and read-only ([`UserInviteRepository::find_open_by_hash`], whose
/// `WHERE` is the whole validity rule), and it issues nothing, which is why it
/// is the one invite route that does not go through [`Credentials`]. Nothing
/// may be decided from it: acceptance re-reads the same row under a lock.
///
/// [`UserInvite`]: crate::models::UserInvite
#[derive(Debug, Serialize)]
struct InviteLookupResponse {
    email: String,
    admin: bool,
    expires_at: DateTime<Utc>,
}

/// Preview an invitation, or the same 400 `invalid or expired invite` that
/// acceptance answers with — deliberately one message from both, so a caller
/// holding a guessed token learns only that it is not a live invitation.
///
/// The path parameter is hashed exactly as it arrived — no trimming. A path
/// segment has no surrounding whitespace to lose, and `%20` in a link is a
/// different token, not a typo to be repaired.
async fn lookup_invite(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<InviteLookupResponse>> {
    let Some(invite) = UserInviteRepository::new(&state.pool)
        .find_open_by_hash(&OpaqueToken::hash_of(&token))
        .await?
    else {
        // No `invite_id` to log: there is no row. The token itself is never
        // logged (rule 3), so this rejection is deliberately silent.
        return Err(invalid_invite());
    };

    Ok(Json(InviteLookupResponse {
        email: invite.email,
        admin: invite.admin,
        expires_at: invite.expires_at,
    }))
}

/// `POST /auth/accept-invite` (`{ token, username, password }`).
///
/// There is no `email` field and no `admin` field: both come from the invite,
/// which is the point of the invitation being the credential (`SPEC.md`,
/// "User-facing features": there is no self-registration). A caller who could
/// choose either would be registering, not accepting.
#[derive(Debug, Deserialize)]
struct AcceptInviteRequest {
    token: String,
    username: String,
    password: String,
}

/// Spend an invitation: create the user it names and sign them in. 201 with
/// `{ user, access_token }` and the `refresh_token` cookie.
async fn accept_invite(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<AcceptInviteRequest>,
) -> Result<(StatusCode, CookieJar, Json<TokenPairResponse>)> {
    let IssuedPair {
        user,
        access_token,
        refresh_cookie,
    } = Credentials::new(&state)
        .accept_invite(&body.token, &body.username, &body.password)
        .await?;

    Ok((
        StatusCode::CREATED,
        jar.add(refresh_cookie),
        Json(TokenPairResponse { user, access_token }),
    ))
}
