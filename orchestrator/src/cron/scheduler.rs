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
//!
//! A job may also be *woken* between ticks ([`spawn_woken_job`]): the wake-up
//! is one more arm of the same `select!` and runs the same body on the same
//! loop, which is the whole reason a woken job still never overlaps itself or
//! its own timer.

use std::future::Future;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
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
    shutdown: watch::Receiver<bool>,
    job: F,
) -> JoinHandle<()>
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<JobReport>> + Send + 'static,
{
    spawn_woken_job(name, period, None, shutdown, job)
}

/// The sending half of a job's wake-up: "run before your next tick".
///
/// Cheap to clone and never blocks. [`JobWaker::wake`] is a `try_send` on a
/// channel of capacity one, so a burst of twenty wake-ups is one pending
/// wake-up and a waker whose loop has stopped is not an error anybody has to
/// handle — the timer is behind every wake-up path there is.
#[derive(Clone)]
pub struct JobWaker {
    tx: mpsc::Sender<()>,
}

impl JobWaker {
    /// Ask the job to run soon. Never blocks and never fails.
    pub fn wake(&self) {
        // Full: a wake-up is already pending and this one is that one.
        // Closed: the loop is gone, which shutdown is allowed to do first.
        let _ = self.tx.try_send(());
    }
}

/// The receiving half [`spawn_woken_job`] selects on, with its debounce.
pub struct JobWake {
    rx: mpsc::Receiver<()>,
    /// How long the loop waits after a wake-up before running, so that the
    /// rest of a burst arrives first and costs no second run.
    debounce: Duration,
    /// A sender of our own, so the channel never closes while the loop holds
    /// it: a closed channel would make `recv` return at once, forever.
    _keepalive: mpsc::Sender<()>,
}

/// A waker and the wake it drives, debounced by `debounce`.
pub fn job_wake(debounce: Duration) -> (JobWaker, JobWake) {
    // One: a pending wake-up is a boolean, not a queue. What matters is that
    // one more run happens, not how many signals asked for it.
    let (tx, rx) = mpsc::channel(1);

    (
        JobWaker { tx: tx.clone() },
        JobWake {
            rx,
            debounce,
            _keepalive: tx,
        },
    )
}

/// [`spawn_job`], plus a wake-up that runs the job before its next tick.
///
/// The wake-up is an arm of the loop's own `select!` and not a second spawner,
/// which is what keeps the guarantee [`spawn_job`] documents: **one run at a
/// time**, whether the run was asked for by the timer or by a waker, because
/// there is one loop and it awaits its job before selecting again.
///
/// Wake-ups are coalesced twice over. A wake-up is a single pending signal
/// ([`JobWaker::wake`]), so a burst of twenty is one; and a wake-up that wins
/// the `select!` waits out `debounce` and then takes everything that arrived
/// meanwhile, so a burst spread over a few milliseconds is still one run.
/// Signals that arrive while the job is running are neither of those: they
/// stay pending and cause exactly one more run afterwards, which is the point
/// — whatever they were about was not necessarily seen by the run that was
/// already in flight.
pub fn spawn_woken_job<F, Fut>(
    name: &'static str,
    period: Duration,
    wake: Option<JobWake>,
    mut shutdown: watch::Receiver<bool>,
    job: F,
) -> JoinHandle<()>
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<JobReport>> + Send + 'static,
{
    tokio::spawn(async move {
        let mut wake = wake;
        let mut ticker = interval(period);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            // Also the post-tick check: an in-flight job that finished after
            // the signal arrived stops the loop here rather than running
            // again.
            if *shutdown.borrow_and_update() {
                break;
            }

            let woken = tokio::select! {
                changed = shutdown.changed() => {
                    // Every sender is gone: nothing can ask for another tick,
                    // so this is shutdown as much as a `true` is. Otherwise
                    // re-read the value at the top of the loop.
                    if changed.is_err() {
                        break;
                    }
                    continue;
                }
                _ = ticker.tick() => false,
                () = wait_for_wake(wake.as_mut()) => true,
            };

            if let Some(wake) = wake.as_mut()
                && woken
            {
                // Let the rest of the burst arrive, then take all of it: the
                // twenty tasks a planner files are one run, not twenty.
                tokio::time::sleep(wake.debounce).await;
                while wake.rx.try_recv().is_ok() {}
                debug!(job = name, "job woken");
                // A woken run is the same full run the timer would have made,
                // so the fallback starts counting again from here rather than
                // ticking a moment later for nothing.
                ticker.reset();

                // The debounce is a window a shutdown can arrive in, and a
                // loop that is stopping does not owe anybody a last run.
                if *shutdown.borrow_and_update() {
                    break;
                }
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

/// Wait for a wake-up, or forever when the job has no waker.
///
/// Cancel-safe in the `select!` above, because `mpsc::Receiver::recv` is: a
/// signal is taken out of the channel only when this future completes, so a
/// tick that wins the race loses nothing.
///
/// The `None` arm is [`std::future::pending`] rather than a second `select!`
/// without the arm, so a timer-only job and a woken one stay one loop.
async fn wait_for_wake(wake: Option<&mut JobWake>) {
    match wake {
        // A closed channel is unreachable: `JobWake` holds a sender of its
        // own, so it outlives every waker. Treated as "no wake-up, ever"
        // rather than asserted on, because the alternative is a hot loop.
        Some(wake) => match wake.rx.recv().await {
            Some(()) => {}
            None => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
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

    /// Debounce short enough to be crossed instantly on a paused clock.
    const DEBOUNCE: Duration = Duration::from_millis(5);

    /// What a [`counting_job`] recorded.
    ///
    /// Counters rather than assertions inside the job: the scheduler catches a
    /// panic in a job and logs it, so an assertion there would be swallowed
    /// instead of failing the test.
    #[derive(Default)]
    struct Runs {
        /// Runs that finished.
        finished: AtomicUsize,
        /// Runs in flight right now.
        live: AtomicUsize,
        /// Times a run started while another was in flight.
        overlaps: AtomicUsize,
    }

    impl Runs {
        fn finished(&self) -> usize {
            self.finished.load(Ordering::SeqCst)
        }

        fn overlaps(&self) -> usize {
            self.overlaps.load(Ordering::SeqCst)
        }
    }

    /// A job that records its runs into `runs` and takes `duration` each.
    fn counting_job(
        runs: Arc<Runs>,
        duration: Duration,
    ) -> impl Fn() -> std::pin::Pin<Box<dyn Future<Output = Result<JobReport>> + Send>>
    + Send
    + Sync
    + 'static {
        move || {
            let runs = Arc::clone(&runs);
            Box::pin(async move {
                if runs.live.fetch_add(1, Ordering::SeqCst) > 0 {
                    runs.overlaps.fetch_add(1, Ordering::SeqCst);
                }
                tokio::time::sleep(duration).await;
                runs.finished.fetch_add(1, Ordering::SeqCst);
                runs.live.fetch_sub(1, Ordering::SeqCst);
                Ok(JobReport::default())
            })
        }
    }

    /// Wait until `runs` has finished at least `count`, or fail the test.
    ///
    /// The wait is a `sleep` rather than a `yield_now` because the clock is
    /// paused: a task that is ready to run stops the runtime advancing time,
    /// so a spin would never reach its own deadline.
    async fn wait_for_runs(runs: &Runs, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while runs.finished() < count {
            assert!(
                Instant::now() < deadline,
                "only {} of {count} runs happened",
                runs.finished(),
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_wake_ups_is_one_run_and_a_wake_up_during_a_run_is_one_more() {
        // An hour: nothing below can be explained by the timer, whose first
        // tick is the one run every assertion starts from.
        let period = Duration::from_secs(3600);
        let runs = Arc::new(Runs::default());
        let (waker, wake) = job_wake(DEBOUNCE);
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        // Each run takes a while, so a wake-up can arrive during one.
        let job_duration = DEBOUNCE * 10;
        let handle = spawn_woken_job(
            "woken",
            period,
            Some(wake),
            shutdown_rx,
            counting_job(Arc::clone(&runs), job_duration),
        );

        // The startup tick.
        wait_for_runs(&runs, 1).await;

        // A planner filing twenty tasks: twenty wake-ups, one run.
        for _ in 0..20 {
            waker.wake();
        }
        wait_for_runs(&runs, 2).await;
        tokio::time::sleep(job_duration * 3).await;
        assert_eq!(
            runs.finished(),
            2,
            "a burst of wake-ups caused more than one run",
        );

        // A wake-up during a run: the run in flight may not have seen what it
        // was about, so exactly one more run follows — and then quiet.
        waker.wake();
        // Long enough to be inside the run the wake-up above started.
        tokio::time::sleep(DEBOUNCE * 2).await;
        waker.wake();
        wait_for_runs(&runs, 4).await;
        tokio::time::sleep(job_duration * 4).await;
        assert_eq!(
            runs.finished(),
            4,
            "a wake-up arriving during a run did not cause exactly one more",
        );
        assert_eq!(runs.overlaps(), 0, "two runs of the job overlapped");

        handle.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn the_timer_and_the_waker_never_run_the_job_at_once() {
        // A period far shorter than a run, so the timer is always due while
        // the job is running and every tick races the wake-ups below. The
        // job's own assertion is what fails if two ever overlap.
        let runs = Arc::new(Runs::default());
        let (waker, wake) = job_wake(DEBOUNCE);
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = spawn_woken_job(
            "contended",
            DEBOUNCE,
            Some(wake),
            shutdown_rx,
            counting_job(Arc::clone(&runs), DEBOUNCE * 4),
        );

        for _ in 0..50 {
            waker.wake();
            tokio::time::sleep(DEBOUNCE).await;
        }

        assert!(
            runs.finished() > 1,
            "the contended loop stopped running its job",
        );
        assert_eq!(
            runs.overlaps(),
            0,
            "the timer and the waker ran the job at once",
        );
        handle.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_stops_a_woken_loop_even_with_a_wake_up_pending() {
        let runs = Arc::new(Runs::default());
        let (waker, wake) = job_wake(DEBOUNCE);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = spawn_woken_job(
            "stopping",
            Duration::from_secs(3600),
            Some(wake),
            shutdown_rx,
            counting_job(Arc::clone(&runs), Duration::ZERO),
        );

        wait_for_runs(&runs, 1).await;

        // The wake-up and the shutdown arrive together; the shutdown wins,
        // because a loop that is stopping owes nobody a last run.
        waker.wake();
        shutdown_tx.send(true).expect("the loop is listening");

        tokio::time::timeout(DEBOUNCE * 10, handle)
            .await
            .expect("the woken loop stops within a few debounces")
            .expect("the loop does not panic");
        assert_eq!(runs.finished(), 1);
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
