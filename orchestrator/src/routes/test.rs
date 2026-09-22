//! Test-only routes, compiled only with the `integration-tests` feature and
//! never into a release build (`SPEC.md`, "Test-only routes").
//!
//! Three routes. `POST /test/users` is the way Playwright and the backend
//! integration tests get a signed-in user without the invite flow. `GET
//! /test/stream-whoami` is the probe the stream-authentication tests drive
//! [`crate::routes::stream_auth`] through, because the real `?token=` endpoints
//! are WebSocket and SSE streams and an authentication contract should be
//! asserted on a plain request. `POST /test/scheduler-tick` runs the
//! scheduled-agent job once with the two instants the caller chose, because a
//! cron expression cannot come due sooner than the next minute boundary and no
//! suite here waits a minute. They are the fixture endpoints the
//! specification lists, and nothing else belongs here — a test that needs a
//! row the API cannot produce writes it through a repository from the test
//! process, which reaches the same database.
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
use axum::routing::{get, post};
use axum_extra::extract::CookieJar;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{Credentials, IssuedPair};
use crate::cron::{CronService, JobName};
use crate::models::{Email, Password, Username};
use crate::prelude::*;
use crate::routes::auth::TokenPairResponse;
use crate::routes::stream_auth::{StreamToken, authenticate_stream};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users", post(create_user))
        .route("/stream-whoami", get(stream_whoami))
        .route("/scheduler-tick", post(scheduler_tick))
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

/// `GET /test/stream-whoami?token=<jwt>` → `{ user_id }` (200).
///
/// The stream endpoints authenticate with [`StreamToken`] and
/// [`authenticate_stream`], and both of them are streams, so there is no
/// ordinary request that exercises the one contract they share — signature,
/// expiry, the user row, `auth_version` and the password-change gate, read off
/// `?token=`. This route is that request and nothing more: it authenticates
/// and reports the id it authenticated as, so a test asserts 200, 401 or 403
/// the way it does on any other route.
#[derive(Debug, Serialize)]
struct StreamWhoamiResponse {
    user_id: Uuid,
}

async fn stream_whoami(
    State(state): State<AppState>,
    token: StreamToken,
) -> Result<Json<StreamWhoamiResponse>> {
    let user = authenticate_stream(&state, &token.0).await?;

    Ok(Json(StreamWhoamiResponse { user_id: user.id }))
}

/// `POST /test/scheduler-tick` (`{ now, started_at? }`) → the run's counters.
///
/// The scheduled-agent job fires a profile whose expression has an occurrence
/// in `(max(last_scheduled_at, process start), now]` (`ARCHITECTURE.md`, "Task
/// tracker" → "Scheduled agents"). Both ends of that window are injectable —
/// the job takes its `now` from the caller and the floor is
/// [`CronService::with_started_at`] — but neither is reachable over HTTP, and a
/// suite that cannot place them can only wait for the next minute boundary.
/// This route is the way in: it builds its own `CronService` from the request,
/// runs the one job once and reports what it did. A `started_at` well before
/// `now` — two minutes is plenty — makes `* * * * *` due immediately, which is
/// what `frontend/tests/schedules.spec.ts` and `tests/session_e2e.rs` want.
///
/// The floor defaults to `now`, under which nothing is ever due, so a body
/// without one is the inert call and the due window is always something the
/// caller asked for in writing.
///
/// It is safe beside the real service's own minute ticks, because a tick is
/// claimed before it is launched in one statement carrying the value that was
/// read (`ProjectRepository::claim_schedule_tick`): whichever of the two runs
/// gets there first spends the tick, and the other finds the window closed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SchedulerTickRequest {
    now: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
}

/// The [`crate::cron::JobReport`] of the run, as the three counters every job
/// reports: sessions launched, ticks spent without one, and launches that
/// failed for some other reason.
#[derive(Debug, Serialize)]
struct SchedulerTickResponse {
    items: u64,
    skipped: u64,
    failures: u64,
}

async fn scheduler_tick(
    State(state): State<AppState>,
    Json(body): Json<SchedulerTickRequest>,
) -> Result<Json<SchedulerTickResponse>> {
    let started_at = body.started_at.unwrap_or(body.now);
    let report = CronService::with_started_at(state, started_at)
        .run_once(JobName::Scheduler, body.now)
        .await?;

    Ok(Json(SchedulerTickResponse {
        items: report.items,
        skipped: report.skipped,
        failures: report.failures,
    }))
}
