//! Test-only routes, compiled only with the `integration-tests` feature and
//! never into a release build (`SPEC.md`, "Test-only routes").
//!
//! For now the three probes below: one route per extractor, so the
//! authentication contract is asserted end to end through the real router
//! before the routes that will use the extractors exist. Each answers the
//! authenticated `User` as JSON; what is interesting about them is the status
//! they answer when authentication or authorization fails.
//!
//! `POST /test/users` — the documented route Playwright creates its users
//! with — joins this module with the users-routes task. The probes are
//! expected to be replaced by `GET /users/me` and the real administrator
//! routes once those land, and this module then keeps only `/test/users`.

use axum::Router;
use axum::routing::get;

use crate::models::User;
use crate::prelude::*;
use crate::routes::extractors::{AdminUser, CurrentUser, UngatedUser};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/whoami", get(whoami))
        .route("/whoami-ungated", get(whoami_ungated))
        .route("/whoami-admin", get(whoami_admin))
}

/// The signed-in user, behind the password-change gate.
async fn whoami(CurrentUser(user): CurrentUser) -> Json<User> {
    Json(user)
}

/// The signed-in user, gate not applied: what `GET /users/me` does.
async fn whoami_ungated(UngatedUser(user): UngatedUser) -> Json<User> {
    Json(user)
}

/// The signed-in user, who has to be an administrator in the database row.
async fn whoami_admin(AdminUser(user): AdminUser) -> Json<User> {
    Json(user)
}
