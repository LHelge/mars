//! Axum routers, one module per resource, nested under `/api`. Each module
//! exports `routes() -> Router<AppState>` and keeps its DTOs private.

// Empty module: the glob import is the crate convention (`CLAUDE.md`, "Backend
// conventions"). The first real file here removes this `allow`.
#![allow(unused_imports)]

use crate::prelude::*;
