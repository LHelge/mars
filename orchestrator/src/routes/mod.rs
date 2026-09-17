//! Axum routers, one module per resource, nested under `/api`. Each module
//! exports `routes() -> Router<AppState>` and keeps its DTOs private.
//!
//! This module only merges them; the `/api` prefix is applied once in
//! `build_api_router`, so a module's own paths are written without it.

use axum::Router;

use crate::prelude::*;

pub mod health;
pub mod throttle;

/// Every resource router, merged into the one router nested under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new().merge(health::routes())
}
