//! The bounded retry every tracker write is run under.
//!
//! `ARCHITECTURE.md`, "Task tracker" → "Lock order" fixes one order for the
//! row locks a mutation takes, and every path in this crate follows it. Two
//! things keep that from being the whole answer:
//!
//! - the cascades of a row that is *not* a tracker row can still reach tracker
//!   rows from outside the project lock — deleting a user sets
//!   `tasks.assignee_user_id` to NULL across every project that user was
//!   assigned in, and no single project lock orders that;
//! - a lock order is a property of the code, and a new path can lose it.
//!
//! Postgres answers such a cycle by aborting one of the transactions whole
//! with SQLSTATE `40P01`, and a rolled-back tracker mutation has written no
//! row, no event and no notification (`TrackerMutation`). Running it again is
//! therefore not a partial retry of anything: it is the operation, attempted
//! once more against the state the other transaction left. That is what
//! [`retry_on_serialization_failure`] does, and it is why the contract can say
//! that a deadlock is never what a caller is answered with
//! (`ARCHITECTURE.md`, "Task tracker" → "Lock order").
//!
//! It is deliberately a wrapper around the *whole* operation rather than a
//! loop inside `TrackerMutation`: the deadlock victim is chosen after the
//! transaction has read what it validates against, so an attempt has to start
//! at the project lock again for its rules to be decided against rows nobody
//! else is holding.

use std::time::Duration;

use crate::prelude::*;

/// How many times an operation is attempted in total.
///
/// Three, because a deadlock needs two transactions that reached each other's
/// rows in opposite order and the loser restarts behind the winner: the second
/// attempt runs against a database where the winner has committed. A third is
/// the allowance for a third writer having joined in between. Beyond that the
/// cycle is not a race but a lock order that is wrong, and answering 500 is
/// the honest report — with `error!` naming the operation, so it is findable.
const MAX_ATTEMPTS: u32 = 3;

/// How long the loser waits before trying again.
///
/// Long enough for the winner to commit and release its locks, short enough
/// not to show as latency; multiplied by the attempt number, so a repeated
/// collision spreads out instead of re-colliding at the same instant.
const BACKOFF: Duration = Duration::from_millis(20);

/// Run `operation` until it succeeds, fails for its own reasons, or has
/// deadlocked [`MAX_ATTEMPTS`] times.
///
/// `name` is what the log line calls the operation — the endpoint or the tool,
/// as `update_task` or `publish_handoff`, never a formatted id.
///
/// **What may be wrapped.** An operation whose only durable effects are inside
/// the tracker transaction, so that a rolled-back attempt leaves nothing for
/// the next one to trip over. Every tracker mutation qualifies by
/// construction: `TrackerMutation` holds its events, its `task_sessions` links
/// and its escalation emails until the commit. An operation that also writes
/// outside Postgres — the hand-off publication's pinned ref — qualifies only
/// because it cleans that up on every way out but success, which is what
/// `handoffs::HandoffService::update_with_handoff` documents.
///
/// A retried attempt is logged at `warn!`: a deadlock that happens often
/// enough to see in a log is a lock order to fix, not a cost to pay.
///
/// The closure is `FnMut() -> impl Future` rather than an `AsyncFnMut`
/// deliberately: an async closure's future is generic over the borrow of each
/// call, and the compiler cannot then prove it `Send` for *every* lifetime,
/// which is what an axum handler and a spawned job both need. A plain closure
/// returning an `async` block borrows the enclosing scope with one concrete
/// lifetime instead, so each attempt's future is an ordinary `Send` one. Call
/// sites are written `|| async { ... }`.
pub async fn retry_on_serialization_failure<F, Fut, T>(
    name: &'static str,
    mut operation: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    for attempt in 1..=MAX_ATTEMPTS {
        let result = operation().await;

        let Err(err) = result else {
            return result;
        };

        if !err.is_serialization_failure() {
            return Err(err);
        }

        if attempt == MAX_ATTEMPTS {
            error!(
                operation = name,
                attempts = attempt,
                error = %err,
                "a tracker operation deadlocked on every attempt",
            );
            return Err(err);
        }

        warn!(
            operation = name,
            attempt, "a tracker operation was chosen as a deadlock victim; retrying",
        );

        tokio::time::sleep(BACKOFF * attempt).await;
    }

    // Unreachable: the loop returns on the last attempt either way. Written as
    // an internal error rather than an `unreachable!`, because a panic in a
    // request handler is never the better answer (`CLAUDE.md`, "Backend
    // conventions").
    error!(operation = name, "the retry loop fell through");
    Err(Error::Internal("retry loop fell through".into()))
}
