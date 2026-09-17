//! All SQL, through `sqlx::query!` / `query_as!`. One `XRepository<'a>` per
//! aggregate, borrowing the `PgPool`, with the scope in the `WHERE` clause.

// Empty module: the glob import is the crate convention (`CLAUDE.md`, "Backend
// conventions"). The first real file here removes this `allow`.
#![allow(unused_imports)]

use crate::prelude::*;
