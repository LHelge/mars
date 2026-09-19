//! The dependency-graph rules against a real Postgres (`CLAUDE.md`, "Testing
//! expectations").
//!
//! `tracker::graph` is what every later tracker mutation leans on, so what is
//! asserted here is the contract the rest of the tracker is allowed to assume
//! (`ARCHITECTURE.md`, "Task tracker" → "Blocked is stored" and "Parents";
//! `docs/data-model.md`, `tasks.blocked` and `task_dependencies`):
//!
//! - `blocked` is a function of the graph and nothing else — an open `blocks`
//!   prerequisite or an open child sets it, a terminal one does not — and it
//!   is written, with an event, only when it actually flips;
//! - the set a terminal move affects is "dependants plus parent", counted
//!   once when a task is both;
//! - a `blocks` edge that would close a ring is refused with 409, while the
//!   same ring in `related` edges is nobody's problem;
//! - a deletion can read who was waiting on it, and with which kind, while
//!   the rows are still there;
//! - and the one race the whole design turns on: two reciprocal `blocks`
//!   edges inserted at once cannot both pass, because they serialise on the
//!   project lock and the second one's walk sees the first edge (ADR 0021).
//!
//! Needs a container engine; see `tests/common/db.rs`.

mod common;

use axum::http::StatusCode;
use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::models::{NewTask, Task, TaskDependencyKind};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::TaskRepository;
use mars_orchestrator::repositories::tasks::test_support::{StateFields, TaskRepositoryTestExt};
use mars_orchestrator::tracker::graph::{
    BlockedFlip, affected_by_state_change, capture_before_delete, check_no_cycle, dependants_of,
    recompute_blocked,
};
use mars_orchestrator::tracker::{Locked, TrackerMutation};
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the Argon2id PHC string
/// the seeded user would carry (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// A project with the documented default state set, and the user every row is
/// attributed to.
async fn seed(pool: &PgPool) -> Uuid {
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, $2, $3, $4)")
        .bind(user_id)
        .bind(format!("user-{}", &user_id.simple().to_string()[..8]))
        .bind(format!("{user_id}@example.test"))
        .bind(FAKE_PASSWORD_HASH)
        .execute(pool)
        .await
        .expect("the user seeds");

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        // `.invalid` can never resolve (`CLAUDE.md`, rule 3).
        .bind("https://example.invalid/org/repo.git")
        .execute(pool)
        .await
        .expect("the project seeds");

    in_mutation(pool, project_id, async |repository, tx| {
        repository
            .insert_default_states(tx, project_id)
            .await
            .expect("the default states insert");
    })
    .await;

    project_id
}

/// Run `body` inside a committed tracker mutation.
async fn in_mutation<T, F>(pool: &PgPool, project_id: Uuid, body: F) -> T
where
    F: AsyncFnOnce(&TaskRepository<'_>, Locked<'_>) -> T,
{
    let repository = TaskRepository::new(pool);
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let outcome = body(&repository, mutation.conn()).await;
    mutation.commit().await.expect("the mutation commits");

    outcome
}

/// A titled task on this project, inserted and committed.
async fn task(pool: &PgPool, project_id: Uuid, title: &str) -> Task {
    let new = NewTask::new(project_id, title).expect("the title parses");

    in_mutation(pool, project_id, async |repository, tx| {
        repository
            .insert_task(tx, project_id, &new)
            .await
            .expect("the task inserts")
    })
    .await
}

/// A child of `parent`, inserted and committed.
async fn child(pool: &PgPool, project_id: Uuid, parent: Uuid, title: &str) -> Task {
    let mut new = NewTask::new(project_id, title).expect("the title parses");
    new.parent_id = Some(parent);

    in_mutation(pool, project_id, async |repository, tx| {
        repository
            .insert_task(tx, project_id, &new)
            .await
            .expect("the child inserts")
    })
    .await
}

/// Add one edge, without the cycle check or the recomputation a real caller
/// would compose around it.
async fn edge(
    pool: &PgPool,
    project_id: Uuid,
    task_id: Uuid,
    depends_on: Uuid,
    kind: TaskDependencyKind,
) {
    in_mutation(pool, project_id, async |repository, tx| {
        repository
            .insert_dependency(tx, project_id, task_id, depends_on, kind)
            .await
            .expect("the edge inserts");
    })
    .await;
}

/// Move a task into the project's `done` state, the default terminal one.
async fn close(pool: &PgPool, project_id: Uuid, task_id: Uuid) {
    let done = TaskRepository::new(pool)
        .find_state_by_name(project_id, "done")
        .await
        .expect("the state reads")
        .expect("the project has a done state")
        .id;

    in_mutation(pool, project_id, async |repository, tx| {
        repository
            .set_task_state_fields(
                tx,
                project_id,
                task_id,
                &StateFields {
                    state_id: Some(done),
                    ..StateFields::default()
                },
            )
            .await
            .expect("the task closes");
    })
    .await;
}

/// The stored flag, read back from the committed row.
async fn blocked(pool: &PgPool, project_id: Uuid, task_id: Uuid) -> bool {
    TaskRepository::new(pool)
        .load_task_dto(project_id, task_id)
        .await
        .expect("the task loads")
        .expect("the task is in this project")
        .blocked
}

/// Every committed event of the project, oldest first.
async fn events(pool: &PgPool, project_id: Uuid) -> Vec<TaskEvent> {
    TaskRepository::new(pool)
        .list_task_events_after(project_id, 0, 100)
        .await
        .expect("the events read")
        .into_iter()
        .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
        .collect()
}

/// Recompute these tasks in a mutation of their own and commit it.
async fn recompute(pool: &PgPool, project_id: Uuid, task_ids: &[Uuid]) -> Vec<BlockedFlip> {
    let mut mutation = TrackerMutation::begin(pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let flips = recompute_blocked(&mut mutation, task_ids)
        .await
        .expect("the recomputation runs");
    mutation.commit().await.expect("the mutation commits");

    flips
}

#[tokio::test]
async fn an_open_prerequisite_blocks_its_dependant_once_and_only_once() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project_id = seed(&pool).await;

    let prerequisite = task(&pool, project_id, "prerequisite").await;
    let dependant = task(&pool, project_id, "dependant").await;
    edge(
        &pool,
        project_id,
        dependant.id,
        prerequisite.id,
        TaskDependencyKind::Blocks,
    )
    .await;

    // A fresh task is not blocked; the edge alone does not change that, since
    // the flag is stored and only a recomputation writes it.
    assert!(!blocked(&pool, project_id, dependant.id).await);

    let flips = recompute(&pool, project_id, &[dependant.id]).await;
    assert_eq!(
        flips,
        vec![BlockedFlip {
            task_id: dependant.id,
            blocked: true,
        }]
    );
    assert!(blocked(&pool, project_id, dependant.id).await);

    let after_first = events(&pool, project_id).await;
    assert_eq!(after_first.len(), 1);
    assert_eq!(after_first[0].kind, TaskEventKind::Blocked);
    assert_eq!(after_first[0].task_id, Some(dependant.id));
    // The payload carries the task as it is *after* the write.
    assert_eq!(
        after_first[0].task.as_ref().map(|task| task.blocked),
        Some(true)
    );

    // Nothing changed, so nothing is written and nothing is announced.
    assert!(
        recompute(&pool, project_id, &[dependant.id])
            .await
            .is_empty()
    );
    assert_eq!(events(&pool, project_id).await.len(), 1);

    // The prerequisite reaching a terminal state unblocks it, with one event.
    close(&pool, project_id, prerequisite.id).await;
    let flips = recompute(&pool, project_id, &[dependant.id]).await;
    assert_eq!(
        flips,
        vec![BlockedFlip {
            task_id: dependant.id,
            blocked: false,
        }]
    );
    assert!(!blocked(&pool, project_id, dependant.id).await);

    let after_close = events(&pool, project_id).await;
    assert_eq!(after_close.len(), 2);
    assert_eq!(after_close[1].kind, TaskEventKind::Unblocked);
    assert_eq!(
        after_close[1].task.as_ref().map(|task| task.blocked),
        Some(false)
    );
}

#[tokio::test]
async fn only_blocks_edges_and_only_open_ones_count() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project_id = seed(&pool).await;

    let closed = task(&pool, project_id, "already done").await;
    let origin = task(&pool, project_id, "where it came from").await;
    let subject = task(&pool, project_id, "subject").await;

    // A `blocks` edge onto a terminal task is legal and leaves the flag false.
    close(&pool, project_id, closed.id).await;
    edge(
        &pool,
        project_id,
        subject.id,
        closed.id,
        TaskDependencyKind::Blocks,
    )
    .await;
    // The informational kinds never affect readiness, whatever their state.
    for kind in [
        TaskDependencyKind::DiscoveredFrom,
        TaskDependencyKind::Related,
    ] {
        edge(&pool, project_id, subject.id, origin.id, kind).await;
    }

    assert!(recompute(&pool, project_id, &[subject.id]).await.is_empty());
    assert!(!blocked(&pool, project_id, subject.id).await);
    assert!(events(&pool, project_id).await.is_empty());
}

#[tokio::test]
async fn a_parent_is_blocked_by_an_open_child_and_freed_by_the_last_one() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project_id = seed(&pool).await;

    let parent = task(&pool, project_id, "epic").await;
    let first = child(&pool, project_id, parent.id, "first child").await;
    let second = child(&pool, project_id, parent.id, "second child").await;

    let flips = recompute(&pool, project_id, &[parent.id]).await;
    assert_eq!(
        flips,
        vec![BlockedFlip {
            task_id: parent.id,
            blocked: true,
        }]
    );

    // One of two children closing leaves the other one holding the parent.
    close(&pool, project_id, first.id).await;
    assert!(recompute(&pool, project_id, &[parent.id]).await.is_empty());
    assert!(blocked(&pool, project_id, parent.id).await);

    close(&pool, project_id, second.id).await;
    let flips = recompute(&pool, project_id, &[parent.id]).await;
    assert_eq!(
        flips,
        vec![BlockedFlip {
            task_id: parent.id,
            blocked: false,
        }]
    );
    assert!(!blocked(&pool, project_id, parent.id).await);

    // A terminal parent with an open child is still `blocked`: the flag is a
    // function of the graph alone, and only claimability ever reads it.
    close(&pool, project_id, parent.id).await;
    let reopened = child(&pool, project_id, parent.id, "third child").await;
    assert_eq!(
        recompute(&pool, project_id, &[parent.id]).await,
        vec![BlockedFlip {
            task_id: parent.id,
            blocked: true,
        }]
    );
    assert!(blocked(&pool, project_id, parent.id).await);
    assert_eq!(reopened.parent_id, Some(parent.id));
}

#[tokio::test]
async fn a_task_that_is_both_a_dependant_and_the_parent_is_recomputed_once() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project_id = seed(&pool).await;

    let parent = task(&pool, project_id, "epic").await;
    let subject = child(&pool, project_id, parent.id, "the child").await;
    // The parent also waits on its own child, which is unusual but legal.
    edge(
        &pool,
        project_id,
        parent.id,
        subject.id,
        TaskDependencyKind::Blocks,
    )
    .await;

    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");

    assert_eq!(
        dependants_of(&mut mutation, subject.id)
            .await
            .expect("the dependants read"),
        vec![parent.id]
    );
    let affected = affected_by_state_change(&mut mutation, subject.id)
        .await
        .expect("the affected set reads");
    assert_eq!(affected, vec![parent.id], "the parent is counted once");

    let flips = recompute_blocked(&mut mutation, &affected)
        .await
        .expect("the recomputation runs");
    mutation.commit().await.expect("the mutation commits");

    assert_eq!(
        flips,
        vec![BlockedFlip {
            task_id: parent.id,
            blocked: true,
        }]
    );
    let stored = events(&pool, project_id).await;
    assert_eq!(stored.len(), 1, "one flip is one event");
}

#[tokio::test]
async fn an_empty_set_and_a_vanished_task_are_no_ops() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project_id = seed(&pool).await;

    assert!(recompute(&pool, project_id, &[]).await.is_empty());
    // An id that is not a task of this project is skipped, not an error: the
    // set to recompute is assembled from edges a cascade may have erased.
    assert!(
        recompute(&pool, project_id, &[Uuid::new_v4()])
            .await
            .is_empty()
    );
    assert!(events(&pool, project_id).await.is_empty());
}

#[tokio::test]
async fn a_blocks_edge_that_would_close_a_ring_is_refused_and_a_related_one_is_not() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project_id = seed(&pool).await;

    // A blocks B blocks C, written the way the edges point: A depends on B.
    let a = task(&pool, project_id, "a").await;
    let b = task(&pool, project_id, "b").await;
    let c = task(&pool, project_id, "c").await;
    edge(&pool, project_id, a.id, b.id, TaskDependencyKind::Blocks).await;
    edge(&pool, project_id, b.id, c.id, TaskDependencyKind::Blocks).await;

    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");

    // C depending on A closes the ring.
    let error = check_no_cycle(&mut mutation, c.id, a.id)
        .await
        .expect_err("the ring is refused");
    assert_eq!(error.status(), StatusCode::CONFLICT);
    assert_eq!(error.user_message(), "dependency would create a cycle");

    // A self-edge is a ring of length one, refused here before the table's
    // CHECK has anything to say about it.
    let error = check_no_cycle(&mut mutation, a.id, a.id)
        .await
        .expect_err("a self-edge is refused");
    assert_eq!(error.status(), StatusCode::CONFLICT);

    // The other direction is not a ring: A already depends on B, and B on C,
    // so C depending on nothing new is fine — but A depending on C is too,
    // since it only shortens an existing path.
    check_no_cycle(&mut mutation, a.id, c.id)
        .await
        .expect("a second path to the same prerequisite is not a cycle");

    mutation.no_change().await.expect("the mutation rolls back");

    // The same ring in `related` edges is nobody's problem: only `blocks`
    // edges are traversed, as start or as path.
    edge(&pool, project_id, c.id, a.id, TaskDependencyKind::Related).await;

    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .expect("the second mutation opens");
    // ... and a `related` ring does not make a later `blocks` edge a cycle.
    check_no_cycle(&mut mutation, b.id, a.id)
        .await
        .expect_err("B depending on A would still close a blocks ring");
    mutation.no_change().await.expect("the mutation rolls back");
}

#[tokio::test]
async fn reciprocal_edges_inserted_at_once_cannot_both_pass() {
    // Two mutations at once, plus the connections their helpers take.
    let (_postgres, pool) = common::db::test_pool_with(6).await;
    let project_id = seed(&pool).await;

    let a = task(&pool, project_id, "a").await;
    let b = task(&pool, project_id, "b").await;

    /// Check and insert one edge in one mutation, as a real caller composes it.
    async fn add(pool: PgPool, project_id: Uuid, task_id: Uuid, depends_on: Uuid) -> Result<()> {
        let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System).await?;
        check_no_cycle(&mut mutation, task_id, depends_on).await?;
        TaskRepository::new(&pool)
            .insert_dependency(
                mutation.conn(),
                project_id,
                task_id,
                depends_on,
                TaskDependencyKind::Blocks,
            )
            .await?;
        mutation.commit().await?;

        Ok(())
    }

    let forward = tokio::spawn(add(pool.clone(), project_id, a.id, b.id));
    let backward = tokio::spawn(add(pool.clone(), project_id, b.id, a.id));

    let results = [
        forward.await.expect("the first task does not panic"),
        backward.await.expect("the second task does not panic"),
    ];

    let refused: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    assert_eq!(
        refused.len(),
        1,
        "exactly one reciprocal edge may be accepted",
    );
    assert_eq!(refused[0].status(), StatusCode::CONFLICT);
    assert_eq!(refused[0].user_message(), "dependency would create a cycle");
}

#[tokio::test]
async fn a_deletion_can_read_every_incoming_kind_its_parent_and_its_children() {
    let (_postgres, pool) = common::db::test_pool().await;
    let project_id = seed(&pool).await;

    let parent = task(&pool, project_id, "epic").await;
    let subject = child(&pool, project_id, parent.id, "the one being deleted").await;
    let dependant = task(&pool, project_id, "waiting on it").await;
    let other = task(&pool, project_id, "merely related").await;

    // The same pair carries two kinds; each is its own edge and each is its
    // own `dependency_removed` when the task goes.
    for kind in [
        TaskDependencyKind::Blocks,
        TaskDependencyKind::DiscoveredFrom,
    ] {
        edge(&pool, project_id, dependant.id, subject.id, kind).await;
    }
    edge(
        &pool,
        project_id,
        other.id,
        subject.id,
        TaskDependencyKind::Related,
    )
    .await;
    // An outgoing edge is not a dependant and must not appear.
    edge(
        &pool,
        project_id,
        subject.id,
        other.id,
        TaskDependencyKind::Blocks,
    )
    .await;

    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");
    let capture = capture_before_delete(&mut mutation, subject.id)
        .await
        .expect("the capture reads");
    mutation.no_change().await.expect("the mutation rolls back");

    // The ids are random, so both sides are put in the same order rather than
    // the test assuming one.
    let mut dependants = capture.dependants.clone();
    dependants.sort_by_key(|(task_id, kind)| (*task_id, format!("{kind:?}")));
    let mut expected = vec![
        (dependant.id, TaskDependencyKind::Blocks),
        (dependant.id, TaskDependencyKind::DiscoveredFrom),
        (other.id, TaskDependencyKind::Related),
    ];
    expected.sort_by_key(|(task_id, kind)| (*task_id, format!("{kind:?}")));
    assert_eq!(dependants, expected);

    assert_eq!(capture.parent_id, Some(parent.id));
    assert!(capture.children.is_empty(), "a child has no children");

    // The set to recompute afterwards names each task once, parent included.
    let mut recompute_set = capture.to_recompute();
    recompute_set.sort();
    let mut expected_set = vec![dependant.id, other.id, parent.id];
    expected_set.sort();
    assert_eq!(recompute_set, expected_set);

    // The parent's own capture sees its children, and no parent of its own.
    let mut mutation = TrackerMutation::begin(&pool, project_id, TaskActor::System)
        .await
        .expect("the second mutation opens");
    let capture = capture_before_delete(&mut mutation, parent.id)
        .await
        .expect("the capture reads");
    mutation.no_change().await.expect("the mutation rolls back");

    assert!(capture.dependants.is_empty());
    assert_eq!(capture.parent_id, None);
    assert_eq!(capture.children, vec![subject.id]);
    assert_eq!(capture.to_recompute(), Vec::<Uuid>::new());
}
