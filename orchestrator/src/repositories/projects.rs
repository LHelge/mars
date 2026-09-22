//! All SQL against `projects`, `project_shared_dirs` and `agent_profiles`,
//! plus the two primitives every tracker mutation is built on.
//!
//! `docs/data-model.md`, "Projects and profiles" is the column contract. The
//! reason the three tables share one repository is that they share one
//! aggregate: a shared directory and a profile are configuration of a project,
//! have no identity outside it and are cascaded away with it, so every
//! statement below scopes on `project_id` in its `WHERE` clause rather than
//! reading a row and checking afterwards (`CLAUDE.md`, "Backend conventions").
//!
//! The two primitives are [`ProjectRepository::lock_project`] and
//! [`ProjectRepository::allocate_task_number`]. `docs/data-model.md`, "Tracker
//! mutation transactions" and `ARCHITECTURE.md`, "Task tracker" make the
//! project row the serialisation point of the whole tracker: every tracker
//! writer locks it first and then reads authoritative state under it, which is
//! what makes cycle checks, lease decisions and task numbering safe under
//! `READ COMMITTED` (ADR 0021). Neither primitive enforces anything by itself;
//! the compositions live with the routes and the tracker's own repository.

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::models::{
    AgentBackend, AgentProfile, BranchName, NewAgentProfile, NewProject, NewSharedDir, ProfileKind,
    ProfileUpdate, Project, ProjectStatus, ProjectUpdate, SharedDir,
};
use crate::prelude::*;
use crate::repositories::{check_violation, foreign_key_violation, unique_violation};
use crate::secrets::GIT_CREDENTIAL_NAME;

/// All SQL against a project and the two tables that configure it
/// (`ARCHITECTURE.md`, "Orchestrator internals").
///
/// Reads that need no transaction go straight to the pool; everything that
/// mutates or locks takes the caller's `&mut PgConnection`, so one transaction
/// can hold a whole tracker mutation — the project lock, a task number, the
/// mutation itself and its events — together.
///
/// Every statement that returns a [`Project`] computes `has_credential` with
/// the same `EXISTS` subquery over `secrets`, because the field is part of the
/// row everywhere it is read (`SPEC.md`, "Projects"): a second round trip per
/// project would turn `GET /projects` into a query per row for a boolean.
pub struct ProjectRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> ProjectRepository<'a> {
    /// Borrow `pool` for the lifetime of this repository.
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new project and return the stored row.
    ///
    /// `status` and `next_task_number` are left to their column defaults: a
    /// project starts `cloning` with numbering at 1, and only the clone job
    /// moves it on. A duplicate name is the caller's mistake, so
    /// `projects_name_key` maps to [`Error::Conflict`] (`SPEC.md`,
    /// "Projects").
    ///
    /// `has_credential` is computed rather than assumed false: creating a
    /// project and storing its `GIT_CREDENTIAL` are one request, and a caller
    /// that writes the secret first in the same transaction gets a row that
    /// says so.
    pub async fn insert(&self, tx: &mut PgConnection, project: &NewProject) -> Result<Project> {
        let inserted = sqlx::query_as!(
            Project,
            r#"
            INSERT INTO projects (id, name, remote_url, default_branch, created_by, max_attempts)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, name, remote_url, default_branch, status as "status: ProjectStatus",
                      status_message, created_by, last_fetched_at, max_attempts,
                      next_task_number, max_concurrent_sessions, automation_paused,
                      created_at, updated_at,
                      EXISTS (
                          SELECT 1 FROM secrets s
                          WHERE s.scope = 'project' AND s.scope_id = projects.id AND s.name = $7
                      ) AS "has_credential!"
            "#,
            project.id,
            project.name.as_str(),
            project.remote_url.as_str(),
            project
                .default_branch
                .as_ref()
                .map(|branch| branch.as_str()),
            project.created_by,
            project.max_attempts.get(),
            GIT_CREDENTIAL_NAME,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(map_project_error)?;

        debug!(project_id = %inserted.id, "project inserted");

        Ok(inserted)
    }

    /// The project with this id, or `None`.
    pub async fn find(&self, id: Uuid) -> Result<Option<Project>> {
        let mut conn = self.pool.acquire().await?;

        self.find_in(&mut conn, id).await
    }

    /// The same, on the caller's connection.
    ///
    /// What a tracker mutation reads right after
    /// [`ProjectRepository::lock_project`]: the row it just locked, so the
    /// values it validates against — `max_attempts` above all — are the ones
    /// nobody else can change until it commits (`docs/data-model.md`,
    /// "Tracker mutation transactions"). Read on the pool instead and the
    /// answer is a snapshot from before the lock.
    pub async fn find_in(&self, conn: &mut PgConnection, id: Uuid) -> Result<Option<Project>> {
        let project = sqlx::query_as!(
            Project,
            r#"
            SELECT p.id, p.name, p.remote_url, p.default_branch,
                   p.status as "status: ProjectStatus", p.status_message, p.created_by,
                   p.last_fetched_at, p.max_attempts, p.next_task_number,
                   p.max_concurrent_sessions, p.automation_paused, p.created_at,
                   p.updated_at,
                   EXISTS (
                       SELECT 1 FROM secrets s
                       WHERE s.scope = 'project' AND s.scope_id = p.id AND s.name = $2
                   ) AS "has_credential!"
            FROM projects p
            WHERE p.id = $1
            "#,
            id,
            GIT_CREDENTIAL_NAME,
        )
        .fetch_optional(&mut *conn)
        .await?;

        Ok(project)
    }

    /// Every project, oldest first (`GET /projects`).
    ///
    /// `created_at` rather than `name`, because the list is the project
    /// switcher's order and a project does not move in it when it is renamed
    /// (`SPEC.md`, "Projects"). `id` breaks ties, which two projects created in
    /// the same microsecond would otherwise leave to the planner.
    pub async fn list(&self) -> Result<Vec<Project>> {
        let projects = sqlx::query_as!(
            Project,
            r#"
            SELECT p.id, p.name, p.remote_url, p.default_branch,
                   p.status as "status: ProjectStatus", p.status_message, p.created_by,
                   p.last_fetched_at, p.max_attempts, p.next_task_number,
                   p.max_concurrent_sessions, p.automation_paused, p.created_at,
                   p.updated_at,
                   EXISTS (
                       SELECT 1 FROM secrets s
                       WHERE s.scope = 'project' AND s.scope_id = p.id AND s.name = $1
                   ) AS "has_credential!"
            FROM projects p
            ORDER BY p.created_at, p.id
            "#,
            GIT_CREDENTIAL_NAME,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(projects)
    }

    /// The ids of every project in `status`, in a stable order.
    ///
    /// Ids and nothing else, because the one caller — the periodic mirror
    /// fetch (`ARCHITECTURE.md`, "Background jobs") — re-reads each project
    /// under `fetch_project` anyway and must not act on a row that has gone
    /// stale while the sweep worked through the list. `ORDER BY id` so a sweep
    /// visits projects in the same order every tick, which makes a log of two
    /// consecutive runs comparable.
    pub async fn list_ids_by_status(&self, status: ProjectStatus) -> Result<Vec<Uuid>> {
        let ids = sqlx::query_scalar!(
            r#"SELECT id FROM projects WHERE status = $1 ORDER BY id"#,
            status as ProjectStatus,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(ids)
    }

    /// The ids of every project, whatever its status, in a stable order.
    ///
    /// [`ProjectRepository::list_ids_by_status`]'s companion for the sweeps
    /// that are about what is on disk rather than about what may be fetched:
    /// the orphan-cleanup job's hand-off refs live in the repository of a
    /// project in *any* status, including one stuck in `error` or
    /// `retry-clone`, and leaving those out would leave their leftovers
    /// forever (`ARCHITECTURE.md`, "Background jobs"). `ORDER BY id` for the
    /// same reason: two consecutive runs visit projects in the same order.
    pub async fn list_ids(&self) -> Result<Vec<Uuid>> {
        let ids = sqlx::query_scalar!(r#"SELECT id FROM projects ORDER BY id"#)
            .fetch_all(self.pool)
            .await?;

        Ok(ids)
    }

    /// Apply the set fields of `update` and return the stored row, or `None`
    /// when no project has this id (the route decides whether that is a 404).
    ///
    /// `COALESCE` per column keeps "leave it alone" in the statement rather
    /// than in a query builder, so one prepared statement serves `PUT
    /// /projects/{id}`. `updated_at` moves on every call.
    ///
    /// `remote_url` is deliberately not updatable: the mirror on disk was
    /// cloned from it, so pointing an existing project at a different remote
    /// is a new project, not an edit (`SPEC.md`, "Projects").
    pub async fn update(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        update: &ProjectUpdate,
    ) -> Result<Option<Project>> {
        let updated = sqlx::query_as!(
            Project,
            r#"
            UPDATE projects
            SET name = COALESCE($2, name),
                default_branch = COALESCE($3, default_branch),
                max_attempts = COALESCE($4, max_attempts),
                -- Nullable and clearable, so "leave it alone" cannot be
                -- `COALESCE`: $5 says whether the caller sent the key at all
                -- and $6 is the value, NULL included.
                max_concurrent_sessions = CASE
                    WHEN $5 THEN $6
                    ELSE max_concurrent_sessions
                END,
                automation_paused = COALESCE($7, automation_paused),
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, name, remote_url, default_branch, status as "status: ProjectStatus",
                      status_message, created_by, last_fetched_at, max_attempts,
                      next_task_number, max_concurrent_sessions, automation_paused,
                      created_at, updated_at,
                      EXISTS (
                          SELECT 1 FROM secrets s
                          WHERE s.scope = 'project' AND s.scope_id = projects.id AND s.name = $8
                      ) AS "has_credential!"
            "#,
            id,
            update.name.as_ref().map(|name| name.as_str()),
            update.default_branch.as_ref().map(|branch| branch.as_str()),
            update.max_attempts.map(|attempts| attempts.get()),
            update.max_concurrent_sessions.is_some(),
            update
                .max_concurrent_sessions
                .flatten()
                .map(|cap| cap.get()),
            update.automation_paused,
            GIT_CREDENTIAL_NAME,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_project_error)?;

        debug!(project_id = %id, updated = updated.is_some(), "project updated");

        Ok(updated)
    }

    /// Move the project to `status`, recording `status_message` as the reason.
    ///
    /// The clone and fetch jobs' write. `status_message` is always overwritten,
    /// including with `None`, because a stale reason on a project that has
    /// since become `ready` would be shown to the user as if it were current.
    ///
    /// Moving to `ready` while `default_branch` is null violates the table
    /// `CHECK`: a ready project always has a resolvable integration head. That
    /// is a state conflict, not an internal failure, so it comes back as
    /// [`Error::Conflict`] and the caller discovers the branch first
    /// (`docs/data-model.md`, `projects`).
    pub async fn set_status(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        status: ProjectStatus,
        status_message: Option<&str>,
    ) -> Result<Option<Project>> {
        let updated = sqlx::query_as!(
            Project,
            r#"
            UPDATE projects
            SET status = $2,
                status_message = $3,
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, name, remote_url, default_branch, status as "status: ProjectStatus",
                      status_message, created_by, last_fetched_at, max_attempts,
                      next_task_number, max_concurrent_sessions, automation_paused,
                      created_at, updated_at,
                      EXISTS (
                          SELECT 1 FROM secrets s
                          WHERE s.scope = 'project' AND s.scope_id = projects.id AND s.name = $4
                      ) AS "has_credential!"
            "#,
            id,
            status as ProjectStatus,
            status_message,
            GIT_CREDENTIAL_NAME,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_project_error)?;

        debug!(project_id = %id, status = ?status, updated = updated.is_some(), "project status set");

        Ok(updated)
    }

    /// Finish a clone: `cloning` → `ready` with the branch it discovered.
    ///
    /// The guard is in the `WHERE` clause (`CLAUDE.md`, "Backend
    /// conventions"), so the transition is decided by the database rather than
    /// by a status the caller read earlier: a project deleted, retried or
    /// already finished while the clone ran matches nothing and the count is
    /// 0. **That is not an error.** The clone job has nothing left to do and
    /// stops; treating it as a failure would overwrite a state someone else
    /// deliberately moved to (`SPEC.md`, "Projects").
    ///
    /// `default_branch` is never null here, which is what satisfies the table
    /// `CHECK` that a `ready` project has one, and `last_fetched_at` is set
    /// because the clone that just finished *is* a fetch.
    pub async fn mark_ready(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
        default_branch: &BranchName,
    ) -> Result<u64> {
        let result = sqlx::query!(
            r#"
            UPDATE projects
            SET status = 'ready'::project_status,
                default_branch = $2,
                status_message = NULL,
                last_fetched_at = NOW(),
                updated_at = NOW()
            WHERE id = $1 AND status = 'cloning'::project_status
            "#,
            id,
            default_branch.as_str(),
        )
        .execute(&mut *tx)
        .await
        .map_err(map_project_error)?;

        let rows = result.rows_affected();
        debug!(project_id = %id, rows, "project marked ready");

        Ok(rows)
    }

    /// Fail a clone: `cloning` → `error` with the reason.
    ///
    /// The same guard and the same reading of a 0 count as
    /// [`ProjectRepository::mark_ready`]. `message` is what the user is shown,
    /// so it is the caller's already-sanitised text and never a raw git
    /// invocation (`CLAUDE.md`, rule 3).
    pub async fn mark_error(&self, tx: &mut PgConnection, id: Uuid, message: &str) -> Result<u64> {
        let result = sqlx::query!(
            r#"
            UPDATE projects
            SET status = 'error'::project_status,
                status_message = $2,
                updated_at = NOW()
            WHERE id = $1 AND status = 'cloning'::project_status
            "#,
            id,
            message,
        )
        .execute(&mut *tx)
        .await?;

        let rows = result.rows_affected();
        debug!(project_id = %id, rows, "project marked failed");

        Ok(rows)
    }

    /// Retry a failed clone: `error` → `cloning`, clearing the stale reason.
    ///
    /// `POST /projects/{id}/retry-clone`, which is documented as being
    /// available "only from `error`" (`SPEC.md`, "Projects"). The status is in
    /// the `WHERE` clause, so `None` covers both "no such project" and "not in
    /// `error`" and the route decides which answer that is — under the project
    /// lock it cannot be anything else by the time it answers.
    pub async fn mark_cloning_from_error(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
    ) -> Result<Option<Project>> {
        let updated = sqlx::query_as!(
            Project,
            r#"
            UPDATE projects
            SET status = 'cloning'::project_status,
                status_message = NULL,
                updated_at = NOW()
            WHERE id = $1 AND status = 'error'::project_status
            RETURNING id, name, remote_url, default_branch, status as "status: ProjectStatus",
                      status_message, created_by, last_fetched_at, max_attempts,
                      next_task_number, max_concurrent_sessions, automation_paused,
                      created_at, updated_at,
                      EXISTS (
                          SELECT 1 FROM secrets s
                          WHERE s.scope = 'project' AND s.scope_id = projects.id AND s.name = $2
                      ) AS "has_credential!"
            "#,
            id,
            GIT_CREDENTIAL_NAME,
        )
        .fetch_optional(&mut *tx)
        .await?;

        debug!(project_id = %id, retried = updated.is_some(), "project clone retried");

        Ok(updated)
    }

    /// Record that the mirror was fetched just now.
    ///
    /// The periodic mirror fetch and `POST /projects/{id}/fetch` write this;
    /// it is separate from [`ProjectRepository::set_status`] because a
    /// successful fetch changes nothing else about the project.
    pub async fn set_last_fetched_at(
        &self,
        tx: &mut PgConnection,
        id: Uuid,
    ) -> Result<Option<Project>> {
        let updated = sqlx::query_as!(
            Project,
            r#"
            UPDATE projects
            SET last_fetched_at = NOW(),
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, name, remote_url, default_branch, status as "status: ProjectStatus",
                      status_message, created_by, last_fetched_at, max_attempts,
                      next_task_number, max_concurrent_sessions, automation_paused,
                      created_at, updated_at,
                      EXISTS (
                          SELECT 1 FROM secrets s
                          WHERE s.scope = 'project' AND s.scope_id = projects.id AND s.name = $2
                      ) AS "has_credential!"
            "#,
            id,
            GIT_CREDENTIAL_NAME,
        )
        .fetch_optional(&mut *tx)
        .await?;

        debug!(project_id = %id, updated = updated.is_some(), "project fetch recorded");

        Ok(updated)
    }

    /// Delete a project, reporting whether a row matched.
    ///
    /// `Ok(false)` rather than an error when nothing matched: the route turns
    /// that into 404. Sessions, events, tasks, profiles and shared directories
    /// cascade at the database level; the mirror, the CLI state directory and
    /// the shared directories on disk are the projects epic's to remove after
    /// this returns (`ARCHITECTURE.md`, "Storage").
    pub async fn delete(&self, tx: &mut PgConnection, id: Uuid) -> Result<bool> {
        let result = sqlx::query!("DELETE FROM projects WHERE id = $1", id)
            .execute(&mut *tx)
            .await?;

        let deleted = result.rows_affected() > 0;
        debug!(project_id = %id, deleted, "project deleted");

        Ok(deleted)
    }

    /// Lock the project row `FOR NO KEY UPDATE`, the first statement of every
    /// tracker mutation.
    ///
    /// "Every tracker writer starts a database transaction, locks the project
    /// row with `SELECT ... FOR NO KEY UPDATE`, and only then reads
    /// authoritative state and validates the operation in subsequent
    /// statements under `READ COMMITTED`" (`ARCHITECTURE.md`, "Task tracker";
    /// ADR 0021). Task, state, dependency, comment, claim, release, hand-off
    /// and deletion paths all go through here, and anything they read before
    /// taking this lock is not authoritative.
    ///
    /// **Why not `FOR UPDATE`.** The two conflict with themselves and with each
    /// other, so mutations serialise either way; the difference is that this
    /// one lets a foreign-key check (`FOR KEY SHARE`) on the project through.
    /// A session-only writer that updates its `sessions` row twice in one
    /// transaction makes Postgres check that row's foreign keys again, the
    /// project among them, while a mutation holding the project waits for the
    /// same session row for a foreign key of its own: with `FOR UPDATE` here
    /// that was a deadlock (`ARCHITECTURE.md`, "Task tracker", the lock
    /// strength). No mutation changes a project's key, so none needs more.
    ///
    /// The lock is held until the caller's transaction commits or rolls back.
    /// An unknown project is [`Error::NotFound`] rather than a silent success,
    /// so a mutation cannot proceed unserialised against a project that is not
    /// there.
    ///
    /// **Lock order.** Any git lock is acquired before any database lock, and
    /// an operation spanning several projects takes their rows in UUID order.
    /// Combined tracker and session writes take this lock before any session
    /// row lock, never the other way round.
    pub async fn lock_project(&self, tx: &mut PgConnection, project_id: Uuid) -> Result<()> {
        let locked = sqlx::query_scalar!(
            "SELECT id FROM projects WHERE id = $1 FOR NO KEY UPDATE",
            project_id
        )
        .fetch_optional(&mut *tx)
        .await?;

        if locked.is_none() {
            return Err(Error::NotFound);
        }

        debug!(project_id = %project_id, "project row locked");

        Ok(())
    }

    /// Lock the project row `FOR UPDATE`: [`Self::lock_project`] for a caller
    /// that must also keep new rows from referencing the project.
    ///
    /// Project deletion and the shared-directory clear and delete refuse while
    /// a session is live, and a launch without a task inserts its session
    /// outside any project lock. What makes their count authoritative is that
    /// the insert's foreign-key check (`FOR KEY SHARE`) waits for this lock,
    /// which the weaker tracker lock would let through. It conflicts with
    /// [`Self::lock_project`] as well, so these callers still serialise with
    /// tracker mutations.
    ///
    /// Not for a transaction that goes on to wait for a `sessions` row of a
    /// live session: that is the deadlock [`Self::lock_project`] describes.
    /// Both callers refuse before they would.
    pub async fn lock_project_exclusive(
        &self,
        tx: &mut PgConnection,
        project_id: Uuid,
    ) -> Result<()> {
        let locked = sqlx::query_scalar!(
            "SELECT id FROM projects WHERE id = $1 FOR UPDATE",
            project_id
        )
        .fetch_optional(&mut *tx)
        .await?;

        if locked.is_none() {
            return Err(Error::NotFound);
        }

        debug!(project_id = %project_id, "project row locked exclusively");

        Ok(())
    }

    /// Take the next per-project task number and advance the counter.
    ///
    /// **Call only while holding [`ProjectRepository::lock_project`] in the
    /// same transaction.** The `UPDATE ... RETURNING` is atomic on its own, so
    /// two callers never receive the same number, but the number is only
    /// *correct* inside the mutation that inserts the task with it: a caller
    /// that allocates outside the project lock and then fails leaves a gap,
    /// and one that allocates before validating can hand out a number for a
    /// task that is never inserted (`docs/data-model.md`, "Tracker mutation
    /// transactions").
    ///
    /// This is the one write in this repository that does not touch
    /// `updated_at`: the counter is internal bookkeeping on behalf of the
    /// `tasks` row being inserted, and moving the project's timestamp for it
    /// would make every task insert look like a project edit in the UI.
    pub async fn allocate_task_number(
        &self,
        tx: &mut PgConnection,
        project_id: Uuid,
    ) -> Result<i32> {
        let number = sqlx::query_scalar!(
            r#"
            UPDATE projects
            SET next_task_number = next_task_number + 1
            WHERE id = $1
            RETURNING next_task_number - 1 AS "number!"
            "#,
            project_id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(Error::NotFound)?;

        debug!(project_id = %project_id, number, "task number allocated");

        Ok(number)
    }

    /// Insert a shared directory and return the stored row.
    ///
    /// The row is configuration; the directory under
    /// `/data/projects/<project_id>/shared/<name>` is created lazily at the
    /// next launch (`ARCHITECTURE.md`, "Storage"). Both ways of colliding with
    /// an existing entry — the name, which is the primary key, and the mount
    /// point, which has its own unique index — map to the 409 `SPEC.md`,
    /// "Shared directories" promises, and an unknown project maps to
    /// [`Error::NotFound`].
    pub async fn insert_shared_dir(
        &self,
        tx: &mut PgConnection,
        project_id: Uuid,
        dir: &NewSharedDir,
    ) -> Result<SharedDir> {
        let inserted = sqlx::query_as!(
            SharedDir,
            r#"
            INSERT INTO project_shared_dirs (project_id, name, container_path)
            VALUES ($1, $2, $3)
            RETURNING project_id, name, container_path, created_at
            "#,
            project_id,
            dir.name.as_str(),
            dir.container_path.as_str(),
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(map_shared_dir_error)?;

        debug!(project_id = %project_id, name = %inserted.name, "shared directory inserted");

        Ok(inserted)
    }

    /// The shared directory with this name in this project, or `None`.
    pub async fn find_shared_dir(&self, project_id: Uuid, name: &str) -> Result<Option<SharedDir>> {
        let dir = sqlx::query_as!(
            SharedDir,
            r#"
            SELECT project_id, name, container_path, created_at
            FROM project_shared_dirs
            WHERE project_id = $1 AND name = $2
            "#,
            project_id,
            name,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(dir)
    }

    /// The project's shared directories, by name.
    pub async fn list_shared_dirs(&self, project_id: Uuid) -> Result<Vec<SharedDir>> {
        let dirs = sqlx::query_as!(
            SharedDir,
            r#"
            SELECT project_id, name, container_path, created_at
            FROM project_shared_dirs
            WHERE project_id = $1
            ORDER BY name
            "#,
            project_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(dirs)
    }

    /// Delete a shared directory, reporting whether a row matched.
    ///
    /// Removing the directory and its contents from disk, and refusing the
    /// operation while a session of the project is `running` or `creating`,
    /// are the route's (`SPEC.md`, "Shared directories").
    pub async fn delete_shared_dir(
        &self,
        tx: &mut PgConnection,
        project_id: Uuid,
        name: &str,
    ) -> Result<bool> {
        let result = sqlx::query!(
            "DELETE FROM project_shared_dirs WHERE project_id = $1 AND name = $2",
            project_id,
            name,
        )
        .execute(&mut *tx)
        .await?;

        let deleted = result.rows_affected() > 0;
        debug!(project_id = %project_id, name, deleted, "shared directory deleted");

        Ok(deleted)
    }

    /// Insert an agent profile and return the stored row.
    ///
    /// **Call only while holding [`ProjectRepository::lock_project`] in the
    /// same transaction** (`docs/data-model.md`, "Tracker mutation
    /// transactions"): a profile that arrives as the new default first clears
    /// the flag from the current one, and that pair of statements is only
    /// atomic under the project lock.
    ///
    /// `partial_messages` has no column default, so the value bound here is
    /// [`NewAgentProfile::partial_messages`], which resolves an unset one from
    /// the profile's kind. A duplicate name is the caller's mistake and maps to
    /// the 409 `SPEC.md`, "Agent profiles" documents; the states this profile
    /// serves are a separate table, written by
    /// [`crate::repositories::TaskRepository::set_profile_states_by_name`] in
    /// the same transaction, so the returned row's `serves_states` is empty
    /// until it is.
    ///
    /// The timestamps take the column default `now()` unless
    /// [`NewAgentProfile::created_at`] is set, which is project creation
    /// asking for a listing order its four seeded profiles cannot get from a
    /// transaction-wide `now()` (`docs/data-model.md`, `agent_profiles`).
    pub async fn insert_profile(
        &self,
        tx: &mut PgConnection,
        profile: &NewAgentProfile,
    ) -> Result<AgentProfile> {
        if profile.is_default {
            clear_other_defaults(&mut *tx, profile.project_id, profile.id).await?;
        }

        let inserted = sqlx::query_as!(
            AgentProfile,
            r#"
            INSERT INTO agent_profiles (
                id, project_id, name, kind, backend, model, system_prompt, permission_mode,
                image, runtime, mcp_tools, secrets, partial_messages, idle_timeout_secs,
                is_default, auto_launch, max_concurrent, schedule_cron, schedule_prompt,
                created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                    $18, $19,
                    COALESCE($20::timestamptz, now()), COALESCE($20::timestamptz, now()))
            RETURNING id, project_id, name, kind as "kind: ProfileKind",
                      backend as "backend: AgentBackend", model, system_prompt, permission_mode,
                      image, runtime, mcp_tools, secrets,
                      ARRAY[]::text[] as "serves_states!", partial_messages, idle_timeout_secs,
                      is_default, auto_launch, max_concurrent, schedule_cron, schedule_prompt,
                      last_scheduled_at, created_at, updated_at
            "#,
            profile.id,
            profile.project_id,
            profile.name,
            profile.kind as ProfileKind,
            profile.backend as AgentBackend,
            profile.model,
            profile.system_prompt,
            profile.permission_mode,
            profile.image,
            profile.runtime,
            &profile.mcp_tools[..],
            &profile.secrets[..],
            profile.partial_messages(),
            profile.idle_timeout_secs,
            profile.is_default,
            profile.auto_launch,
            profile.max_concurrent,
            profile.schedule_cron,
            profile.schedule_prompt,
            profile.created_at,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(map_profile_error)?;

        debug!(
            project_id = %profile.project_id,
            profile_id = %inserted.id,
            kind = ?inserted.kind,
            "profile inserted"
        );

        Ok(inserted)
    }

    /// The profile with this id in this project, or `None`.
    ///
    /// The `profile_states` join comes with it: `serves_states` is the state
    /// *names* in board order, which is what `SPEC.md`, "Agent profiles" puts
    /// on the wire. `array_remove(array_agg(...), NULL)` turns the `LEFT JOIN`
    /// of a profile that serves nothing into the empty array rather than a row
    /// of one `NULL`.
    pub async fn find_profile(&self, project_id: Uuid, id: Uuid) -> Result<Option<AgentProfile>> {
        let profile = sqlx::query_as!(
            AgentProfile,
            r#"
            SELECT p.id, p.project_id, p.name, p.kind as "kind: ProfileKind",
                   p.backend as "backend: AgentBackend", p.model, p.system_prompt,
                   p.permission_mode, p.image, p.runtime, p.mcp_tools, p.secrets,
                   array_remove(array_agg(ts.name ORDER BY ts.position), NULL)
                       as "serves_states!",
                   p.partial_messages, p.idle_timeout_secs, p.is_default, p.auto_launch,
                   p.max_concurrent, p.schedule_cron, p.schedule_prompt, p.last_scheduled_at,
                   p.created_at, p.updated_at
            FROM agent_profiles AS p
            LEFT JOIN profile_states AS ps ON ps.profile_id = p.id
            LEFT JOIN task_states AS ts ON ts.id = ps.state_id
            WHERE p.id = $1 AND p.project_id = $2
            GROUP BY p.id
            "#,
            id,
            project_id,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(profile)
    }

    /// The project's default profile, or `None`.
    ///
    /// At most one row can match: `agent_profiles_one_default_idx` is a
    /// partial unique index on `(project_id) WHERE is_default`.
    pub async fn find_default_profile(&self, project_id: Uuid) -> Result<Option<AgentProfile>> {
        let profile = sqlx::query_as!(
            AgentProfile,
            r#"
            SELECT p.id, p.project_id, p.name, p.kind as "kind: ProfileKind",
                   p.backend as "backend: AgentBackend", p.model, p.system_prompt,
                   p.permission_mode, p.image, p.runtime, p.mcp_tools, p.secrets,
                   array_remove(array_agg(ts.name ORDER BY ts.position), NULL)
                       as "serves_states!",
                   p.partial_messages, p.idle_timeout_secs, p.is_default, p.auto_launch,
                   p.max_concurrent, p.schedule_cron, p.schedule_prompt, p.last_scheduled_at,
                   p.created_at, p.updated_at
            FROM agent_profiles AS p
            LEFT JOIN profile_states AS ps ON ps.profile_id = p.id
            LEFT JOIN task_states AS ts ON ts.id = ps.state_id
            WHERE p.project_id = $1 AND p.is_default
            GROUP BY p.id
            "#,
            project_id,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(profile)
    }

    /// The project's profiles, oldest first, each with the states it serves.
    ///
    /// Creation order rather than alphabetical, because it is the order
    /// `SPEC.md`, "Agent profiles" documents for `GET
    /// /projects/{pid}/profiles` and the one that keeps the seeded `default`
    /// profile at the top of the list a user reads, whatever the profiles
    /// added after it are called. The name breaks a tie, so two profiles
    /// created in one transaction — which share `NOW()` — still list in a
    /// fixed order.
    pub async fn list_profiles(&self, project_id: Uuid) -> Result<Vec<AgentProfile>> {
        let profiles = sqlx::query_as!(
            AgentProfile,
            r#"
            SELECT p.id, p.project_id, p.name, p.kind as "kind: ProfileKind",
                   p.backend as "backend: AgentBackend", p.model, p.system_prompt,
                   p.permission_mode, p.image, p.runtime, p.mcp_tools, p.secrets,
                   array_remove(array_agg(ts.name ORDER BY ts.position), NULL)
                       as "serves_states!",
                   p.partial_messages, p.idle_timeout_secs, p.is_default, p.auto_launch,
                   p.max_concurrent, p.schedule_cron, p.schedule_prompt, p.last_scheduled_at,
                   p.created_at, p.updated_at
            FROM agent_profiles AS p
            LEFT JOIN profile_states AS ps ON ps.profile_id = p.id
            LEFT JOIN task_states AS ts ON ts.id = ps.state_id
            WHERE p.project_id = $1
            GROUP BY p.id
            ORDER BY p.created_at, p.name
            "#,
            project_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(profiles)
    }

    /// The project's `auto_launch` profiles, oldest first.
    ///
    /// The dispatcher's scan (`ARCHITECTURE.md`, "Dispatcher"). `created_at`
    /// order is the documented tie-break of that section — "when two
    /// `auto_launch` profiles serve the same state, the older profile by
    /// `agent_profiles.created_at` wins" — so the order is the rule and not a
    /// presentation choice, with the name breaking a tie between two profiles
    /// created in one transaction exactly as [`ProjectRepository::list_profiles`]
    /// does.
    ///
    /// A separate query rather than a filter over that list, because the
    /// predicate is the partial index `agent_profiles_auto_launch_idx
    /// (project_id, created_at) WHERE auto_launch`: a job that ticks over every
    /// ready project reads only the handful of rows that can launch anything,
    /// however many profiles the project has.
    pub async fn list_auto_launch_profiles(&self, project_id: Uuid) -> Result<Vec<AgentProfile>> {
        let profiles = sqlx::query_as!(
            AgentProfile,
            r#"
            SELECT p.id, p.project_id, p.name, p.kind as "kind: ProfileKind",
                   p.backend as "backend: AgentBackend", p.model, p.system_prompt,
                   p.permission_mode, p.image, p.runtime, p.mcp_tools, p.secrets,
                   array_remove(array_agg(ts.name ORDER BY ts.position), NULL)
                       as "serves_states!",
                   p.partial_messages, p.idle_timeout_secs, p.is_default, p.auto_launch,
                   p.max_concurrent, p.schedule_cron, p.schedule_prompt, p.last_scheduled_at,
                   p.created_at, p.updated_at
            FROM agent_profiles AS p
            LEFT JOIN profile_states AS ps ON ps.profile_id = p.id
            LEFT JOIN task_states AS ts ON ts.id = ps.state_id
            WHERE p.project_id = $1 AND p.auto_launch
            GROUP BY p.id
            ORDER BY p.created_at, p.name
            "#,
            project_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(profiles)
    }

    /// The project's profiles that carry a cron schedule, oldest first.
    ///
    /// The scheduler job's scan (`ARCHITECTURE.md`, "Task tracker" →
    /// "Scheduled agents"). `auto_launch` is not consulted: a schedule is a
    /// second, independent reason to launch a profile unattended, and a
    /// profile can have either, both or neither.
    ///
    /// A separate query rather than a filter over
    /// [`ProjectRepository::list_profiles`], and for the same reason
    /// [`ProjectRepository::list_auto_launch_profiles`] is one: the predicate
    /// is the partial index `agent_profiles_schedule_idx (project_id) WHERE
    /// schedule_cron IS NOT NULL`, so a job that ticks over every ready
    /// project once a minute reads only the few rows that can fire.
    pub async fn list_scheduled_profiles(&self, project_id: Uuid) -> Result<Vec<AgentProfile>> {
        let profiles = sqlx::query_as!(
            AgentProfile,
            r#"
            SELECT p.id, p.project_id, p.name, p.kind as "kind: ProfileKind",
                   p.backend as "backend: AgentBackend", p.model, p.system_prompt,
                   p.permission_mode, p.image, p.runtime, p.mcp_tools, p.secrets,
                   array_remove(array_agg(ts.name ORDER BY ts.position), NULL)
                       as "serves_states!",
                   p.partial_messages, p.idle_timeout_secs, p.is_default, p.auto_launch,
                   p.max_concurrent, p.schedule_cron, p.schedule_prompt, p.last_scheduled_at,
                   p.created_at, p.updated_at
            FROM agent_profiles AS p
            LEFT JOIN profile_states AS ps ON ps.profile_id = p.id
            LEFT JOIN task_states AS ts ON ts.id = ps.state_id
            WHERE p.project_id = $1 AND p.schedule_cron IS NOT NULL
            GROUP BY p.id
            ORDER BY p.created_at, p.name
            "#,
            project_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(profiles)
    }

    /// Claim a scheduled tick by advancing `last_scheduled_at` from the value
    /// the caller read to `fired_at`, and answer whether this call is the one
    /// that claimed it.
    ///
    /// The whole of "a tick fires at most once" (`ARCHITECTURE.md`, "Task
    /// tracker" → "Scheduled agents"). One statement, with the old value in
    /// the `WHERE` and no lock taken: two writers that read the same
    /// `last_scheduled_at` cannot both move it, so the second one answers
    /// `false` and launches nothing. That is what makes a restart inside a
    /// tick's own minute safe — the row remembers, not the process.
    ///
    /// `previous` is matched with `IS NOT DISTINCT FROM`, so the very first
    /// tick of a profile, whose column is NULL, is claimed by the same
    /// statement as every later one.
    ///
    /// `schedule_cron IS NOT NULL` is part of the claim rather than a check
    /// before it: a save that cleared the schedule between the scan and the
    /// claim also cleared `last_scheduled_at`, and this is what keeps the
    /// scheduler from writing the column of a profile that no longer has a
    /// schedule ([`ProjectRepository::update_profile`]).
    ///
    /// **The caller launches after this returns `true`, never before.** A
    /// crash in between loses that one run, which is the direction "Scheduled
    /// agents" trades in; a launch first would double it.
    pub async fn claim_schedule_tick(
        &self,
        profile_id: Uuid,
        previous: Option<DateTime<Utc>>,
        fired_at: DateTime<Utc>,
    ) -> Result<bool> {
        let claimed = sqlx::query!(
            r#"
            UPDATE agent_profiles
            SET last_scheduled_at = $3
            WHERE id = $1
              AND last_scheduled_at IS NOT DISTINCT FROM $2
              AND schedule_cron IS NOT NULL
            "#,
            profile_id,
            previous,
            fired_at,
        )
        .execute(self.pool)
        .await?
        .rows_affected();

        Ok(claimed > 0)
    }

    /// Replace a profile's configuration and return the stored row, or `None`
    /// when this project has no such profile.
    ///
    /// **Call only while holding [`ProjectRepository::lock_project`] in the
    /// same transaction**: the current row is read to decide the default flag,
    /// and both the transfer and the refusal below are answers about the
    /// project's profiles as a whole (`docs/data-model.md`, "Tracker mutation
    /// transactions").
    ///
    /// A full replacement, because `PUT /projects/{pid}/profiles/{id}` takes a
    /// whole `ProfileInput`: omitting `model` clears it. `updated_at` moves on
    /// every call.
    ///
    /// `is_default` is the exception to the replacement. `None` leaves the flag
    /// as it is. `Some(true)` takes it from whichever profile has it, so a
    /// project always has exactly one default and the caller never has to clear
    /// the old one first; setting it on the profile that already has it changes
    /// nothing. `Some(false)` on the current default is [`Error::Conflict`]
    /// — a project without a default profile has nothing to launch from, so the
    /// flag is moved, never dropped (`SPEC.md`, "Agent profiles").
    ///
    /// `last_scheduled_at` is not in the update shape at all — it is the
    /// scheduler's own column and read-only over REST — but a save that clears
    /// the schedule clears it too, in the same statement: the guard against
    /// firing one tick twice has nothing left to guard, and a schedule set on
    /// the profile later starts with nothing behind it (ADR 0043). A save that
    /// only *changes* the expression keeps it, because the tick it records
    /// really did fire.
    pub async fn update_profile(
        &self,
        tx: &mut PgConnection,
        project_id: Uuid,
        id: Uuid,
        update: &ProfileUpdate,
    ) -> Result<Option<AgentProfile>> {
        let Some(was_default) = find_profile_is_default(&mut *tx, project_id, id).await? else {
            return Ok(None);
        };

        let is_default = match update.is_default {
            Some(true) => {
                clear_other_defaults(&mut *tx, project_id, id).await?;
                true
            }
            Some(false) if was_default => {
                return Err(Error::Conflict(
                    "project must keep a default profile".into(),
                ));
            }
            Some(false) => false,
            None => was_default,
        };

        let updated = sqlx::query_as!(
            AgentProfile,
            r#"
            WITH updated AS (
                UPDATE agent_profiles
                SET name = $3,
                    kind = $4,
                    backend = $5,
                    model = $6,
                    system_prompt = $7,
                    permission_mode = $8,
                    image = $9,
                    runtime = $10,
                    mcp_tools = $11,
                    secrets = $12,
                    partial_messages = $13,
                    idle_timeout_secs = $14,
                    is_default = $15,
                    auto_launch = $16,
                    max_concurrent = $17,
                    schedule_cron = $18,
                    schedule_prompt = $19,
                    -- Clearing the schedule clears the guard with it: the next
                    -- schedule set on this profile starts with nothing behind
                    -- it (ADR 0043).
                    last_scheduled_at = CASE
                        WHEN $18::text IS NULL THEN NULL
                        ELSE last_scheduled_at
                    END,
                    updated_at = NOW()
                WHERE id = $1 AND project_id = $2
                RETURNING id, project_id, name, kind, backend, model, system_prompt,
                          permission_mode, image, runtime, mcp_tools, secrets, partial_messages,
                          idle_timeout_secs, is_default, auto_launch, max_concurrent,
                          schedule_cron, schedule_prompt, last_scheduled_at, created_at,
                          updated_at
            )
            SELECT u.id, u.project_id, u.name, u.kind as "kind: ProfileKind",
                   u.backend as "backend: AgentBackend", u.model, u.system_prompt,
                   u.permission_mode, u.image, u.runtime, u.mcp_tools, u.secrets,
                   COALESCE(
                       (
                           SELECT array_agg(ts.name ORDER BY ts.position)
                           FROM profile_states AS ps
                           JOIN task_states AS ts ON ts.id = ps.state_id
                           WHERE ps.profile_id = u.id
                       ),
                       '{}'
                   ) as "serves_states!",
                   u.partial_messages, u.idle_timeout_secs, u.is_default, u.auto_launch,
                   u.max_concurrent, u.schedule_cron, u.schedule_prompt, u.last_scheduled_at,
                   u.created_at, u.updated_at
            FROM updated AS u
            "#,
            id,
            project_id,
            update.name,
            update.kind as ProfileKind,
            update.backend as AgentBackend,
            update.model,
            update.system_prompt,
            update.permission_mode,
            update.image,
            update.runtime,
            &update.mcp_tools[..],
            &update.secrets[..],
            update.partial_messages(),
            update.idle_timeout_secs,
            is_default,
            update.auto_launch,
            update.max_concurrent,
            update.schedule_cron,
            update.schedule_prompt,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_profile_error)?;

        debug!(project_id = %project_id, profile_id = %id, updated = updated.is_some(), "profile updated");

        Ok(updated)
    }

    /// Delete a profile, reporting whether a row matched.
    ///
    /// **Call only while holding [`ProjectRepository::lock_project`] in the
    /// same transaction**: both refusals below are read before the `DELETE`,
    /// and without the lock a launch for a task — a mutation of its own —
    /// could put a session on the profile in between. A launch without a task
    /// takes no project lock and can still do that; its uncommitted session
    /// makes the `DELETE` wait, `ON DELETE RESTRICT` then refuses, and
    /// `map_profile_error` answers the same conflict. The launch waits for
    /// nothing this transaction holds, so that race has no cycle in it
    /// (ADR 0041).
    ///
    /// Two profiles cannot be deleted, and each says which it is rather than
    /// letting the client guess (`SPEC.md`, "Agent profiles": 409 "if default
    /// or has sessions"). The project's default profile is what every launch
    /// falls back to, so it is moved with `is_default` on another profile
    /// before it can go. A profile that has ever run a session is held by
    /// `sessions.profile_id`, which is `ON DELETE RESTRICT` because those rows
    /// carry the transcript the UI still shows; the count is read explicitly so
    /// the answer is the documented conflict rather than a database error, and
    /// `map_profile_error` stays as the backstop.
    pub async fn delete_profile(
        &self,
        tx: &mut PgConnection,
        project_id: Uuid,
        id: Uuid,
    ) -> Result<bool> {
        let Some(is_default) = find_profile_is_default(&mut *tx, project_id, id).await? else {
            return Ok(false);
        };

        if is_default {
            return Err(Error::Conflict(
                "the default profile cannot be deleted".into(),
            ));
        }
        if self.profile_session_count(&mut *tx, id).await? > 0 {
            return Err(Error::Conflict("profile has sessions".into()));
        }

        let result = sqlx::query!(
            "DELETE FROM agent_profiles WHERE id = $1 AND project_id = $2",
            id,
            project_id,
        )
        .execute(&mut *tx)
        .await
        .map_err(map_profile_error)?;

        let deleted = result.rows_affected() > 0;
        debug!(project_id = %project_id, profile_id = %id, deleted, "profile deleted");

        Ok(deleted)
    }

    /// How many sessions were launched from this profile, ever.
    ///
    /// The count behind [`ProjectRepository::delete_profile`]'s second refusal,
    /// exposed because the routes want the same answer before they offer the
    /// button: every state counts, `done` and `failed` included, since it is
    /// the row rather than the container that holds the profile
    /// (`docs/data-model.md`, `sessions`).
    pub async fn profile_session_count(&self, tx: &mut PgConnection, id: Uuid) -> Result<i64> {
        let count = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "count!" FROM sessions WHERE profile_id = $1"#,
            id,
        )
        .fetch_one(&mut *tx)
        .await?;

        Ok(count)
    }
}

/// Whether the project's profile `id` exists, and whether it is the default.
///
/// The read both write paths begin with. **Call only under the caller's
/// project lock**: the answer decides whether the default flag moves, and
/// outside the lock it could be stale by the time it is acted on.
async fn find_profile_is_default(
    tx: &mut PgConnection,
    project_id: Uuid,
    id: Uuid,
) -> Result<Option<bool>> {
    let is_default = sqlx::query_scalar!(
        "SELECT is_default FROM agent_profiles WHERE id = $1 AND project_id = $2",
        id,
        project_id,
    )
    .fetch_optional(&mut *tx)
    .await?;

    Ok(is_default)
}

/// Take the default flag off every profile of the project but `keep`.
///
/// `agent_profiles_one_default_idx` allows one default per project, so a
/// caller that sets the flag without clearing the old one would be refused by
/// the index. Clearing first inside the same transaction makes "make this the
/// default" one operation with no window in which the project has two defaults
/// or none, and makes setting the flag on the profile that already has it a
/// no-op (`docs/data-model.md`, `agent_profiles`).
async fn clear_other_defaults(tx: &mut PgConnection, project_id: Uuid, keep: Uuid) -> Result<()> {
    sqlx::query!(
        r#"
        UPDATE agent_profiles
        SET is_default = FALSE, updated_at = NOW()
        WHERE project_id = $1 AND is_default AND id <> $2
        "#,
        project_id,
        keep,
    )
    .execute(&mut *tx)
    .await?;

    Ok(())
}

/// Map the `projects` constraints a caller can break to the documented
/// answers.
///
/// `projects_check` is the unnamed table `CHECK` of the migration, which
/// Postgres names for us: `status <> 'ready' OR default_branch IS NOT NULL`.
/// Anything else — including a unique or check violation on a constraint this
/// function does not know — widens through `#[from] sqlx::Error`, which logs
/// the detail once and answers a generic 500 rather than guessing at a client
/// message.
fn map_project_error(err: sqlx::Error) -> Error {
    if let Some("projects_name_key") = unique_violation(&err) {
        return Error::Conflict("project name already taken".into());
    }
    if let Some("projects_check") = check_violation(&err) {
        return Error::Conflict("default branch unknown".into());
    }

    Error::from(err)
}

/// Map the two ways a shared directory can collide with an existing one, and
/// the one way its project can be missing.
///
/// The name is the table's primary key and the mount point has a unique index
/// of its own, so the two 409s `SPEC.md`, "Shared directories" promises are
/// told apart by constraint name. The foreign key is the third: a directory
/// added to a project that was deleted between the route's lookup and this
/// insert is a 404 for that project, not an internal error.
fn map_shared_dir_error(err: sqlx::Error) -> Error {
    match unique_violation(&err) {
        Some("project_shared_dirs_pkey") => {
            return Error::Conflict("shared directory name already used".into());
        }
        Some("project_shared_dirs_project_id_container_path_key") => {
            return Error::Conflict("container path already used".into());
        }
        _ => {}
    }
    if let Some("project_shared_dirs_project_id_fkey") = foreign_key_violation(&err) {
        return Error::NotFound;
    }

    Error::from(err)
}

/// Map the profile constraints a caller can break.
fn map_profile_error(err: sqlx::Error) -> Error {
    if let Some(constraint) = unique_violation(&err) {
        match constraint {
            "agent_profiles_project_id_name_key" => {
                return Error::Conflict("profile name already taken".into());
            }
            "agent_profiles_one_default_idx" => {
                return Error::Conflict("project already has a default profile".into());
            }
            _ => {}
        }
    }
    if let Some("sessions_profile_id_fkey") = foreign_key_violation(&err) {
        return Error::Conflict("profile has sessions".into());
    }

    Error::from(err)
}
