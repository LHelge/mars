//! What a project owns outside Postgres: its directory on the `/data` volume.
//!
//! `ARCHITECTURE.md`, "Storage" draws the tree — `repo.git`, the per-project
//! CLI state directory and the shared directories, all under
//! `DATA_DIR/projects/<id>/` — and [`layout::ProjectLayout`] is that subtree as
//! code together with the handful of filesystem operations the clone job,
//! project deletion and the shared-directory endpoints need. Path construction
//! lives there and nowhere else, so no call site joins `shared` or `claude`
//! onto a project directory by hand.
//!
//! The database side of a project is [`crate::repositories::ProjectRepository`];
//! the git side is [`crate::git::mirror`]. This module is only the directories.

pub mod clone_job;
pub mod create;
pub mod layout;

pub use create::{DEFAULT_PROFILE_NAME, NewProjectRequest, create_project};
pub use layout::{ProjectLayout, session_dir};

// The crate convention (`CLAUDE.md`, "Backend conventions"); this module is a
// facade, so nothing here uses it.
#[allow(unused_imports)]
use crate::prelude::*;
