//! Which sessions worked on which tasks.
//!
//! `docs/data-model.md`, `task_sessions`. The row is upserted whenever a
//! session actually changes a task; `first_touched_at` is preserved on
//! conflict and `last_touched_at` only advances for a real change (ADR 0030).
//! Both rules are `ON CONFLICT` behaviour and live in the repository, so this
//! model is the row shape alone.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// The crate convention (`CLAUDE.md`, "Backend conventions"); see `task.rs`.
#[allow(unused_imports)]
use crate::prelude::*;

/// A `task_sessions` row, column for column (`docs/data-model.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskSession {
    pub task_id: Uuid,
    pub session_id: Uuid,
    pub first_touched_at: DateTime<Utc>,
    pub last_touched_at: DateTime<Utc>,
}
