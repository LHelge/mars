//! The cron service's shape: dispatch, names and intervals
//! (`ARCHITECTURE.md`, "Background jobs").
//!
//! What a job *does* is asserted by the suite of the task that writes it. What
//! is asserted here is the frame those bodies hang in and that every one of
//! them relies on: that `run_once` reaches each job, that the name an operator
//! greps for is the documented one, and that the interval each job runs at is
//! the documented one — including the mirror fetch's, which comes from the
//! configuration and not from a constant.
//!
//! The scheduler itself is unit-tested on a paused clock in
//! `src/cron/scheduler.rs`; nothing here starts a loop, because `TestApp`
//! deliberately never calls `CronService::start`.
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use chrono::Utc;
use common::TestApp;
use mars_orchestrator::cron::{JobName, JobReport};

#[tokio::test]
async fn run_once_dispatches_every_job() {
    let app = TestApp::spawn().await;
    let now = Utc::now();

    for job in JobName::ALL {
        let report = app
            .cron()
            .run_once(job, now)
            .await
            .unwrap_or_else(|err| panic!("{job} runs: {err}"));

        // A fresh app has no projects, no sessions and no expired rows, so
        // every job finds nothing: an empty report is the whole contract
        // here — the dispatch arrived and returned `Ok`.
        assert_eq!(report, JobReport::default(), "{job} reported work");
    }
}

#[tokio::test]
async fn every_job_has_the_documented_name_and_interval() {
    let app = TestApp::spawn().await;
    let config = &app.state.config;

    // `MIRROR_FETCH_INTERVAL_SECS` is 900 in the test configuration, not the
    // documented default of 600, so this fails if the period ever becomes a
    // constant (`tests/common/app.rs`).
    let mirror_fetch = Duration::from_secs(config.mirror_fetch_interval_secs);
    assert_ne!(
        mirror_fetch,
        Duration::from_secs(600),
        "the test configuration no longer overrides MIRROR_FETCH_INTERVAL_SECS",
    );

    // And the same for the dispatcher's, which is the other configurable one
    // (`DISPATCHER_INTERVAL_SECS`, documented default 60).
    let dispatcher = Duration::from_secs(config.dispatcher_interval_secs);
    assert_ne!(
        dispatcher,
        Duration::from_secs(60),
        "the test configuration no longer overrides DISPATCHER_INTERVAL_SECS",
    );

    let expected = [
        (JobName::MirrorFetch, "mirror_fetch", mirror_fetch),
        (JobName::IdleReaper, "idle_reaper", Duration::from_secs(60)),
        (
            JobName::StuckTaskReaper,
            "stuck_task_reaper",
            Duration::from_secs(60),
        ),
        (JobName::Dispatcher, "dispatcher", dispatcher),
        // Not configurable: one minute is the resolution of the cron
        // expressions this job fires, not a knob (ADR 0043).
        (JobName::Scheduler, "scheduler", Duration::from_secs(60)),
        // Not configurable: the timer is the fallback behind its wake-up.
        (JobName::AutoMerge, "auto_merge", Duration::from_secs(60)),
        (
            JobName::TokenCleanup,
            "token_cleanup",
            Duration::from_secs(3600),
        ),
        (
            JobName::SecretRotation,
            "secret_rotation",
            Duration::from_secs(3600),
        ),
        (
            JobName::OrphanCleanup,
            "orphan_cleanup",
            Duration::from_secs(3600),
        ),
    ];

    assert_eq!(
        expected.len(),
        JobName::ALL.len(),
        "a job was added without a documented name and interval",
    );

    for (job, name, period) in expected {
        assert_eq!(job.to_string(), name);
        assert_eq!(job.as_str(), name);
        assert_eq!(
            job.period(config),
            period,
            "{name} runs at the wrong period"
        );
    }
}
