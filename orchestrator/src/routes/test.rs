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
//! Otherwise it is an ordinary route. It validates through the [`crate::models::user`]
//! types, hashes on the blocking pool, inserts through [`UserRepository`] and
//! issues its pair through the same [`issue_pair`] login uses, so a user
//! created here is indistinguishable from one who accepted an invitation —
//! including the `refresh_token` cookie, which is what lets a Playwright
//! browser refresh its access token like any other.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum_extra::extract::CookieJar;
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::models::{Email, NewUser, Password, Username};
use crate::prelude::*;
use crate::repositories::UserRepository;
use crate::routes::auth::{TokenPairResponse, hash_blocking, issue_pair};
use crate::routes::cookies::refresh_cookie;

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
///
/// No administrator-membership lock is taken. Only `PUT` and `DELETE` on
/// `/users/{id}` can *reduce* the number of administrators, and this route
/// only ever adds one (`SPEC.md`, "Users").
async fn create_user(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<CreateUserRequest>,
) -> Result<(StatusCode, CookieJar, Json<TokenPairResponse>)> {
    // Built field by field rather than through `NewUser::new`, which takes the
    // finished hash: this way the cheap rejections happen first.
    let username = Username::parse(&body.username)?;
    let email = Email::parse(&body.email)?;
    let password = Password::parse(&body.password)?;

    let new_user = NewUser {
        id: Uuid::new_v4(),
        username,
        email,
        password_hash: hash_blocking(password).await?,
        admin: body.admin.unwrap_or(false),
        must_change_password: false,
    };

    // One transaction, as login's is: the user and the refresh token it is
    // handed back with are committed together or not at all. `notify_email`
    // comes from the column default, which is true (`docs/data-model.md`,
    // `users`).
    let mut tx = state.pool.begin().await?;
    let user = UserRepository::new(&state.pool)
        .insert(&mut tx, &new_user)
        .await?;
    let (raw, _) = issue_pair(&state, &mut tx, &user).await?;
    tx.commit().await?;

    // After the commit and from the inserted row, like every other route that
    // issues a pair (`docs/data-model.md`, "Users and authentication").
    let access_token = Claims::for_user(&user, Utc::now()).encode(&state.config)?;

    info!(user_id = %user.id, admin = user.admin, "test user created");

    Ok((
        StatusCode::CREATED,
        jar.add(refresh_cookie(&state.config, &raw)),
        Json(TokenPairResponse { user, access_token }),
    ))
}
