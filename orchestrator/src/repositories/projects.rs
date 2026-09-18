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
                      next_task_number, created_at, updated_at,
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
        let project = sqlx::query_as!(
            Project,
            r#"
            SELECT p.id, p.name, p.remote_url, p.default_branch,
                   p.status as "status: ProjectStatus", p.status_message, p.created_by,
                   p.last_fetched_at, p.max_attempts, p.next_task_number, p.created_at,
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
        .fetch_optional(self.pool)
        .await?;

        Ok(project)
    }

    /// Every project, by name (`GET /projects`).
    ///
    /// `id` breaks ties, which the unique index on `name` makes impossible
    /// today but costs nothing and keeps the order total.
    pub async fn list(&self) -> Result<Vec<Project>> {
        let projects = sqlx::query_as!(
            Project,
            r#"
            SELECT p.id, p.name, p.remote_url, p.default_branch,
                   p.status as "status: ProjectStatus", p.status_message, p.created_by,
                   p.last_fetched_at, p.max_attempts, p.next_task_number, p.created_at,
                   p.updated_at,
                   EXISTS (
                       SELECT 1 FROM secrets s
                       WHERE s.scope = 'project' AND s.scope_id = p.id AND s.name = $1
                   ) AS "has_credential!"
            FROM projects p
            ORDER BY p.name, p.id
            "#,
            GIT_CREDENTIAL_NAME,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(projects)
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
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, name, remote_url, default_branch, status as "status: ProjectStatus",
                      status_message, created_by, last_fetched_at, max_attempts,
                      next_task_number, created_at, updated_at,
                      EXISTS (
                          SELECT 1 FROM secrets s
                          WHERE s.scope = 'project' AND s.scope_id = projects.id AND s.name = $5
                      ) AS "has_credential!"
            "#,
            id,
            update.name.as_ref().map(|name| name.as_str()),
            update.default_branch.as_ref().map(|branch| branch.as_str()),
            update.max_attempts.map(|attempts| attempts.get()),
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
                      next_task_number, created_at, updated_at,
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
                      next_task_number, created_at, updated_at,
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
                      next_task_number, created_at, updated_at,
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

    /// Lock the project row `FOR UPDATE`, the first statement of every tracker
    /// mutation.
    ///
    /// "Every tracker writer starts a database transaction, locks the project
    /// row with `SELECT ... FOR UPDATE`, and only then reads authoritative
    /// state and validates the operation in subsequent statements under `READ
    /// COMMITTED`" (`ARCHITECTURE.md`, "Task tracker"; ADR 0021). Task,
    /// state, dependency, comment, claim, release, hand-off and deletion paths
    /// all go through here, and anything they read before taking this lock is
    /// not authoritative.
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
            "SELECT id FROM projects WHERE id = $1 FOR UPDATE",
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
    /// `partial_messages` has no column default, so the value bound here is
    /// [`NewAgentProfile::partial_messages`], which resolves an unset one from
    /// the profile's kind. A duplicate name and a second default profile are
    /// both the caller's mistake and map to the 409s `SPEC.md`, "Agent
    /// profiles" documents; the states this profile serves are a separate
    /// table, written by the tracker's repository in the same transaction.
    pub async fn insert_profile(
        &self,
        tx: &mut PgConnection,
        profile: &NewAgentProfile,
    ) -> Result<AgentProfile> {
        let inserted = sqlx::query_as!(
            AgentProfile,
            r#"
            INSERT INTO agent_profiles (
                id, project_id, name, kind, backend, model, system_prompt, permission_mode,
                image, runtime, mcp_tools, secrets, partial_messages, idle_timeout_secs,
                is_default
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            RETURNING id, project_id, name, kind as "kind: ProfileKind",
                      backend as "backend: AgentBackend", model, system_prompt, permission_mode,
                      image, runtime, mcp_tools, secrets, partial_messages, idle_timeout_secs,
                      is_default, created_at, updated_at
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
    pub async fn find_profile(&self, project_id: Uuid, id: Uuid) -> Result<Option<AgentProfile>> {
        let profile = sqlx::query_as!(
            AgentProfile,
            r#"
            SELECT id, project_id, name, kind as "kind: ProfileKind",
                   backend as "backend: AgentBackend", model, system_prompt, permission_mode,
                   image, runtime, mcp_tools, secrets, partial_messages, idle_timeout_secs,
                   is_default, created_at, updated_at
            FROM agent_profiles
            WHERE id = $1 AND project_id = $2
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
            SELECT id, project_id, name, kind as "kind: ProfileKind",
                   backend as "backend: AgentBackend", model, system_prompt, permission_mode,
                   image, runtime, mcp_tools, secrets, partial_messages, idle_timeout_secs,
                   is_default, created_at, updated_at
            FROM agent_profiles
            WHERE project_id = $1 AND is_default
            "#,
            project_id,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(profile)
    }

    /// The project's profiles, by name.
    pub async fn list_profiles(&self, project_id: Uuid) -> Result<Vec<AgentProfile>> {
        let profiles = sqlx::query_as!(
            AgentProfile,
            r#"
            SELECT id, project_id, name, kind as "kind: ProfileKind",
                   backend as "backend: AgentBackend", model, system_prompt, permission_mode,
                   image, runtime, mcp_tools, secrets, partial_messages, idle_timeout_secs,
                   is_default, created_at, updated_at
            FROM agent_profiles
            WHERE project_id = $1
            ORDER BY name
            "#,
            project_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(profiles)
    }

    /// Replace a profile's configuration and return the stored row, or `None`
    /// when this project has no such profile.
    ///
    /// A full replacement, because `PUT /projects/{pid}/profiles/{id}` takes a
    /// whole `ProfileInput`: omitting `model` clears it. `updated_at` moves on
    /// every call, and the two conflicts are the same ones an insert can hit.
    pub async fn update_profile(
        &self,
        tx: &mut PgConnection,
        project_id: Uuid,
        id: Uuid,
        update: &ProfileUpdate,
    ) -> Result<Option<AgentProfile>> {
        let updated = sqlx::query_as!(
            AgentProfile,
            r#"
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
                updated_at = NOW()
            WHERE id = $1 AND project_id = $2
            RETURNING id, project_id, name, kind as "kind: ProfileKind",
                      backend as "backend: AgentBackend", model, system_prompt, permission_mode,
                      image, runtime, mcp_tools, secrets, partial_messages, idle_timeout_secs,
                      is_default, created_at, updated_at
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
            update.is_default,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_profile_error)?;

        debug!(project_id = %project_id, profile_id = %id, updated = updated.is_some(), "profile updated");

        Ok(updated)
    }

    /// Delete a profile, reporting whether a row matched.
    ///
    /// `sessions.profile_id` is `ON DELETE RESTRICT`, so a profile that has
    /// ever run a session cannot be deleted while those session rows exist:
    /// they carry the transcript the UI still shows. That is a 409, not a 500
    /// (`SPEC.md`, "Agent profiles"). Refusing to delete the *default* profile
    /// is the route's rule, not a database constraint.
    pub async fn delete_profile(
        &self,
        tx: &mut PgConnection,
        project_id: Uuid,
        id: Uuid,
    ) -> Result<bool> {
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
