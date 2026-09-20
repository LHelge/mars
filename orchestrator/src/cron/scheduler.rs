//! The generic job loop: one tick at a time, isolated from its own job.
//!
//! Everything about running a job periodically is here, so that a job body is
//! an ordinary `async fn` that returns a [`JobReport`] or an error and has to
//! know nothing about intervals, panics or logging
//! (`ARCHITECTURE.md`, "Background jobs": "Every job logs its outcome and
//! never panics the process; a failing job is retried at its next interval").
//!
//! The isolation is the point. A tick runs its job on its *own* task, so a
//! panic arrives here as a [`JoinError`] and is logged instead of unwinding
//! the loop, and neither an error nor a panic affects the next tick or any
//! other job's loop. Nothing is converted: the scheduler only logs.

use std::future::Future;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior, interval};

use super::JobReport;
use crate::prelude::*;

/// Run `job` every `period` until `shutdown` carries `true`.
///
/// The first tick fires immediately, so every job runs once at startup and an
/// operator sees the sweep's outcome without waiting out an interval. Ticks
/// never overlap: the loop awaits its job before asking for the next tick, and
/// `MissedTickBehavior::Delay` then schedules that tick a full period after
/// the late one fired rather than catching up on the ones that passed while
/// the job ran.
///
/// Shutdown is checked between ticks only, so a job that is running when the
/// signal arrives finishes first — jobs are short and transactional, and the
/// caller bounds the wait with `STOP_GRACE_SECS`. A `shutdown` sender dropped
/// without ever sending is shutdown too: nobody is left who could ask for
/// another tick.
pub fn spawn_job<F, Fut>(
    name: &'static str,
    period: Duration,
    mut shutdown: watch::Receiver<bool>,
    job: F,
) -> JoinHandle<()>
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<JobReport>> + Send + 'static,
{
    tokio::spawn(async move {
        let mut ticker = interval(period);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            // Also the post-tick check: an in-flight job that finished after
            // the signal arrived stops the loop here rather than running
            // again.
            if *shutdown.borrow_and_update() {
                break;
            }

            tokio::select! {
                changed = shutdown.changed() => {
                    // Every sender is gone: nothing can ask for another tick,
                    // so this is shutdown as much as a `true` is. Otherwise
                    // re-read the value at the top of the loop.
                    if changed.is_err() {
                        break;
                    }
                    continue;
                }
                _ = ticker.tick() => {}
            }

            let started = Instant::now();

            // The job on its own task, so a panic in it is a `JoinError` here
            // instead of the end of this loop and of every later tick.
            match tokio::spawn(job()).await {
                Ok(Ok(report)) => {
                    let elapsed_ms = started.elapsed().as_millis() as u64;
                    // A sweep that found nothing is the common case and says
                    // nothing an operator needs at `info`; anything that acted
                    // or failed does.
                    if report.items + report.failures > 0 {
                        info!(
                            job = name,
                            elapsed_ms,
                            items = report.items,
                            skipped = report.skipped,
                            failures = report.failures,
                            "job done"
                        );
                    } else {
                        debug!(
                            job = name,
                            elapsed_ms,
                            items = report.items,
                            skipped = report.skipped,
                            failures = report.failures,
                            "job done"
                        );
                    }
                }
                // The job failed as a whole. It is retried at its next tick,
                // which is the whole recovery story for a periodic job.
                Ok(Err(err)) => error!(job = name, error = %err, "job failed"),
                Err(err) if err.is_panic() => error!(job = name, "job panicked"),
                // Cancelled: only this loop's own task going away can do that,
                // so there is nothing left to tick for.
                Err(_) => break,
            }
        }

        debug!(job = name, "job loop stopped");
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::mpsc;

    use super::*;

    /// Short enough that a paused clock crosses several of them instantly, and
    /// long enough to be a period rather than a rounding error.
    const PERIOD: Duration = Duration::from_millis(10);

    #[tokio::test(start_paused = true)]
    async fn a_panic_and_an_error_leave_the_loop_and_the_other_jobs_running() {
        // The first run really panics, so the process-wide hook prints its
        // message to this test's output. That line is the scenario working,
        // not a failure; the hook is deliberately left alone, because it is
        // process-global and the other tests in this binary run beside this
        // one.
        let flaky_runs = Arc::new(AtomicUsize::new(0));
        let steady_runs = Arc::new(AtomicUsize::new(0));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let flaky = spawn_job("flaky", PERIOD, shutdown_rx.clone(), {
            let runs = Arc::clone(&flaky_runs);
            move || {
                let run = runs.fetch_add(1, Ordering::SeqCst);
                async move {
                    match run {
                        0 => panic!("the first run panics"),
                        1 => Err(Error::Internal("the second run fails".to_string())),
                        _ => Ok(JobReport {
                            items: 1,
                            ..JobReport::default()
                        }),
                    }
                }
            }
        });

        let steady = spawn_job("steady", PERIOD, shutdown_rx, {
            let runs = Arc::clone(&steady_runs);
            move || {
                let runs = Arc::clone(&runs);
                async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    Ok(JobReport::default())
                }
            }
        });

        // Five periods on a paused clock: the runtime advances it as soon as
        // both loops are waiting on their tickers.
        tokio::time::sleep(PERIOD * 5).await;

        let flaky_before_stop = flaky_runs.load(Ordering::SeqCst);
        assert!(
            flaky_before_stop >= 3,
            "the panic and the error did not stop the loop: {flaky_before_stop} runs",
        );
        let steady_before_stop = steady_runs.load(Ordering::SeqCst);
        assert!(
            steady_before_stop >= 3,
            "the neighbouring job stopped ticking: {steady_before_stop} runs",
        );

        shutdown_tx.send(true).expect("both loops are listening");

        for (name, handle) in [("flaky", flaky), ("steady", steady)] {
            tokio::time::timeout(PERIOD, handle)
                .await
                .unwrap_or_else(|_| panic!("the {name} loop stops within one period"))
                .unwrap_or_else(|_| panic!("the {name} loop does not panic"));
        }

        // And having stopped, neither runs again.
        let flaky_after_stop = flaky_runs.load(Ordering::SeqCst);
        let steady_after_stop = steady_runs.load(Ordering::SeqCst);
        tokio::time::sleep(PERIOD * 5).await;
        assert_eq!(flaky_runs.load(Ordering::SeqCst), flaky_after_stop);
        assert_eq!(steady_runs.load(Ordering::SeqCst), steady_after_stop);
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_tick_fires_without_waiting_a_period() {
        // An hour, so anything but an immediate first tick is unmistakable.
        let period = Duration::from_secs(3600);
        let started = Instant::now();

        let (ran_tx, mut ran_rx) = mpsc::unbounded_channel();
        // Bound to a name rather than `_`, so the sender outlives the loop and
        // a dropped sender is not mistaken for shutdown.
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = spawn_job("startup", period, shutdown_rx, move || {
            let ran_tx = ran_tx.clone();
            async move {
                let _ = ran_tx.send(Instant::now());
                Ok(JobReport::default())
            }
        });

        let first = ran_rx.recv().await.expect("the first tick runs");
        assert!(
            first.duration_since(started) < period,
            "the first tick waited a full period",
        );

        handle.abort();
    }
}
