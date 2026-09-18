//! How a project comes into existence, and what it owns outside Postgres.
//!
//! Three parts, in the order a project goes through them.
//!
//! [`layout::ProjectLayout`] is the project's subtree of the `/data` volume as
//! code — `repo.git`, the per-project CLI state directory and the shared
//! directories under `DATA_DIR/projects/<id>/` (`ARCHITECTURE.md`, "Storage")
//! — together with the filesystem operations the clone job, project deletion
//! and the shared-directory endpoints need. Path construction lives there and
//! nowhere else, so no call site joins `shared` or `claude` onto a project
//! directory by hand.
//!
//! [`create::create_project`] is what `POST /api/projects` runs: one
//! transaction that writes the project row, the default task states, the
//! `default` agent profile and — when the caller supplied one — the
//! project-scoped `GIT_CREDENTIAL` secret, or none of them. It touches neither
//! git nor the filesystem, so a rolled-back creation leaves nothing on the
//! volume.
//!
//! [`clone_job`] is the background half, started after that transaction
//! committed and again by `POST /api/projects/{id}/retry-clone`: under the
//! project git lock it creates the directories, initialises and fetches the
//! bare repository through [`crate::git::mirror`], and publishes the outcome as
//! the one guarded `cloning → ready` or `cloning → error` write.
//!
//! [`delete::delete_project`] is the other end of that life, and it takes the
//! same lock: one short transaction that refuses while a session is `running`
//! or `creating`, deletes the project's secrets and the row every other table
//! cascades from, and then — after the commit, still under the lock — removes
//! the project directory and each former session's directory.
//!
//! The database side of a project is [`crate::repositories::ProjectRepository`];
//! the HTTP side is [`crate::routes::projects`].

pub mod clone_job;
pub mod create;
pub mod delete;
pub mod layout;

pub use create::{DEFAULT_PROFILE_NAME, NewProjectRequest, create_project};
pub use delete::delete_project;
pub use layout::{ProjectLayout, session_dir};

// The crate convention (`CLAUDE.md`, "Backend conventions"); this module is a
// facade, so nothing here uses it.
#[allow(unused_imports)]
use crate::prelude::*;
