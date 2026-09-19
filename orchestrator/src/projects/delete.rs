//! Deleting a project: one short transaction, then everything it owned on
//! disk.
//!
//! `DELETE /api/projects/{id}` is 204, or 409 "while any session is `running`
//! or `creating`" (`SPEC.md`, "Projects"). What goes with the row is the whole
//! of it — "Deleting a project deletes its sessions, tasks, secrets, shared
//! directories, CLI state directory and mirror" (`SPEC.md`, "User-facing
//! features" → "Projects"; `README.md`, "Operating notes").
//!
//! **Order** ([`delete_project`]): the project git lock, then the transaction
//! opened under it, then the filesystem work after the commit and still under
//! the lock (`ARCHITECTURE.md`, "Git model", Serialization: "project deletion
//! uses the same lock"; ADR 0021 for the direction, never the reverse). The
//! lock is what keeps the cron mirror fetch, a launch or a git endpoint from
//! opening `repo.git` while it is being removed, and it is what makes a
//! deletion racing the clone job a clean case rather than a corruption: the
//! job holds the lock, this waits, and the job's guarded `mark_*` then matches
//! no row and it removes what it created ([`super::clone_job`]).
//!
//! **The transaction is short and touches no file.** It locks the project row
//! — the tracker's "one mutation at a time per project" (`ARCHITECTURE.md`,
//! "Task tracker"), so a deletion cannot interleave with a tracker write —
//! counts the live sessions, reads the session ids it will need afterwards,
//! deletes the project's secrets and deletes the row. Everything else is a
//! cascade: every foreign key to `projects(id)` is `ON DELETE CASCADE`
//! (`docs/data-model.md`), which takes the profiles, the shared-directory
//! rows, the sessions and their events, the task states, the tasks with their
//! dependencies, comments and hand-offs, and the `task_events` that cascade on
//! `project_id` of their own. Secrets are the one exception, because
//! `secrets.scope_id` carries no foreign key to cascade through; they are
//! deleted explicitly here rather than left for the orphan reaper
//! (`ARCHITECTURE.md`, "Background jobs"), and their `secret_uses` audit rows
//! cascade with them.
//!
//! **After the commit the filesystem is best effort.** The rows are gone; a
//! directory that will not go is logged with the project and the path and the
//! answer is still 204, because there is nothing left to roll back to and the
//! orphan-cleanup job and the operator are what handle leftovers
//! (`ARCHITECTURE.md`, "Background jobs").
//!
//! **No container work.** With no session `running` or `creating` every
//! session container is already gone by the lifecycle rules, and a stray one
//! is the orphan-cleanup job's (`ARCHITECTURE.md`, "Background jobs").

use uuid::Uuid;

use crate::prelude::*;
use crate::projects::layout::{remove_dir_all, session_dir};
use crate::repositories::{ProjectRepository, SecretRepository, SessionRepository};

/// What a caller is told while a session of the project is `running` or
/// `creating` (409; `SPEC.md`, "Projects").
const RUNNING_SESSIONS: &str = "project has running sessions";

/// Delete a project, its rows, its secrets and everything it owns on disk.
///
/// [`Error::NotFound`] for an id that is not there, including a second
/// deletion of the same project, and [`Error::Conflict`] while any of its
/// sessions is `running` or `creating`. Both are decided inside the
/// transaction that holds the project row, so neither can be overtaken by the
/// launch it is refusing: a launcher that commits its `creating` row first is
/// seen by the count, and one that commits second finds no project and fails
/// the launch.
#[instrument(skip_all, fields(project_id = %id))]
pub async fn delete_project(state: &AppState, id: Uuid) -> Result<()> {
    // First, and held past the commit through the last removal (ADR 0021).
    let guard = state.git_locks.lock(id).await;

    let session_ids = delete_rows(state, id).await?;

    // The row is gone, so nothing below may fail the request: each removal
    // logs what it could not do and the deletion still answers 204.
    remove_files(state, id, &session_ids).await;

    // There is no session registry to drop entries from yet — it belongs to
    // the Session lifecycle epic — so there is nothing to forget here but the
    // lock itself. Forgetting it while it is still held is the documented
    // order ([`ProjectGitLocks::forget`](crate::git::ProjectGitLocks::forget)):
    // waiters keep the entry they queued on, and a caller arriving afterwards
    // makes a fresh one and fails on the directory that is no longer there.
    state.git_locks.forget(id);
    drop(guard);

    info!(sessions = session_ids.len(), "project deleted");

    Ok(())
}

/// The transaction: lock, refuse, collect, delete. No filesystem work between
/// `BEGIN` and `COMMIT`.
///
/// Returns the ids of the sessions the cascade removed, which is what
/// [`remove_files`] needs and the only reason they are read at all. The
/// `cli_session_id`s are deliberately not among them: transcripts live under
/// the project's `claude/` directory, which goes with the project directory.
async fn delete_rows(state: &AppState, id: Uuid) -> Result<Vec<Uuid>> {
    let projects = ProjectRepository::new(&state.pool);
    let sessions = SessionRepository::new(&state.pool);
    let secrets = SecretRepository::new(&state.pool);

    let mut tx = state.pool.begin().await?;

    // `Error::NotFound` for an unknown project, which is the documented 404.
    // The transaction is rolled back as it is dropped on the way out.
    projects.lock_project(&mut tx, id).await?;

    if sessions.count_live_for_project(&mut tx, id).await? > 0 {
        tx.rollback().await?;
        return Err(Error::Conflict(RUNNING_SESSIONS.to_string()));
    }

    let session_ids = sessions.ids_for_project(&mut tx, id).await?;

    // Before the row, because `secrets.scope_id` has no foreign key: nothing
    // in the schema would take them, and a project's leftovers would otherwise
    // become exactly the orphans the reaper exists to find.
    secrets.delete_all_for_project(&mut tx, id).await?;

    if !projects.delete(&mut tx, id).await? {
        // Unreachable: the row was locked in this transaction and no other
        // writer can have removed it. Answered rather than asserted, because a
        // panic in a handler is worse than a 404.
        tx.rollback().await?;
        return Err(Error::NotFound);
    }

    tx.commit().await?;

    Ok(session_ids)
}

/// The project's own directory and one directory per former session, under the
/// git lock the caller still holds.
///
/// Infallible on purpose: the rows are committed, so every failure here is a
/// leftover for the orphan-cleanup job rather than an outcome the caller can
/// act on. Each one is logged with the project id and the path in structured
/// fields by the layout helpers (`crate::projects::layout`).
async fn remove_files(state: &AppState, id: Uuid, session_ids: &[Uuid]) {
    // `repo.git`, `claude/` and every `shared/<name>` in one removal
    // (`ARCHITECTURE.md`, "Storage"); a project whose clone job never ran has
    // no directory, and a missing one is a success.
    if let Err(error) = state.config.project_layout(id).remove_all().await {
        error!(%error, "the deleted project's directory could not be removed");
    }

    for &session_id in session_ids {
        let path = session_dir(&state.config.data_dir, session_id);

        if let Err(error) = remove_dir_all(&path).await {
            error!(
                session_id = %session_id,
                %error,
                "a deleted project's session directory could not be removed"
            );
        }
    }
}
