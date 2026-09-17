//! Axum routers, one module per resource, nested under `/api`. Each module
//! exports `routes() -> Router<AppState>` and keeps its DTOs private.
//!
//! This module only merges them; the `/api` prefix is applied once in
//! `build_api_router`, so a module's own paths are written without it.

use axum::Router;

use crate::prelude::*;

pub mod auth;
pub mod cookies;
pub mod extractors;
pub mod health;
pub mod throttle;
pub mod users;

/// The probes and fixtures the integration and end-to-end tests drive
/// (`SPEC.md`, "Test-only routes"). Never compiled into a release build.
#[cfg(feature = "integration-tests")]
pub mod test;

pub use extractors::{AdminUser, CurrentUser, UngatedUser, authenticate_access_token};

/// Every resource router, merged into the one router nested under `/api`.
pub fn routes() -> Router<AppState> {
    let router = Router::new()
        .merge(health::routes())
        .nest("/auth", auth::routes())
        .nest("/users", users::routes());

    // The only routes with a prefix of their own, because `SPEC.md`,
    // "Test-only routes" gives them one, and because a single `nest` is what
    // keeps them all behind the one feature gate.
    #[cfg(feature = "integration-tests")]
    let router = router.nest("/test", test::routes());

    router
}
