//! The one per-project asynchronous git lock (`ARCHITECTURE.md`, "Git model",
//! Serialization; ADR 0017).
//!
//! Every orchestrator operation that mutates a project repository — its
//! initialization, upstream fetch, session fetch-back, hand-off publication,
//! merge, rebase, push, hand-off ref removal and the project's own deletion —
//! runs under [`ProjectGitLocks::lock`] for that project. The table lives in
//! [`crate::prelude::AppState`], so the REST handlers, the MCP tools, the
//! session launcher, the cron jobs and the deletion paths all wait on the same
//! lock. It does not serialize the commands an agent runs inside its own
//! checkout: those touch the session's clone, not the project repository.
//!
//! The lock is advisory within one process. v1 runs a single orchestrator
//! instance (`SPEC.md`, "Non-goals for v1"), which is what makes an in-memory
//! table sufficient; a second instance would need a lock in Postgres instead.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::OwnedMutexGuard;
use tracing::{Instrument, debug_span};
use uuid::Uuid;

use crate::prelude::*;

/// How long a caller waits before it says so at `debug`.
///
/// A git lock is normally held for the length of a fetch or a merge. Waiting
/// longer than this is not an error — a first clone of a large repository
/// takes minutes — but it is the thing to look at when an operation seems
/// stuck, so it is logged once per waiter and the wait then continues.
const SLOW_WAIT: Duration = Duration::from_secs(5);

/// The per-project git locks, one entry per project that has been touched
/// since start-up.
///
/// The map is behind a `std::sync::Mutex` and only ever held long enough to
/// clone one `Arc`; the awaiting happens on the per-project
/// `tokio::sync::Mutex` after the map lock has been released, so no
/// synchronous lock is ever held across an `.await`.
///
/// # Ordering
///
/// Git locks come first. An operation that changes both the repository and
/// tracker data acquires the git lock, prepares the refs, and only then opens
/// the database transaction that locks the project row; the tracker
/// transaction revalidates what git preparation assumed (ADR 0021;
/// `ARCHITECTURE.md`, "Task tracker", "One mutation at a time per project").
/// Never the reverse: a transaction holding a project row with `SELECT ... FOR
/// UPDATE` must not wait for a git lock, because the holder of that git lock
/// may be about to want the same row. An operation spanning several projects
/// takes their git locks in ascending [`Uuid`] order — that is what
/// [`ProjectGitLocks::lock_many`] is for — and then their project rows in the
/// same order.
#[derive(Debug, Default)]
pub struct ProjectGitLocks {
    /// One `tokio::sync::Mutex` per project, created on first use.
    locks: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
}

impl ProjectGitLocks {
    /// An empty table; entries appear as projects are used.
    pub fn new() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
        }
    }

    /// Acquire the lock for one project, waiting for the current holder.
    ///
    /// The returned guard is owned and `Send`, so it can be held across
    /// `.await` points and moved into a spawned task: a composite operation
    /// acquires it once and holds it through completion.
    pub async fn lock(&self, project_id: Uuid) -> ProjectGitGuard {
        let mutex = self.entry(project_id);
        let span = debug_span!("git_lock", project_id = %project_id);

        async move {
            let acquire = mutex.lock_owned();
            tokio::pin!(acquire);

            // `select!` rather than a `timeout`, which would cancel the
            // acquisition and send this caller to the back of the mutex's
            // queue each time it reported the wait.
            let mut reported = false;
            let guard = loop {
                tokio::select! {
                    guard = &mut acquire => break guard,
                    () = tokio::time::sleep(SLOW_WAIT), if !reported => {
                        reported = true;
                        debug!(
                            project_id = %project_id,
                            waited_secs = SLOW_WAIT.as_secs(),
                            "still waiting for the project git lock"
                        );
                    }
                }
            };

            ProjectGitGuard {
                project_id,
                _guard: guard,
            }
        }
        .instrument(span)
        .await
    }

    /// Acquire the locks for several projects at once, in ascending [`Uuid`]
    /// order.
    ///
    /// Duplicate ids are collapsed — asking twice for the same project would
    /// otherwise deadlock on the second acquisition — so the returned vector
    /// holds one guard per distinct id, sorted, whatever order the caller
    /// passed. Acquiring in a fixed order is what keeps two multi-project
    /// operations from deadlocking against each other; the database locks that
    /// follow are taken in the same order (ADR 0021).
    pub async fn lock_many(&self, ids: &[Uuid]) -> Vec<ProjectGitGuard> {
        let mut ordered: Vec<Uuid> = ids.to_vec();
        ordered.sort_unstable();
        ordered.dedup();

        let mut guards = Vec::with_capacity(ordered.len());
        for project_id in ordered {
            guards.push(self.lock(project_id).await);
        }
        guards
    }

    /// Drop the entry for a deleted project.
    ///
    /// Called after the project's repository and row are gone. A caller that
    /// arrives later simply creates a fresh entry and takes an uncontended
    /// lock; it then fails on I/O, because the repository directory no longer
    /// exists, which is the correct answer for an operation on a deleted
    /// project. Forgetting an entry while it is held is harmless: the holder
    /// keeps its `Arc` and its guard, and only new callers get the new entry.
    pub fn forget(&self, project_id: Uuid) {
        self.entries().remove(&project_id);
    }

    /// How many projects have an entry. Test and diagnostic use only.
    pub fn tracked_projects(&self) -> usize {
        self.entries().len()
    }

    /// The `Arc` for one project, inserted if this is its first use.
    ///
    /// The map lock is released when this returns, before the caller awaits
    /// the per-project mutex.
    fn entry(&self, project_id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(self.entries().entry(project_id).or_default())
    }

    /// The map, recovering from a poisoned lock rather than propagating a
    /// panic. Nothing here can panic while the map lock is held — the critical
    /// section is a hash lookup and an `Arc` clone — so a poisoned map means
    /// some unrelated failure, and refusing every git operation because of it
    /// would be worse than carrying on.
    fn entries(&self) -> MutexGuard<'_, HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>> {
        self.locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Proof that the caller holds one project's git lock.
///
/// Hold it through completion of a composite operation; helpers take
/// `&ProjectGitGuard` and never reacquire. Passing the guard rather than the
/// project id makes "acquire once, hold through completion"
/// (`ARCHITECTURE.md`, "Git model", Serialization) visible at every call site:
/// a helper that wanted the lock for itself could not be called without one,
/// and could not take one without the borrow checker showing where.
///
/// Dropping it releases the lock, including when the holding task is
/// cancelled. Cancellation therefore never leaks the lock, but it can leave a
/// half-finished operation behind: callers must leave the project repository
/// consistent on every path, and the temporary clones that are left over are
/// removed by the orphan-cleanup job.
///
/// # Ordering
///
/// See [`ProjectGitLocks`]: never acquire one of these while a database
/// transaction holding the project row is open.
#[derive(Debug)]
pub struct ProjectGitGuard {
    /// The project this guard covers.
    project_id: Uuid,
    /// The lock itself; dropping the guard releases it.
    _guard: OwnedMutexGuard<()>,
}

impl ProjectGitGuard {
    /// The project whose repository this guard covers.
    ///
    /// Helpers that take the guard as proof of the lock read the id from it
    /// rather than from a second parameter, so the id and the lock cannot
    /// disagree.
    pub fn project_id(&self) -> Uuid {
        self.project_id
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::time::timeout;

    use super::*;

    /// Long enough that a genuinely serialized second acquisition cannot beat
    /// it, short enough that a broken lock fails the test quickly.
    const SHORT: Duration = Duration::from_millis(200);

    fn assert_send<T: Send>() {}

    #[tokio::test(flavor = "multi_thread")]
    async fn the_guard_is_send_and_names_its_project() {
        assert_send::<ProjectGitGuard>();

        let locks = ProjectGitLocks::new();
        let project = Uuid::new_v4();

        let guard = locks.lock(project).await;

        assert_eq!(guard.project_id(), project);
        assert_eq!(locks.tracked_projects(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn two_locks_on_one_project_run_one_after_the_other() {
        let locks = Arc::new(ProjectGitLocks::new());
        let project = Uuid::new_v4();
        let order = Arc::new(AtomicUsize::new(0));

        let first = locks.lock(project).await;

        let second = tokio::spawn({
            let locks = Arc::clone(&locks);
            let order = Arc::clone(&order);
            async move {
                let guard = locks.lock(project).await;
                // The position this acquisition took in the sequence.
                (guard, order.fetch_add(1, Ordering::SeqCst))
            }
        });

        // While the first guard lives, the second task cannot get in.
        tokio::time::sleep(SHORT).await;
        assert_eq!(
            order.load(Ordering::SeqCst),
            0,
            "the second lock must not be granted while the first guard is held"
        );

        // The first operation finishes its work and only then releases.
        let first_position = order.fetch_add(1, Ordering::SeqCst);
        drop(first);

        let (second_guard, second_position) = timeout(SHORT, second)
            .await
            .expect("the second lock is granted once the first guard drops")
            .expect("the spawned task does not panic");

        assert_eq!(first_position, 0);
        assert_eq!(second_position, 1);
        assert_eq!(second_guard.project_id(), project);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn locks_on_two_projects_are_independent() {
        let locks = Arc::new(ProjectGitLocks::new());
        let one = Uuid::new_v4();
        let two = Uuid::new_v4();

        let held = locks.lock(one).await;

        let other = tokio::spawn({
            let locks = Arc::clone(&locks);
            async move { locks.lock(two).await }
        });

        let guard = timeout(SHORT, other)
            .await
            .expect("another project's lock is not blocked by this one")
            .expect("the spawned task does not panic");

        assert_eq!(guard.project_id(), two);
        assert_eq!(held.project_id(), one);
        assert_eq!(locks.tracked_projects(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lock_many_returns_guards_in_uuid_order_and_deduplicates() {
        let locks = ProjectGitLocks::new();

        let mut ids: Vec<Uuid> = (0..4).map(|_| Uuid::new_v4()).collect();
        let mut expected = ids.clone();
        expected.sort_unstable();

        // Whatever order the caller passes, including a repeat.
        ids.reverse();
        ids.push(expected[0]);

        let guards = timeout(SHORT, locks.lock_many(&ids))
            .await
            .expect("a repeated id must not deadlock");

        let acquired: Vec<Uuid> = guards.iter().map(ProjectGitGuard::project_id).collect();
        assert_eq!(acquired, expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn forget_drops_the_entry_and_a_late_caller_gets_a_fresh_one() {
        let locks = ProjectGitLocks::new();
        let project = Uuid::new_v4();

        drop(locks.lock(project).await);
        assert_eq!(locks.tracked_projects(), 1);

        locks.forget(project);
        assert_eq!(locks.tracked_projects(), 0);

        let late = timeout(SHORT, locks.lock(project))
            .await
            .expect("a caller after deletion takes an uncontended fresh lock");
        assert_eq!(late.project_id(), project);
        assert_eq!(locks.tracked_projects(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_cancelled_holder_releases_the_lock() {
        let locks = Arc::new(ProjectGitLocks::new());
        let project = Uuid::new_v4();

        let holder = tokio::spawn({
            let locks = Arc::clone(&locks);
            async move {
                let _guard = locks.lock(project).await;
                // Never completes: the task is aborted while holding the lock.
                std::future::pending::<()>().await;
            }
        });

        // Make sure the holder really has the lock before aborting it.
        while locks.entry(project).try_lock().is_ok() {
            tokio::task::yield_now().await;
        }

        holder.abort();

        let guard = timeout(SHORT, locks.lock(project))
            .await
            .expect("aborting the holder releases the lock");
        assert_eq!(guard.project_id(), project);
    }
}
