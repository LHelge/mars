//! A tracker mutation holds exactly one pooled connection (`CLAUDE.md`,
//! "Testing expectations").
//!
//! `ARCHITECTURE.md`, "Task tracker" → "One mutation at a time per project"
//! and `docs/data-model.md`, "Tracker mutation transactions": a mutation locks
//! its project row and shares that one transaction with every helper it calls.
//! The lock serialises mutations *of one project* and nothing else — two
//! projects are meant to be changed at the same time.
//!
//! That only holds while a mutation needs one connection. A read taken on the
//! pool while the transaction is open makes it hold two, and then as many
//! concurrent mutations as the pool has connections each hold one and wait for
//! a second that nobody can give back: every one of them stalls until the
//! acquire timeout, on projects that never contend for a lock at all.
//!
//! So this is the shape of that failure rather than a test of any one read:
//! a pool of `POOL_SIZE` connections, `POOL_SIZE` mutations on distinct
//! projects, each held open past a barrier so they really are simultaneous,
//! and each then doing the work a caller does — resolve a state by name, read
//! the task under the lock, move it. All of them finish well inside the pool's
//! acquire timeout, or this test fails where the deadlock is, instead of the
//! suite slowing down somewhere else.
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use mars_orchestrator::events::TaskActor;
use mars_orchestrator::models::{NewTask, Task, TaskRef};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::TaskRepository;
use mars_orchestrator::tracker::TrackerMutation;
use mars_orchestrator::tracker::state::{StateChangeOptions, change_state, resolve_state};
use tokio::sync::Barrier;
use uuid::Uuid;

/// The pool every mutation below shares, and the number of mutations: as many
/// as there are connections, which is the count that exhausts it.
const POOL_SIZE: u32 = 4;

/// Well inside the pool's own acquire timeout, which sqlx defaults to 30s: a
/// mutation waiting for a second connection blows this long before it gives
/// up, so the failure is named here rather than surfacing as a slow suite.
const WELL_INSIDE_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// A project with the documented default states and one task to move.
struct Fixture {
    project_id: Uuid,
    task: Task,
}

/// One project, its default states and a task in the default queue state.
async fn seed(pool: &PgPool) -> Fixture {
    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        // `.invalid` can never resolve (`CLAUDE.md`, rule 3).
        .bind("https://example.invalid/org/repo.git")
        .execute(pool)
        .await
        .expect("the project seeds");

    let repository = TaskRepository::new(pool);

    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    repository
        .insert_default_states(mutation.conn(), project_id)
        .await
        .expect("the default states insert");
    mutation.commit().await.expect("the mutation commits");

    let new = NewTask::new(project_id, "a task to move").expect("the title parses");
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let task = repository
        .insert_task(mutation.conn(), project_id, &new)
        .await
        .expect("the task inserts");
    mutation.commit().await.expect("the mutation commits");

    Fixture { project_id, task }
}

/// One caller's whole mutation: open it, wait for its siblings to have opened
/// theirs, then do the reads and the write.
///
/// The barrier is what makes the test deterministic. Without it the mutations
/// could run one after another and each find a free connection for its second
/// read; with it, every one of them is holding a connection before any of them
/// asks for another.
async fn move_task(pool: PgPool, fixture: Fixture, barrier: Arc<Barrier>) -> Result<()> {
    let project_id = fixture.project_id;
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System).await?;

    barrier.wait().await;

    let state = resolve_state(&mut mutation, "review").await?;
    let task = TaskRepository::new(mutation.pool())
        .find_task_for_update(mutation.conn(), project_id, TaskRef::Id(fixture.task.id))
        .await?
        .ok_or(Error::NotFound)?;

    change_state(&mut mutation, &task, &state, StateChangeOptions::default()).await?;

    mutation.commit().await?;

    Ok(())
}

#[tokio::test]
async fn a_mutation_per_pooled_connection_all_finish() {
    let (_postgres, pool) = common::db::test_pool_with(POOL_SIZE).await;

    let mut fixtures = Vec::new();
    for _ in 0..POOL_SIZE {
        fixtures.push(seed(&pool).await);
    }

    let barrier = Arc::new(Barrier::new(POOL_SIZE as usize));
    let handles = fixtures
        .into_iter()
        .map(|fixture| tokio::spawn(move_task(pool.clone(), fixture, Arc::clone(&barrier))))
        .collect::<Vec<_>>();

    let moved = tokio::time::timeout(WELL_INSIDE_ACQUIRE_TIMEOUT, async {
        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.expect("the task does not panic"));
        }
        results
    })
    .await
    .expect("every concurrent mutation finishes well inside the pool acquire timeout");

    for result in moved {
        result.expect("the mutation succeeds");
    }

    // And the moves really happened: every task is in `review`, with the one
    // `state_changed` its mutation committed.
    let repository = TaskRepository::new(&pool);
    let states = sqlx::query_scalar::<_, String>(
        "SELECT s.name FROM tasks AS t JOIN task_states AS s ON s.id = t.state_id",
    )
    .fetch_all(&pool)
    .await
    .expect("the states read");

    assert_eq!(states.len(), POOL_SIZE as usize);
    for state in states {
        assert_eq!(state, "review", "every task moved");
    }

    let kinds = sqlx::query_scalar::<_, String>("SELECT kind FROM task_events")
        .fetch_all(&pool)
        .await
        .expect("the events read");
    assert_eq!(
        kinds.iter().filter(|kind| *kind == "state_changed").count(),
        POOL_SIZE as usize,
        "each mutation committed exactly one state change",
    );

    // The pool is intact afterwards: nothing leaked a connection.
    assert!(
        repository.list_states(Uuid::new_v4()).await.is_ok(),
        "the pool still serves reads",
    );
}
