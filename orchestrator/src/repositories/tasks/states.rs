//! `task_states` and `profile_states`: the board's columns and which of them
//! each agent profile serves.
//!
//! `docs/data-model.md`, `task_states` and `profile_states`, and `SPEC.md`,
//! "Task states". Positions are kept packed as `0..n` after every change, so
//! the board order is `ORDER BY position` with no gaps to reason about; a
//! missing position appends, an explicit one shifts the states at and after it
//! (`SPEC.md`). A state's `kind` is immutable, so there is no setter for it.
//!
//! Three invariants belong to the project rather than to any single row, and
//! are therefore checked here under the project lock rather than by a
//! constraint: a project retains at least one queue state, exactly one human
//! state and at least one terminal state (`ARCHITECTURE.md`, "Task tracker").
//! The single human state is the one the database can express — the partial
//! unique index `task_states_one_human_idx` — and its violation is mapped
//! back to the documented 409.
//!
//! `profile_states` is the whole role mechanism: a profile serves queue states
//! of its own project and nothing else. Neither half of that is a constraint
//! the schema can carry, because both span tables, so both are checked here.

use std::collections::HashMap;

use sqlx::PgConnection;
use uuid::Uuid;

use crate::models::{DEFAULT_TASK_STATES, NewTaskState, TaskState, TaskStateKind, TaskStateName};
use crate::prelude::*;
use crate::repositories::tasks::TaskRepository;
use crate::repositories::{foreign_key_violation, unique_violation};
use crate::tracker::Locked;

impl TaskRepository<'_> {
    /// Insert the documented default state set and return it in board order.
    ///
    /// **Call in the transaction that inserts the project**: a project without
    /// states has no default state to put its first task in
    /// ([`crate::projects::create_project`]). That one transaction is the
    /// exception to the rule the rest of this module states, and it is not a
    /// weaker one: [`TaskRepository::begin_mutation`] cannot open it, because
    /// the project row it would lock does not exist yet, and there is nothing
    /// for a lock to serialise against while the row is still invisible to
    /// every other transaction. A caller that seeds the states of a project
    /// that already exists — a test fixture, say — opens the usual mutation.
    ///
    /// The set itself is [`DEFAULT_TASK_STATES`], which is the model's, not
    /// this file's, so the list exists once.
    pub async fn insert_default_states(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
    ) -> Result<Vec<TaskState>> {
        let mut inserted = Vec::with_capacity(DEFAULT_TASK_STATES.len());
        for (name, kind, position) in DEFAULT_TASK_STATES {
            inserted.push(
                insert_state_row(&mut tx, project_id, Uuid::new_v4(), name, kind, position).await?,
            );
        }

        debug!(project_id = %project_id, count = inserted.len(), "default task states inserted");

        Ok(inserted)
    }

    /// Insert one state and return the stored row.
    ///
    /// The token is what makes this answerable: the position it lands at, and
    /// whether the project already has a human state, are both facts about the
    /// project's current state list.
    ///
    /// A `None` position appends. An explicit one shifts every state at or
    /// after it up by one first, so the new state takes that place and the
    /// list stays packed; a position beyond the end appends instead of leaving
    /// a gap. A negative position is [`Error::BadRequest`] — the board has no
    /// column before the first one.
    ///
    /// The two documented 409s (`SPEC.md`, "Task states"): a name already used
    /// in this project, and a second `human` state.
    pub async fn insert_state(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        state: &NewTaskState,
    ) -> Result<TaskState> {
        let end = end_position(&mut tx, project_id).await?;

        let position = match state.position {
            Some(position) if position < 0 => {
                return Err(Error::BadRequest("position must not be negative".into()));
            }
            // Past the end is an append, not a gap.
            Some(position) => position.min(end),
            None => end,
        };

        if state.position.is_some() {
            sqlx::query!(
                "UPDATE task_states SET position = position + 1 WHERE project_id = $1 AND position >= $2",
                project_id,
                position,
            )
            .execute(&mut *tx)
            .await?;
        }

        let inserted = insert_state_row(
            &mut tx,
            project_id,
            state.id,
            state.name.as_str(),
            state.kind,
            position,
        )
        .await?;

        debug!(
            project_id = %project_id,
            state_id = %inserted.id,
            kind = ?inserted.kind,
            position,
            "task state inserted",
        );

        Ok(inserted)
    }

    /// The project's states in board order (`GET /projects/{pid}/task-states`).
    ///
    /// `name` breaks ties, which packed positions make impossible, but it
    /// keeps the order total for free.
    pub async fn list_states(&self, project_id: Uuid) -> Result<Vec<TaskState>> {
        states_in_order(self.pool, project_id).await
    }

    /// The same list, read under the caller's lock.
    ///
    /// What a board edit's `states_changed` payload is built from: the event
    /// carries "the full list after the change" (`SPEC.md`, "TaskEvent"), and
    /// the change is still uncommitted, so the pool read above cannot see it.
    pub async fn list_states_in(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
    ) -> Result<Vec<TaskState>> {
        states_in_order(&mut *tx, project_id).await
    }

    /// The state with this id in this project, or `None`.
    pub async fn find_state(&self, project_id: Uuid, id: Uuid) -> Result<Option<TaskState>> {
        let state = sqlx::query_as!(
            TaskState,
            r#"
            SELECT id, project_id, name, kind as "kind: TaskStateKind", position, created_at
            FROM task_states
            WHERE id = $1 AND project_id = $2
            "#,
            id,
            project_id,
        )
        .fetch_optional(self.pool)
        .await?;

        Ok(state)
    }

    /// The state with this name in this project, or `None`.
    ///
    /// The API and the MCP tools address states by name — `{name}` is the path
    /// segment and `state` on a task is the state's name (`SPEC.md`) — while
    /// the rows reference them by id; this is the translation.
    pub async fn find_state_by_name(
        &self,
        project_id: Uuid,
        name: &str,
    ) -> Result<Option<TaskState>> {
        state_by_name(self.pool, project_id, name).await
    }

    /// The same lookup under the caller's lock.
    ///
    /// Every board edit addresses its state by name and then changes it, so
    /// the resolution belongs inside the mutation: resolved from the pool, a
    /// concurrent rename between the lookup and the change would send the edit
    /// to whatever row now answers to that name.
    pub async fn find_state_by_name_in(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        name: &str,
    ) -> Result<Option<TaskState>> {
        state_by_name(&mut *tx, project_id, name).await
    }

    /// Rename a state, keeping its id, kind and position.
    ///
    /// Renaming renames the state everywhere at once, because tasks and
    /// profiles reference it by id (`SPEC.md`, "Task states"). The new name
    /// being taken in this project is [`Error::Conflict`]; an unknown state is
    /// [`Error::NotFound`].
    ///
    /// There is deliberately no `set_kind`: a state's kind is immutable, and
    /// the route answers 400 when one is supplied.
    pub async fn rename_state(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        id: Uuid,
        name: &TaskStateName,
    ) -> Result<TaskState> {
        let renamed = sqlx::query_as!(
            TaskState,
            r#"
            UPDATE task_states
            SET name = $3
            WHERE id = $1 AND project_id = $2
            RETURNING id, project_id, name, kind as "kind: TaskStateKind", position, created_at
            "#,
            id,
            project_id,
            name.as_str(),
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_state_error)?
        .ok_or(Error::NotFound)?;

        debug!(project_id = %project_id, state_id = %id, "task state renamed");

        Ok(renamed)
    }

    /// Move a state to `position`, re-packing the rest around it.
    ///
    /// The new order is computed from the current one, which is why this
    /// takes the token.
    ///
    /// The list is read in board order, the state is taken out and put back at
    /// `position`, and the whole list is written back as `0..n` in one
    /// statement, so the result is packed whatever the caller asked for. A
    /// position beyond the end moves the state last; a negative one is
    /// [`Error::BadRequest`] and an unknown state [`Error::NotFound`].
    pub async fn move_state(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        id: Uuid,
        position: i32,
    ) -> Result<TaskState> {
        if position < 0 {
            return Err(Error::BadRequest("position must not be negative".into()));
        }

        let mut ordered = state_ids_in_order(&mut tx, project_id).await?;
        let from = ordered
            .iter()
            .position(|state_id| *state_id == id)
            .ok_or(Error::NotFound)?;

        ordered.remove(from);
        // `len()` after the removal, so moving the last state to a position
        // past the end is a no-op rather than an error.
        let to = (position as usize).min(ordered.len());
        ordered.insert(to, id);

        write_positions(&mut tx, project_id, &ordered).await?;

        debug!(project_id = %project_id, state_id = %id, position = to, "task state moved");

        // Read back rather than trusting the arithmetic: the returned row is
        // what the route serialises.
        self.find_state_in_tx(&mut tx, project_id, id).await
    }

    /// Delete a state, refusing the four cases the documents reserve.
    ///
    /// Every refusal below is a count over the project's current rows, and
    /// without the lock a concurrent mutation could remove the last queue
    /// state between the check and the delete.
    ///
    /// A project retains at least one queue state, exactly one human state and
    /// at least one terminal state (`ARCHITECTURE.md`, "Task tracker"), and no
    /// state a task is still in may be removed (`docs/data-model.md`,
    /// `task_states`). All four are [`Error::Conflict`], the 409 `SPEC.md`
    /// promises, with the messages `cannot delete the human state`, `cannot
    /// delete the last queue state`, `cannot delete the last terminal state`
    /// and `state is in use by tasks`. An unknown state is [`Error::NotFound`].
    ///
    /// The in-use check is an `EXISTS` under the lock because it produces the
    /// right message; `tasks.state_id`'s `ON DELETE RESTRICT` is the backstop
    /// for anything that slips between the two, and maps to the same conflict.
    ///
    /// Positions are re-packed to `0..n` afterwards, so the board keeps no gap
    /// where the column was.
    pub async fn delete_state(&self, mut tx: Locked<'_>, project_id: Uuid, id: Uuid) -> Result<()> {
        let state = self.find_state_in_tx(&mut tx, project_id, id).await?;

        match state.kind {
            TaskStateKind::Human => {
                return Err(Error::Conflict("cannot delete the human state".into()));
            }
            TaskStateKind::Queue
                if count_kind(&mut tx, project_id, TaskStateKind::Queue).await? <= 1 =>
            {
                return Err(Error::Conflict("cannot delete the last queue state".into()));
            }
            TaskStateKind::Terminal
                if count_kind(&mut tx, project_id, TaskStateKind::Terminal).await? <= 1 =>
            {
                return Err(Error::Conflict(
                    "cannot delete the last terminal state".into(),
                ));
            }
            _ => {}
        }

        let in_use = sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM tasks WHERE state_id = $1) AS "in_use!""#,
            id,
        )
        .fetch_one(&mut *tx)
        .await?;
        if in_use {
            return Err(Error::Conflict("state is in use by tasks".into()));
        }

        sqlx::query!(
            "DELETE FROM task_states WHERE id = $1 AND project_id = $2",
            id,
            project_id,
        )
        .execute(&mut *tx)
        .await
        .map_err(map_state_error)?;

        let ordered = state_ids_in_order(&mut tx, project_id).await?;
        write_positions(&mut tx, project_id, &ordered).await?;

        debug!(project_id = %project_id, state_id = %id, "task state deleted");

        Ok(())
    }

    /// The state a new task lands in: the queue state with the lowest position.
    ///
    /// Read under the same token as the insert that follows it
    /// (`docs/data-model.md`,
    /// `task_states`: "The default state for a new task is the `queue` state
    /// with the lowest position"). `backlog` by default.
    ///
    /// [`Error::NotFound`] when the project has no queue state, which
    /// [`TaskRepository::delete_state`] makes unreachable for a project this
    /// crate created, and which is still not an insert this repository can
    /// make up a state for.
    pub async fn default_state(&self, mut tx: Locked<'_>, project_id: Uuid) -> Result<TaskState> {
        let state = sqlx::query_as!(
            TaskState,
            r#"
            SELECT id, project_id, name, kind as "kind: TaskStateKind", position, created_at
            FROM task_states
            WHERE project_id = $1 AND kind = 'queue'
            ORDER BY position, name
            LIMIT 1
            "#,
            project_id,
        )
        .fetch_optional(&mut *tx)
        .await?;

        state.ok_or(Error::NotFound)
    }

    /// Replace the set of states a profile serves.
    ///
    /// The states are validated against the project's current list, and a
    /// concurrent state deletion would otherwise land between the check and
    /// the insert.
    ///
    /// Every state must belong to `project_id` and be a `queue` state — agents
    /// claim from queues, and serving the human state would defeat the point
    /// of escalation (`docs/data-model.md`, `profile_states`). A state that is
    /// neither is [`Error::BadRequest`] with `served states must be queue
    /// states of this project`; a profile of another project is
    /// [`Error::NotFound`], the answer every out-of-scope id gets.
    ///
    /// An empty slice is valid and clears the links: a profile that serves
    /// nothing is never offered a task by `ready`, which is exactly what a
    /// profile meant for hand-launched sessions wants.
    pub async fn set_profile_states(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        profile_id: Uuid,
        state_ids: &[Uuid],
    ) -> Result<()> {
        let profile_in_project = sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM agent_profiles WHERE id = $1 AND project_id = $2
            ) AS "exists!"
            "#,
            profile_id,
            project_id,
        )
        .fetch_one(&mut *tx)
        .await?;
        if !profile_in_project {
            return Err(Error::NotFound);
        }

        // Counted over the distinct ids, so a repeated id is not mistaken for
        // a missing one; the primary key would reject the duplicate anyway.
        let mut distinct: Vec<Uuid> = state_ids.to_vec();
        distinct.sort_unstable();
        distinct.dedup();

        let servable = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) AS "count!"
            FROM task_states
            WHERE project_id = $1 AND kind = 'queue' AND id = ANY($2)
            "#,
            project_id,
            &distinct[..],
        )
        .fetch_one(&mut *tx)
        .await?;

        if servable != distinct.len() as i64 {
            return Err(Error::BadRequest(
                "served states must be queue states of this project".into(),
            ));
        }

        sqlx::query!(
            "DELETE FROM profile_states WHERE profile_id = $1",
            profile_id
        )
        .execute(&mut *tx)
        .await?;

        if !distinct.is_empty() {
            sqlx::query!(
                "INSERT INTO profile_states (profile_id, state_id) SELECT $1, * FROM UNNEST($2::uuid[])",
                profile_id,
                &distinct[..],
            )
            .execute(&mut *tx)
            .await?;
        }

        debug!(
            project_id = %project_id,
            profile_id = %profile_id,
            count = distinct.len(),
            "profile served states set",
        );

        Ok(())
    }

    /// [`TaskRepository::set_profile_states`] addressed by state *name*, which
    /// is how `ProfileInput.serves_states` arrives.
    ///
    /// Under the token for the same reason: the names are resolved against
    /// the project's current states, and a rename or a deletion between the
    /// lookup and the insert would write a link the caller never asked for
    /// (`docs/data-model.md`, "Tracker mutation transactions" lists profile
    /// served states).
    ///
    /// A name that is not a state of this project, and a name that is a state
    /// but not a `queue` one, are the same mistake to the caller and get the
    /// same [`Error::BadRequest`] — naming the entry and listing the project's
    /// queue states in board order, because "not a queue state" is only useful
    /// next to the ones that are (`SPEC.md`, "Agent profiles": 400 when
    /// `serves_states` entries "must be names of the project's `queue`
    /// states"). That covers the default `["ready"]` against a project whose
    /// `ready` state has been renamed or removed: the caller sees which names
    /// it may use.
    ///
    /// Resolution is all this adds. The link rows are written by
    /// [`TaskRepository::set_profile_states`], so there is one path that
    /// deletes and inserts `profile_states`, and the profile's own scope check
    /// happens there.
    pub async fn set_profile_states_by_name(
        &self,
        mut tx: Locked<'_>,
        project_id: Uuid,
        profile_id: Uuid,
        names: &[String],
    ) -> Result<()> {
        let rows = sqlx::query!(
            r#"
            SELECT id, name, kind as "kind: TaskStateKind"
            FROM task_states
            WHERE project_id = $1 AND name = ANY($2)
            "#,
            project_id,
            names,
        )
        .fetch_all(&mut *tx)
        .await?;

        let mut state_ids = Vec::with_capacity(names.len());
        for name in names {
            match rows.iter().find(|row| row.name == *name) {
                Some(row) if row.kind == TaskStateKind::Queue => state_ids.push(row.id),
                _ => {
                    let queues = queue_state_names(&mut tx, project_id).await?;
                    return Err(Error::BadRequest(format!(
                        "serves_states: \"{name}\" is not a queue state of this project; \
                         queue states are: {}",
                        queues.join(", "),
                    )));
                }
            }
        }

        self.set_profile_states(tx, project_id, profile_id, &state_ids)
            .await
    }

    /// The states a profile serves, in board order.
    pub async fn list_profile_states(&self, profile_id: Uuid) -> Result<Vec<TaskState>> {
        let states = sqlx::query_as!(
            TaskState,
            r#"
            SELECT s.id, s.project_id, s.name, s.kind as "kind: TaskStateKind", s.position,
                   s.created_at
            FROM profile_states AS p
            JOIN task_states AS s ON s.id = p.state_id
            WHERE p.profile_id = $1
            ORDER BY s.position, s.name
            "#,
            profile_id,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(states)
    }

    /// Every profile of the project mapped to the state ids it serves, in
    /// board order.
    ///
    /// One query for the profile list DTO, instead of one
    /// [`TaskRepository::list_profile_states`] per profile. A profile that
    /// serves nothing has no entry; the caller reads a missing key as the
    /// empty list it is.
    pub async fn list_profile_state_ids_for_project(
        &self,
        project_id: Uuid,
    ) -> Result<HashMap<Uuid, Vec<Uuid>>> {
        let rows = sqlx::query!(
            r#"
            SELECT p.profile_id, p.state_id
            FROM profile_states AS p
            JOIN task_states AS s ON s.id = p.state_id
            WHERE s.project_id = $1
            ORDER BY p.profile_id, s.position, s.name
            "#,
            project_id,
        )
        .fetch_all(self.pool)
        .await?;

        let mut served: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        for row in rows {
            served.entry(row.profile_id).or_default().push(row.state_id);
        }

        Ok(served)
    }

    /// [`TaskRepository::find_state`] under the caller's lock, as a hard
    /// requirement rather than an `Option`.
    ///
    /// The refusal paths below all begin by reading the state they are about
    /// to change, and all answer [`Error::NotFound`] when it is not there, so
    /// the unwrapping lives here once.
    async fn find_state_in_tx(
        &self,
        tx: &mut PgConnection,
        project_id: Uuid,
        id: Uuid,
    ) -> Result<TaskState> {
        let state = sqlx::query_as!(
            TaskState,
            r#"
            SELECT id, project_id, name, kind as "kind: TaskStateKind", position, created_at
            FROM task_states
            WHERE id = $1 AND project_id = $2
            "#,
            id,
            project_id,
        )
        .fetch_optional(&mut *tx)
        .await?;

        state.ok_or(Error::NotFound)
    }
}

/// The project's states in board order, from the pool or from inside a
/// mutation.
///
/// One query text for both readers, so the board order is defined once and
/// `.sqlx/` carries one entry for it.
async fn states_in_order<'e, E>(executor: E, project_id: Uuid) -> Result<Vec<TaskState>>
where
    E: sqlx::PgExecutor<'e>,
{
    let states = sqlx::query_as!(
        TaskState,
        r#"
        SELECT id, project_id, name, kind as "kind: TaskStateKind", position, created_at
        FROM task_states
        WHERE project_id = $1
        ORDER BY position, name
        "#,
        project_id,
    )
    .fetch_all(executor)
    .await?;

    Ok(states)
}

/// One state of this project by name, from the pool or from inside a
/// mutation.
async fn state_by_name<'e, E>(
    executor: E,
    project_id: Uuid,
    name: &str,
) -> Result<Option<TaskState>>
where
    E: sqlx::PgExecutor<'e>,
{
    let state = sqlx::query_as!(
        TaskState,
        r#"
        SELECT id, project_id, name, kind as "kind: TaskStateKind", position, created_at
        FROM task_states
        WHERE project_id = $1 AND name = $2
        "#,
        project_id,
        name,
    )
    .fetch_optional(executor)
    .await?;

    Ok(state)
}

/// The one `INSERT` into `task_states`, shared by the default set and the
/// caller-supplied one.
async fn insert_state_row(
    tx: &mut PgConnection,
    project_id: Uuid,
    id: Uuid,
    name: &str,
    kind: TaskStateKind,
    position: i32,
) -> Result<TaskState> {
    let inserted = sqlx::query_as!(
        TaskState,
        r#"
        INSERT INTO task_states (id, project_id, name, kind, position)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, project_id, name, kind as "kind: TaskStateKind", position, created_at
        "#,
        id,
        project_id,
        name,
        kind as TaskStateKind,
        position,
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(map_state_error)?;

    Ok(inserted)
}

/// One past the project's last position: where an append lands.
async fn end_position(tx: &mut PgConnection, project_id: Uuid) -> Result<i32> {
    let end = sqlx::query_scalar!(
        r#"
        SELECT COALESCE(MAX(position), -1) + 1 AS "end!"
        FROM task_states
        WHERE project_id = $1
        "#,
        project_id,
    )
    .fetch_one(&mut *tx)
    .await?;

    Ok(end)
}

/// The project's state ids in board order.
async fn state_ids_in_order(tx: &mut PgConnection, project_id: Uuid) -> Result<Vec<Uuid>> {
    let ids = sqlx::query_scalar!(
        "SELECT id FROM task_states WHERE project_id = $1 ORDER BY position, name",
        project_id,
    )
    .fetch_all(&mut *tx)
    .await?;

    Ok(ids)
}

/// Write `ordered` back as positions `0..n`, in one statement.
///
/// The re-packing every move and delete ends with: the caller decides the
/// order, this decides the numbers, and no row is left with a gap or a
/// duplicate behind it.
async fn write_positions(tx: &mut PgConnection, project_id: Uuid, ordered: &[Uuid]) -> Result<()> {
    if ordered.is_empty() {
        return Ok(());
    }

    let positions: Vec<i32> = (0..ordered.len() as i32).collect();

    sqlx::query!(
        r#"
        UPDATE task_states AS s
        SET position = packed.position
        FROM UNNEST($2::uuid[], $3::int[]) AS packed (id, position)
        WHERE s.id = packed.id AND s.project_id = $1 AND s.position <> packed.position
        "#,
        project_id,
        ordered,
        &positions[..],
    )
    .execute(&mut *tx)
    .await?;

    Ok(())
}

/// The project's `queue` state names in board order: the ones a profile may
/// serve, for the message that says a name is not one of them.
async fn queue_state_names(tx: &mut PgConnection, project_id: Uuid) -> Result<Vec<String>> {
    let names = sqlx::query_scalar!(
        r#"
        SELECT name
        FROM task_states
        WHERE project_id = $1 AND kind = 'queue'
        ORDER BY position, name
        "#,
        project_id,
    )
    .fetch_all(&mut *tx)
    .await?;

    Ok(names)
}

/// How many states of this kind the project has.
async fn count_kind(tx: &mut PgConnection, project_id: Uuid, kind: TaskStateKind) -> Result<i64> {
    let count = sqlx::query_scalar!(
        r#"
        SELECT COUNT(*) AS "count!"
        FROM task_states
        WHERE project_id = $1 AND kind = $2
        "#,
        project_id,
        kind as TaskStateKind,
    )
    .fetch_one(&mut *tx)
    .await?;

    Ok(count)
}

/// Map the `task_states` constraints a caller can break to the documented
/// answers (`SPEC.md`, "Task states").
///
/// `task_states_one_human_idx` is the partial unique index that allows at most
/// one human state per project; `task_states_project_id_name_key` is the
/// `UNIQUE (project_id, name)` of the migration. `tasks_state_id_fkey` is the
/// `ON DELETE RESTRICT` backstop under
/// [`TaskRepository::delete_state`]'s own `EXISTS` check, and answers the same
/// conflict so the two paths are indistinguishable to the client.
fn map_state_error(err: sqlx::Error) -> Error {
    if let Some(constraint) = unique_violation(&err) {
        match constraint {
            "task_states_project_id_name_key" => {
                return Error::Conflict("state name already taken".into());
            }
            "task_states_one_human_idx" => {
                return Error::Conflict("project already has a human state".into());
            }
            _ => {}
        }
    }
    if let Some("tasks_state_id_fkey") = foreign_key_violation(&err) {
        return Error::Conflict("state is in use by tasks".into());
    }

    Error::from(err)
}
