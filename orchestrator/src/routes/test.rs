//! Test-only routes, compiled only with the `integration-tests` feature and
//! never into a release build (`SPEC.md`, "Test-only routes").
//!
//! One route: `POST /test/users`, the way Playwright and the backend
//! integration tests get a signed-in user without the invite flow. It is the
//! only fixture endpoint the specification lists, and nothing else belongs
//! here — a test that needs a row the API cannot produce writes it through a
//! repository from the test process, which reaches the same database.
//!
//! The route takes no authentication of any kind and creates administrators on
//! request, which is exactly why the whole module is behind the feature gate:
//! the gate is the security boundary, not a check inside the handler.
//!
//! Otherwise it is an ordinary route. It validates through the
//! [`crate::models::user`] types and then hands over to
//! [`Credentials::create_user`], the same module every other credential comes
//! from, so a user created here is indistinguishable from one who accepted an
//! invitation — including the `refresh_token` cookie, which is what lets a
//! Playwright browser refresh its access token like any other.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum_extra::extract::CookieJar;
use serde::Deserialize;

use crate::auth::{Credentials, IssuedPair};
use crate::models::{Email, Password, Username};
use crate::prelude::*;
use crate::routes::auth::TokenPairResponse;

pub fn routes() -> Router<AppState> {
    Router::new().route("/users", post(create_user))
}

/// `POST /test/users` (`{ username, email, password, admin? }`).
///
/// `admin` is the only optional field and defaults to false.
/// `must_change_password` is not a field: this route exists to hand out a user
/// who can go straight to the routes under test, so the flag is always false
/// (`SPEC.md`, "Test-only routes"). A test that wants a gated user inserts one
/// through the repository instead.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateUserRequest {
    username: String,
    email: String,
    password: String,
    admin: Option<bool>,
}

/// Create a user and sign them in: 201 with `{ user, access_token }` and the
/// refresh cookie.
///
/// The three fields are validated before anything is hashed, so a bad username
/// or email costs no Argon2 work and the caller gets the 400 that names the
/// field it got wrong. A username or email that is well formed but taken is
/// the repository's 409 (`SPEC.md`, "Users"), which is what makes a Playwright
/// run that reuses a name fail loudly instead of authenticating as somebody
/// else's fixture.
async fn create_user(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<CreateUserRequest>,
) -> Result<(StatusCode, CookieJar, Json<TokenPairResponse>)> {
    // Parsed field by field before anything is hashed, so the cheap rejections
    // happen first and name the field that was wrong.
    let username = Username::parse(&body.username)?;
    let email = Email::parse(&body.email)?;
    let password = Password::parse(&body.password)?;

    let IssuedPair {
        user,
        access_token,
        refresh_cookie,
    } = Credentials::new(&state)
        .create_user(username, email, password, body.admin.unwrap_or(false))
        .await?;

    Ok((
        StatusCode::CREATED,
        jar.add(refresh_cookie),
        Json(TokenPairResponse { user, access_token }),
    ))
}
