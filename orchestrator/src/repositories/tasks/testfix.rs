//! The rows the in-crate row-level tests need, and nothing more.
//!
//! A handful of `tasks` rules have no tracker verb above them and no
//! constraint under them — a state must belong to the project, a hand-off to
//! the task, an event to a task of the project — so they are asserted against
//! the statement itself. The statements are `pub(crate)`, because `tracker/`
//! is their only caller, and `tests/` is a separate crate; the assertions
//! therefore live in `#[cfg(test)]` modules beside the statements, and this is
//! their shared arrangement (`CLAUDE.md`, "Testing expectations", "Tracker
//! tests").
//!
//! It is deliberately thin. Everything a *composition* owes the board — the
//! events, the lease, `attempts`, the recomputed graph — is asserted through
//! the verbs in `tests/`, and nothing here reaches for any of it: a project,
//! its default states, and tasks in them.
//!
//! Database from [`testdb`](crate::repositories::testdb), so a lib test
//! process joins the same Postgres as every integration test process of the
//! run. It may `expect`: an arrangement that cannot be built has nothing to
//! hand back.

use uuid::Uuid;

use crate::events::TaskActor;
use crate::models::{NewProject, NewTask, NewTaskComment, NewTaskHandoff, Task, TaskHandoff};
use crate::prelude::*;
use crate::repositories::testdb::{TestDatabase, test_pool};
use crate::repositories::{ProjectRepository, TaskRepository};
use crate::tracker::TrackerMutation;

/// Not a real remote: the fixture value every project test stores (rule 3).
const TEST_REMOTE: &str = "https://git.example.invalid/fake/repo.git";

/// A throw-away database with one project, its default states, and a second
/// project to test the scope rules against.
pub(crate) struct Fixture {
    /// Dropping it takes the database away, so it is held for the test's life.
    _database: TestDatabase,
    pub(crate) pool: PgPool,
    pub(crate) project_id: Uuid,
    /// A second project of the same database: what "another project's state"
    /// and "another project's task" are made of.
    pub(crate) other_project_id: Uuid,
    /// An author for the rows that need one; a hand-off always names who
    /// published it.
    user_id: Uuid,
}

impl Fixture {
    pub(crate) async fn create() -> Self {
        let (database, pool) = test_pool().await;
        let user_id = user(&pool).await;
        let project_id = project(&pool).await;
        let other_project_id = project(&pool).await;

        Self {
            _database: database,
            pool,
            project_id,
            other_project_id,
            user_id,
        }
    }

    /// A task of `project_id`, in that project's default queue state.
    pub(crate) async fn task(&self, project_id: Uuid, title: &str) -> Task {
        let new = NewTask::new(project_id, title).expect("the title parses");

        let mut mutation = TrackerMutation::begin(&self.pool, project_id, TaskActor::System)
            .await
            .expect("the mutation opens");
        let inserted = TaskRepository::new(&self.pool)
            .insert_task(mutation.conn(), project_id, &new)
            .await
            .expect("the task inserts");
        mutation.commit().await.expect("the mutation commits");

        inserted
    }

    /// The id of a state of `project_id`, by name.
    pub(crate) async fn state_id(&self, project_id: Uuid, name: &str) -> Uuid {
        TaskRepository::new(&self.pool)
            .find_state_by_name(project_id, name)
            .await
            .expect("the state reads")
            .expect("the project has this state")
            .id
    }

    /// A `task_handoffs` row on `task`, with the comment it owes.
    ///
    /// Inserted through the two repository helpers that carry the
    /// cross-table scope rules and no verb — which is what stays at the
    /// repository (`CLAUDE.md`, "Testing expectations").
    pub(crate) async fn handoff(&self, project_id: Uuid, task: &Task) -> TaskHandoff {
        let mut mutation = TrackerMutation::begin(&self.pool, project_id, TaskActor::System)
            .await
            .expect("the mutation opens");
        let repository = TaskRepository::new(&self.pool);

        let comment = repository
            .insert_comment(
                mutation.conn(),
                project_id,
                &NewTaskComment::from_system(task.id, "the code is ready"),
            )
            .await
            .expect("the comment inserts");

        // Not a real object id: forty hex digits that no repository holds
        // (rule 3 is about credentials, but a fixture is a fixture).
        let mut handoff = NewTaskHandoff::new(
            task.id,
            "session/fake",
            "0123456789abcdef0123456789abcdef01234567",
            comment.id,
        );
        // A record always names who published it, and a user is the author
        // that needs no session row behind it.
        handoff.created_by_user_id = Some(self.user_id);
        let inserted = repository
            .insert_handoff(mutation.conn(), project_id, &handoff)
            .await
            .expect("the hand-off inserts");
        mutation.commit().await.expect("the mutation commits");

        inserted
    }
}

/// A user row, for the columns that need an author.
///
/// Seeded with an unchecked statement rather than through `UserRepository`:
/// nothing here is about users, and the foreign key is all these tests want
/// from one.
async fn user(pool: &PgPool) -> Uuid {
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, $2, $3, $4)")
        .bind(user_id)
        .bind(format!("user-{}", &user_id.simple().to_string()[..8]))
        .bind(format!("{user_id}@example.test"))
        // Not a credential: an obviously fake stand-in for the Argon2id PHC
        // string a real row would carry (rule 3).
        .bind("$argon2id$fake$hash")
        .execute(pool)
        .await
        .expect("the user seeds");

    user_id
}

/// A project with the documented default states.
async fn project(pool: &PgPool) -> Uuid {
    let new = NewProject::new(
        &format!("project-{}", &Uuid::new_v4().simple().to_string()[..8]),
        TEST_REMOTE,
    )
    .expect("the project is valid");

    let mut tx = pool.begin().await.expect("a transaction begins");
    let project = ProjectRepository::new(pool)
        .insert(&mut tx, &new)
        .await
        .expect("the project inserts");
    tx.commit().await.expect("the transaction commits");

    let mut mutation = TrackerMutation::begin(pool, project.id, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(pool)
        .insert_default_states(mutation.conn(), project.id)
        .await
        .expect("the default states insert");
    mutation.commit().await.expect("the mutation commits");

    project.id
}
