//! The scheduled-agent job against a real database and the mock engine
//! (`ARCHITECTURE.md`, "Task tracker" → "Scheduled agents" and "Background
//! jobs"; ADR 0043).
//!
//! The job is driven directly, one run at a time, with two instants the
//! scenario chose: the `now` the run is given and the process-start floor the
//! service was built with (`CronService::with_started_at`). Both have to be
//! placeable, because every rule here is about which occurrences fall inside a
//! window — and none of them is about a timer, which belongs to
//! `cron::scheduler` and is tested there.
//!
//! What the job composes has its own suite already: the launch path and its
//! actor are `tests/session_create.rs`'s, the four bounds are
//! `tests/session_capacity.rs`'s, the expression and the window are the unit
//! tests of `src/models/schedule.rs`, and the save-time rules of a schedule
//! are `tests/profiles.rs`'s. What is asserted here is only what the job
//! itself decides: *whether* a tick is due, that it fires once, and what it
//! does when the answer is no.
//!
//! The arrangement is a user's throughout — a real bare upstream, a project
//! created over `POST /api/projects` and waited on until it is `ready`, an
//! agent credential stored through `POST /api/secrets`, profiles created and
//! changed over the profiles endpoint — so a scenario cannot arrange a row the
//! product cannot produce.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use chrono::{DateTime, TimeZone, Utc};
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::cron::{CronService, JobName, JobReport};
use mars_orchestrator::git::testutil::TestUpstream;
use mars_orchestrator::models::{
    ProjectStatus, ProjectUpdate, Session, SessionLaunchSource, SessionState,
};
use mars_orchestrator::projects::clone_job;
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository};
use serde_json::{Value, json};
use uuid::Uuid;

/// How long a clone or a launch may take before a scenario gives up with a
/// message rather than hanging.
const PATIENCE: Duration = Duration::from_secs(30);

/// How often the polls re-read what they are waiting for.
const POLL: Duration = Duration::from_millis(25);

/// Not a real credential: the value every credential row below holds (rule 3).
const FAKE_CREDENTIAL: &str = "fake-value-not-a-credential";

/// The Claude adapter's preferred credential name (`agent::credential_names`).
const OAUTH_TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// What every scheduled profile below is told to do.
const PROMPT: &str = "scan the repository for tech debt and file tasks";

/// Every minute, which is exactly the job's own period.
const EVERY_MINUTE: &str = "* * * * *";

/// 03:30 UTC daily, the expression the "before the process started" scenarios
/// place their instants around.
const DAILY_AT_0330: &str = "30 3 * * *";

/// An instant of the fixed day these scenarios are set on.
fn at(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 22, hour, minute, 0)
        .single()
        .expect("a fixed UTC instant")
}

/// A signed-in user, a `ready` project and an agent credential the job can
/// resolve without a user.
struct Fixture {
    /// Held because dropping it removes the upstream the mirror was cloned
    /// from.
    _upstream: TestUpstream,
    user: AuthenticatedUser,
    project_id: Uuid,
}

impl Fixture {
    async fn create(app: &TestApp) -> Self {
        let upstream = TestUpstream::create().await;
        let name = Uuid::new_v4().simple().to_string()[..8].to_string();
        let user = app
            .create_user(
                &format!("user-{name}"),
                &format!("user-{name}@example.test"),
                &format!("fake-password-{name}"),
            )
            .await;

        let response = app
            .post_as(&user, "/api/projects")
            .json(&json!({
                "name": format!("project-{name}"),
                "remote_url": format!("file://{}", upstream.path.display()),
                "default_branch": "main",
            }))
            .await;
        response.assert_status(StatusCode::CREATED);
        let project_id = id_of(&response.json::<Value>());

        let cloned = clone_job::wait_for_clone(&app.state, project_id, PATIENCE).await;
        assert_eq!(
            cloned.status,
            ProjectStatus::Ready,
            "the fixture clone failed: {:?}",
            cloned.status_message,
        );

        // A schedule is refused at save without one, and the job checks again
        // at every tick (`ARCHITECTURE.md`, "Eligibility").
        let stored = app
            .post_as(&user, "/api/secrets")
            .json(&json!({
                "scope": "project",
                "scope_id": project_id.to_string(),
                "name": OAUTH_TOKEN,
                "value": FAKE_CREDENTIAL,
            }))
            .await;
        stored.assert_status(StatusCode::CREATED);

        Self {
            _upstream: upstream,
            user,
            project_id,
        }
    }

    /// A scheduled ephemeral profile, capped at `max_concurrent` live sessions
    /// of its own.
    async fn scheduled_profile(
        &self,
        app: &TestApp,
        name: &str,
        cron: &str,
        max_concurrent: i32,
    ) -> Uuid {
        let response = app
            .post_as(
                &self.user,
                &format!("/api/projects/{}/profiles", self.project_id),
            )
            .json(&json!({
                "name": name,
                "kind": "ephemeral",
                "max_concurrent": max_concurrent,
                "schedule_cron": cron,
                "schedule_prompt": PROMPT,
            }))
            .await;
        response.assert_status(StatusCode::CREATED);

        id_of(&response.json::<Value>())
    }

    /// Replace the profile with the same one minus its schedule, as the
    /// profile editor does when a user empties the field.
    async fn clear_schedule(&self, app: &TestApp, profile_id: Uuid, name: &str) {
        let response = app
            .put_as(
                &self.user,
                &format!("/api/projects/{}/profiles/{profile_id}", self.project_id),
            )
            .json(&json!({
                "name": name,
                "kind": "ephemeral",
            }))
            .await;
        response.assert_status(StatusCode::OK);
        assert_eq!(response.json::<Value>()["schedule_cron"], Value::Null);
    }

    /// Set the project's automation pause, as the project page does.
    async fn set_paused(&self, app: &TestApp, paused: bool) {
        let update = ProjectUpdate {
            automation_paused: Some(paused),
            ..ProjectUpdate::default()
        };

        let mut tx = app.pool.begin().await.expect("a transaction begins");
        ProjectRepository::new(&app.pool)
            .update(&mut tx, self.project_id, &update)
            .await
            .expect("the project updates")
            .expect("the project is there");
        tx.commit().await.expect("the update commits");
    }

    /// Every session of this project, oldest first.
    async fn sessions(&self, app: &TestApp) -> Vec<Session> {
        let mut sessions = SessionRepository::new(&app.pool)
            .list_by_project(self.project_id, None)
            .await
            .expect("the list runs");
        sessions.sort_by_key(|session| session.created_at);
        sessions
    }

    /// The profile row as the job reads it.
    async fn last_scheduled_at(&self, app: &TestApp, profile_id: Uuid) -> Option<DateTime<Utc>> {
        ProjectRepository::new(&app.pool)
            .list_scheduled_profiles(self.project_id)
            .await
            .expect("the scan runs")
            .into_iter()
            .find(|profile| profile.id == profile_id)
            .and_then(|profile| profile.last_scheduled_at)
    }
}

/// The job over this app's state, with the process-start floor placed.
fn cron(app: &TestApp, started_at: DateTime<Utc>) -> CronService {
    CronService::with_started_at(app.state.clone(), started_at)
}

/// One run of the job, through the same dispatch the loop uses.
async fn run(service: &CronService, now: DateTime<Utc>) -> JobReport {
    service
        .run_once(JobName::Scheduler, now)
        .await
        .expect("the sweep runs")
}

/// The `id` of a resource body, as a [`Uuid`].
fn id_of(body: &Value) -> Uuid {
    body["id"]
        .as_str()
        .expect("a resource carries an id")
        .parse()
        .expect("the id is a uuid")
}

/// Wait until the session is `running`, so nothing it does afterwards can
/// change a later run's capacity mid-assertion.
async fn wait_until_running(app: &TestApp, id: Uuid) -> Session {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    loop {
        let session = SessionRepository::new(&app.pool)
            .get(id)
            .await
            .expect("the session reads");
        if session.state == SessionState::Running {
            return session;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "the scheduled session did not start within {PATIENCE:?}: {:?} {:?}",
            session.state,
            session.error,
        );
        tokio::time::sleep(POLL).await;
    }
}

// ---- a due tick ----

#[tokio::test]
async fn a_due_tick_fires_once_and_the_same_window_never_again() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let profile_id = fixture
        .scheduled_profile(&app, "scout", EVERY_MINUTE, 4)
        .await;

    let service = cron(&app, at(3, 0));
    let now = at(3, 1);

    let first = run(&service, now).await;
    assert_eq!(
        first,
        JobReport {
            items: 1,
            ..JobReport::default()
        },
        "the tick at 03:01 did not fire",
    );

    // The claim is what makes the second run empty, and it is committed: a
    // second run of the same instant — a restart inside the tick's own minute
    // — finds the window closed behind it.
    assert_eq!(fixture.last_scheduled_at(&app, profile_id).await, Some(now));
    assert_eq!(run(&service, now).await, JobReport::default());
    // And so does a second service, which is what a restart really is.
    assert_eq!(
        run(&cron(&app, at(3, 0)), now).await,
        JobReport::default(),
        "a fresh service fired the same tick again",
    );

    assert_eq!(fixture.sessions(&app).await.len(), 1);
}

#[tokio::test]
async fn a_scheduled_session_is_an_ordinary_task_less_ephemeral_launch() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture
        .scheduled_profile(&app, "scout", EVERY_MINUTE, 4)
        .await;

    let now = at(3, 1);
    run(&cron(&app, at(3, 0)), now).await;

    let sessions = fixture.sessions(&app).await;
    let [session] = sessions.as_slice() else {
        panic!("one session was launched, not {}", sessions.len());
    };

    assert_eq!(session.launch_source, SessionLaunchSource::Schedule);
    assert_eq!(session.created_by, None, "a scheduled run has no user");
    assert_eq!(session.task_id, None, "a scheduled run claims no task");
    assert_eq!(
        session.base_ref, "main",
        "a scheduled run starts from the default branch",
    );
    assert_eq!(
        session.title.as_deref(),
        Some("scout — scheduled run 2026-09-22T03:01:00Z"),
        "the title does not name the schedule and its run",
    );

    let running = wait_until_running(&app, session.id).await;
    assert_eq!(running.state, SessionState::Running);
}

#[tokio::test]
async fn several_occurrences_in_one_window_fire_once() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture
        .scheduled_profile(&app, "scout", EVERY_MINUTE, 4)
        .await;

    // Ten minutes since the process started, so ten occurrences are inside the
    // window — the shape of a run that outlived its own period. Ticks are not
    // a queue.
    let report = run(&cron(&app, at(3, 0)), at(3, 10)).await;

    assert_eq!(
        report,
        JobReport {
            items: 1,
            ..JobReport::default()
        },
    );
    assert_eq!(fixture.sessions(&app).await.len(), 1);
}

// ---- the process-start floor ----

#[tokio::test]
async fn an_occurrence_from_before_the_process_started_is_never_caught_up() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture
        .scheduled_profile(&app, "nightly", DAILY_AT_0330, 4)
        .await;

    // 03:30 passed while the service was down, and the process came up at
    // 04:00. The next run of this schedule is tomorrow's.
    let report = run(&cron(&app, at(4, 0)), at(5, 0)).await;

    assert_eq!(report, JobReport::default(), "a missed tick was caught up");
    assert!(fixture.sessions(&app).await.is_empty());

    // And the floor is what did it: the same instants under a process that was
    // already up at 03:00 fire.
    let report = run(&cron(&app, at(3, 0)), at(5, 0)).await;
    assert_eq!(
        report,
        JobReport {
            items: 1,
            ..JobReport::default()
        },
    );
}

// ---- a tick nothing may launch ----

#[tokio::test]
async fn a_paused_projects_tick_is_spent_and_the_next_one_fires() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let profile_id = fixture
        .scheduled_profile(&app, "scout", EVERY_MINUTE, 4)
        .await;
    fixture.set_paused(&app, true).await;

    let service = cron(&app, at(3, 0));

    let refused = run(&service, at(3, 1)).await;
    assert_eq!(
        refused,
        JobReport {
            skipped: 1,
            ..JobReport::default()
        },
    );
    assert!(fixture.sessions(&app).await.is_empty());
    // Spent, not queued: the tick is recorded as having happened.
    assert_eq!(
        fixture.last_scheduled_at(&app, profile_id).await,
        Some(at(3, 1)),
    );

    fixture.set_paused(&app, false).await;

    let fired = run(&service, at(3, 2)).await;
    assert_eq!(
        fired,
        JobReport {
            items: 1,
            ..JobReport::default()
        },
        "the tick after the pause was lifted did not fire",
    );
    assert_eq!(
        fixture.sessions(&app).await.len(),
        1,
        "the skipped tick was replayed",
    );
}

#[tokio::test]
async fn a_tick_the_profile_cap_refuses_is_spent_and_a_later_one_fires() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let profile_id = fixture
        .scheduled_profile(&app, "scout", EVERY_MINUTE, 1)
        .await;

    let service = cron(&app, at(3, 0));

    assert_eq!(
        run(&service, at(3, 1)).await,
        JobReport {
            items: 1,
            ..JobReport::default()
        },
    );
    let first = fixture.sessions(&app).await.remove(0);
    // Live, and so counted against `max_concurrent = 1`.
    wait_until_running(&app, first.id).await;

    let refused = run(&service, at(3, 2)).await;
    assert_eq!(
        refused,
        JobReport {
            skipped: 1,
            ..JobReport::default()
        },
        "a cap with no room launched anyway",
    );
    assert_eq!(fixture.sessions(&app).await.len(), 1);
    assert_eq!(
        fixture.last_scheduled_at(&app, profile_id).await,
        Some(at(3, 2)),
        "the refused tick was not spent",
    );

    // End it through the product, so the cap has room again.
    let ended = app
        .post_as(&fixture.user, &format!("/api/sessions/{}/end", first.id))
        .await;
    ended.assert_status(StatusCode::OK);

    assert_eq!(
        run(&service, at(3, 3)).await,
        JobReport {
            items: 1,
            ..JobReport::default()
        },
        "the tick after the cap freed up did not fire",
    );
    assert_eq!(fixture.sessions(&app).await.len(), 2);
}

// ---- no schedule, nothing to fire ----

#[tokio::test]
async fn clearing_the_schedule_stops_the_ticks() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let profile_id = fixture
        .scheduled_profile(&app, "scout", EVERY_MINUTE, 4)
        .await;

    let service = cron(&app, at(3, 0));
    assert_eq!(
        run(&service, at(3, 1)).await,
        JobReport {
            items: 1,
            ..JobReport::default()
        },
    );

    fixture.clear_schedule(&app, profile_id, "scout").await;

    assert_eq!(
        run(&service, at(3, 2)).await,
        JobReport::default(),
        "a profile with no schedule fired",
    );
    assert_eq!(fixture.sessions(&app).await.len(), 1);
    // Cleared with the schedule, so a schedule set later starts with nothing
    // behind it (`ProjectRepository::update_profile`).
    assert_eq!(fixture.last_scheduled_at(&app, profile_id).await, None);
}

#[tokio::test]
async fn an_unscheduled_profile_is_not_in_the_scan() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    // The project is seeded with role profiles, none of them scheduled.
    let scheduled = ProjectRepository::new(&app.pool)
        .list_scheduled_profiles(fixture.project_id)
        .await
        .expect("the scan runs");
    assert!(scheduled.is_empty(), "{scheduled:?}");

    assert_eq!(
        run(&cron(&app, at(3, 0)), at(3, 10)).await,
        JobReport::default()
    );
    assert!(fixture.sessions(&app).await.is_empty());
}
