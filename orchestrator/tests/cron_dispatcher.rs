//! The dispatcher job against a real database and the mock engine
//! (`ARCHITECTURE.md`, "Dispatcher" and "Task tracker" → "Unattended
//! launches"; ADR 0042).
//!
//! The job is driven directly through `TestApp::cron()`, one run at a time with
//! the `now` the scenario chose, because what is under test is a sweep and not
//! a timer — the loop, its interval, its debounce and its non-overlap belong to
//! `cron::scheduler` and are tested there.
//!
//! The one exception is the last section, "the wake-up": what it asserts is
//! that a committed task event really reaches the job through the shared
//! listener and its fan-out, which nothing below the whole app can show. Those
//! scenarios start the real loop and its waker through
//! `CronService::start_job`, on the suite's `DISPATCHER_INTERVAL_SECS` of 45 —
//! long enough that no launch they wait for can be a tick — and stop both
//! tasks at the end the way the drain does. Nothing sleeps for a fixed period
//! to wait for a launch: the polls have deadlines.
//!
//! Everything the job composes has its own suite already: the claim
//! transaction and the actor are `tests/session_create.rs`'s, the four bounds
//! are `tests/session_capacity.rs`'s, the hand-off base is
//! `tests/handoffs_launch.rs`'s and the ordering is
//! `tests/tracker_leases.rs`'s. What is asserted here is only what the
//! dispatcher itself decides: *which* profile, *which* task, *whether* now, and
//! what it does when the answer is no.
//!
//! The arrangement is a user's throughout — a real bare upstream, a project
//! created over `POST /api/projects` and waited on until it is `ready`, an
//! agent credential stored through `POST /api/secrets`, profiles created over
//! the profiles endpoint — so a scenario cannot arrange a row the product
//! cannot produce. Task preconditions go through `tests/common/tracker.rs` for
//! the same reason.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use chrono::Utc;
use common::{AuthenticatedUser, TestApp};
use mars_orchestrator::cron::{JobName, JobReport};
use mars_orchestrator::events::{TaskActor, TaskEvent, TaskEventKind};
use mars_orchestrator::git::testutil::TestUpstream;
use mars_orchestrator::models::{
    HandoffCaller, NewSession, NewTask, Priority, ProfileKind, ProjectStatus, ProjectUpdate,
    Session, SessionLaunchSource, SessionState, Task,
};
use mars_orchestrator::projects::clone_job;
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository, TaskRepository};
use mars_orchestrator::session::McpToken;
use mars_orchestrator::tracker::{ReviewCarry, TaskDto, TrackerMutation};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::task::JoinHandle;
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

/// A plausible object id no repository here has: the database half of a
/// publication never reads it (`tests/common/tracker.rs`).
const HANDOFF_COMMIT: &str = "1f0e4d3c2b1a09876543210fedcba9876543210f";

/// A signed-in user, a `ready` project and an agent credential the jobs can
/// resolve without a user.
struct Fixture {
    /// Held because dropping it removes the upstream the mirror was cloned
    /// from.
    _upstream: TestUpstream,
    user: AuthenticatedUser,
    project_id: Uuid,
    /// The id of the `project`-scope credential row, for the one scenario that
    /// takes it away again.
    credential_id: Uuid,
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

        // An `auto_launch` profile is refused at save without one, and the job
        // checks again at launch (`ARCHITECTURE.md`, "Eligibility").
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
        let credential_id = id_of(&stored.json::<Value>());

        Self {
            _upstream: upstream,
            user,
            project_id,
            credential_id,
        }
    }

    /// An `auto_launch` ephemeral profile serving `states`, capped at
    /// `max_concurrent` live sessions of its own.
    async fn auto_profile(
        &self,
        app: &TestApp,
        name: &str,
        states: &[&str],
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
                "auto_launch": true,
                "max_concurrent": max_concurrent,
                "serves_states": states,
            }))
            .await;
        response.assert_status(StatusCode::CREATED);

        id_of(&response.json::<Value>())
    }

    /// A task of this project in the named state, at `priority`, inserted the
    /// way the tracker inserts one.
    async fn task(&self, app: &TestApp, title: &str, state: &str, priority: Priority) -> Task {
        let tasks = TaskRepository::new(&app.pool);
        let mut new = NewTask::new(self.project_id, title).expect("the title parses");
        new.priority = priority;
        new.state_id = Some(
            tasks
                .find_state_by_name(self.project_id, state)
                .await
                .expect("the state reads")
                .unwrap_or_else(|| panic!("a project is seeded with a {state} state"))
                .id,
        );

        let mut mutation = TrackerMutation::begin(&app.pool, self.project_id, TaskActor::System)
            .await
            .expect("the mutation opens");
        let inserted = tasks
            .insert_task(mutation.conn(), self.project_id, &new)
            .await
            .expect("the task inserts");
        mutation.commit().await.expect("the mutation commits");

        inserted
    }

    /// The task as it is committed.
    async fn read_task(&self, app: &TestApp, task_id: Uuid) -> TaskDto {
        TaskRepository::new(&app.pool)
            .load_task_dto(self.project_id, task_id)
            .await
            .expect("the task reads")
            .expect("the task is in this project")
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

    /// This task's committed events, oldest first.
    async fn task_events(&self, app: &TestApp, task_id: Uuid) -> Vec<TaskEvent> {
        TaskRepository::new(&app.pool)
            .list_task_events_after(self.project_id, 0, 500)
            .await
            .expect("the events read")
            .into_iter()
            .map(|row| TaskEvent::from_row(row).expect("the row is a documented event"))
            .filter(|event| event.task_id == Some(task_id))
            .collect()
    }

    /// A session row with no launch, for a lease that has to be held by
    /// somebody.
    async fn seed_session(&self, app: &TestApp, profile_id: Uuid) -> Session {
        let mut new_session = NewSession::new(
            self.project_id,
            profile_id,
            ProfileKind::Conversational,
            "main",
            McpToken::generate().hash(),
        );
        new_session.created_by = Some(self.user.user.id);
        let new_session = new_session
            .with_title(Some("the holder"))
            .expect("the fixture title is valid");

        let mut tx = app.pool.begin().await.expect("a transaction begins");
        let inserted = SessionRepository::new(&app.pool)
            .insert(&mut tx, &new_session)
            .await
            .expect("the session inserts");
        tx.commit().await.expect("the transaction commits");

        inserted
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
}

/// One run of the dispatcher.
async fn dispatch(app: &TestApp) -> JobReport {
    app.cron()
        .dispatcher(Utc::now())
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
            "the dispatched session did not start within {PATIENCE:?}: {:?} {:?}",
            session.state,
            session.error,
        );
        tokio::time::sleep(POLL).await;
    }
}

/// The `-p` prompt the launcher gave this session's container.
async fn prompt_of(app: &TestApp, session_id: Uuid) -> String {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    let container_id = loop {
        if let Some(id) = app.engine().container_id_for_session(session_id) {
            break id;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no container was created for the dispatched session within {PATIENCE:?}",
        );
        tokio::time::sleep(POLL).await;
    };

    let cmd = app
        .engine()
        .spec_of(&container_id)
        .expect("the specification was recorded")
        .cmd;

    cmd.iter()
        .position(|argument| argument == "-p")
        .map(|at| cmd[at + 1].clone())
        .expect("an ephemeral session runs its prompt")
}

// ---- the happy path ----

/// One ready task, one `auto_launch` profile: a dispatched session holding it,
/// with the whole of the documented attribution on the row.
#[tokio::test]
async fn a_claimable_task_is_dispatched_with_a_system_claim_and_no_user() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let profile_id = fixture
        .auto_profile(&app, "auto-worker", &["ready"], 2)
        .await;
    let task = fixture
        .task(&app, "Fix the login form", "ready", Priority::MEDIUM)
        .await;

    let report = dispatch(&app).await;
    assert_eq!(report.items, 1, "one task, one launch: {report:?}");
    assert_eq!(report.failures, 0, "{report:?}");

    let sessions = fixture.sessions(&app).await;
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];

    // Nobody asked for it, so there is nobody to attribute it to, and
    // `launch_source` is what records which job did (ADR 0042).
    assert_eq!(session.launch_source, SessionLaunchSource::Dispatcher);
    assert_eq!(session.created_by, None);
    assert_eq!(session.task_id, Some(task.id));
    assert_eq!(session.profile_id, profile_id);
    assert_eq!(session.kind, ProfileKind::Ephemeral);

    // The claim committed with the row, under the orchestrator's own actor.
    let claimed = fixture.read_task(&app, task.id).await;
    assert_eq!(claimed.lease_holder_session_id, Some(session.id));
    assert_eq!(claimed.attempts, 1);

    let events = fixture.task_events(&app, task.id).await;
    let claim = events
        .iter()
        .find(|event| event.kind == TaskEventKind::Claimed)
        .expect("the launch claimed the task");
    assert_eq!(claim.actor, TaskActor::System);

    // An ephemeral session has no stdin, so the generated task message is the
    // prompt it runs (`SPEC.md`, "Sessions").
    let prompt = prompt_of(&app, session.id).await;
    assert!(
        prompt.contains("Fix the login form"),
        "the generated task message is not the prompt: {prompt}",
    );
}

/// The task dispatched is the first row `ready` would have offered: priority
/// first, then task number — never insertion order.
#[tokio::test]
async fn the_dispatched_task_is_the_one_ready_would_have_offered_first() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    // One launch at a time, so the choice is forced to show itself.
    fixture
        .auto_profile(&app, "auto-worker", &["ready"], 1)
        .await;

    // Inserted worst-first, so insertion order and priority order disagree.
    let ordinary = fixture
        .task(&app, "Tidy the imports", "ready", Priority::LOW)
        .await;
    let urgent = fixture
        .task(&app, "Production is down", "ready", Priority::CRITICAL)
        .await;

    let report = dispatch(&app).await;
    assert_eq!(report.items, 1, "{report:?}");

    let sessions = fixture.sessions(&app).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].task_id, Some(urgent.id));

    assert_eq!(
        fixture
            .read_task(&app, ordinary.id)
            .await
            .lease_holder_session_id,
        None,
        "the low-priority task was claimed over the critical one",
    );
}

/// A task with code waiting on it is dispatched from that commit, not from the
/// project's integration head: the dispatcher names no `base_ref`, so the
/// pinned hand-off is what the launch starts from (`ARCHITECTURE.md`, "Task
/// tracker" → "Launching a session for a task").
///
/// The commit is a plausible-looking object id that no repository has, which is
/// all the database half of a publication needs (`tests/common/tracker.rs`);
/// what is asserted is the session row the claim transaction wrote, and the
/// clone that would read the ref is `tests/handoffs_launch.rs`'s subject.
#[tokio::test]
async fn a_dispatched_session_starts_from_the_tasks_current_hand_off() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let profile_id = fixture
        .auto_profile(&app, "auto-reviewer", &["ready"], 2)
        .await;
    let task = fixture
        .task(&app, "Review the parser", "ready", Priority::HIGH)
        .await;

    let source = fixture.seed_session(&app, profile_id).await;
    let branch = format!("session/{}", source.id);
    let (_, handoff_id) = common::tracker::handoff_in_place(
        &app.pool,
        fixture.project_id,
        task.id,
        common::tracker::Handoff {
            source_session_id: Some(source.id),
            source_branch: &branch,
            commit: HANDOFF_COMMIT,
            comment: "ready for review",
            target_state: "",
            caller: HandoffCaller::User {
                user_id: fixture.user.user.id,
            },
            review: ReviewCarry::Fresh,
        },
    )
    .await;

    let report = dispatch(&app).await;
    assert_eq!(report.items, 1, "{report:?}");

    let sessions = fixture.sessions(&app).await;
    let launched = sessions
        .iter()
        .find(|session| session.launch_source == SessionLaunchSource::Dispatcher)
        .expect("the task was dispatched");
    assert_eq!(launched.handoff_id, Some(handoff_id));
    assert_eq!(
        launched.base_ref, HANDOFF_COMMIT,
        "the dispatcher started from the integration head instead of the hand-off",
    );
}

// ---- the bounds ----

/// The profile's own cap is re-asked on every run, and a run that is full
/// launches nothing and leaves the next task exactly where it was.
#[tokio::test]
async fn the_profile_cap_holds_the_second_task_back_on_the_next_run() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture
        .auto_profile(&app, "auto-worker", &["ready"], 1)
        .await;

    let first = fixture
        .task(&app, "Fix the login form", "ready", Priority::HIGH)
        .await;
    let second = fixture
        .task(&app, "Fix the signup form", "ready", Priority::LOW)
        .await;

    let first_run = dispatch(&app).await;
    assert_eq!(first_run.items, 1, "{first_run:?}");
    // The cap refused the second candidate of the same run.
    assert!(first_run.skipped >= 1, "{first_run:?}");

    let sessions = fixture.sessions(&app).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].task_id, Some(first.id));
    // Live and stable, so the second run's counts cannot change underneath it.
    wait_until_running(&app, sessions[0].id).await;

    let second_run = dispatch(&app).await;
    assert_eq!(
        second_run.items, 0,
        "the cap did not hold on the second run: {second_run:?}",
    );
    assert_eq!(second_run.failures, 0, "{second_run:?}");
    assert_eq!(fixture.sessions(&app).await.len(), 1);
    assert_eq!(
        fixture
            .read_task(&app, second.id)
            .await
            .lease_holder_session_id,
        None,
        "a task was claimed with no capacity to run it",
    );
}

/// A paused project dispatches nothing, however claimable its board is, and
/// picks up again the moment the toggle goes back.
#[tokio::test]
async fn a_paused_project_dispatches_nothing_until_it_is_resumed() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture
        .auto_profile(&app, "auto-worker", &["ready"], 2)
        .await;
    let task = fixture
        .task(&app, "Fix the login form", "ready", Priority::HIGH)
        .await;

    fixture.set_paused(&app, true).await;

    let paused_run = dispatch(&app).await;
    assert_eq!(paused_run.items, 0, "{paused_run:?}");
    assert_eq!(paused_run.skipped, 1, "{paused_run:?}");
    assert!(fixture.sessions(&app).await.is_empty());
    assert_eq!(
        fixture.read_task(&app, task.id).await.attempts,
        0,
        "a paused project claimed a task",
    );

    fixture.set_paused(&app, false).await;

    let resumed_run = dispatch(&app).await;
    assert_eq!(resumed_run.items, 1, "{resumed_run:?}");
    assert_eq!(
        fixture.sessions(&app).await[0].task_id,
        Some(task.id),
        "the resumed run dispatched something else",
    );
}

// ---- who gets the task ----

/// Two `auto_launch` profiles serving one state: the older by `created_at`
/// claims the task, and the younger finds nothing left (`ARCHITECTURE.md`,
/// "Dispatcher").
#[tokio::test]
async fn the_older_of_two_profiles_serving_one_state_wins() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let older = fixture.auto_profile(&app, "veteran", &["ready"], 2).await;
    let younger = fixture.auto_profile(&app, "newcomer", &["ready"], 2).await;
    let task = fixture
        .task(&app, "Fix the login form", "ready", Priority::HIGH)
        .await;

    let report = dispatch(&app).await;
    assert_eq!(
        report.items, 1,
        "one task cannot be launched twice: {report:?}"
    );

    let sessions = fixture.sessions(&app).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].profile_id, older, "the younger profile won");
    assert_ne!(sessions[0].profile_id, younger);
    assert_eq!(sessions[0].task_id, Some(task.id));
}

/// A task somebody already holds is not a candidate, and the next one is
/// dispatched in its place rather than the run stopping there.
#[tokio::test]
async fn a_held_task_is_stepped_over_and_the_next_one_is_dispatched() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let profile_id = fixture
        .auto_profile(&app, "auto-worker", &["ready"], 2)
        .await;

    let held = fixture
        .task(&app, "Already being worked on", "ready", Priority::CRITICAL)
        .await;
    let free = fixture
        .task(&app, "Fix the login form", "ready", Priority::LOW)
        .await;

    // Through the claim verb, so the lease and `attempts` are what a claim
    // really leaves behind (`tests/common/tracker.rs`).
    let holder = fixture.seed_session(&app, profile_id).await;
    common::tracker::hold(&app.pool, fixture.project_id, held.id, holder.id).await;

    let report = dispatch(&app).await;
    assert_eq!(report.items, 1, "{report:?}");

    let sessions = fixture.sessions(&app).await;
    let launched: Vec<&Session> = sessions
        .iter()
        .filter(|session| session.launch_source == SessionLaunchSource::Dispatcher)
        .collect();
    assert_eq!(launched.len(), 1);
    assert_eq!(launched[0].task_id, Some(free.id));

    // And the holder kept its lease: the dispatcher never takes one back.
    assert_eq!(
        fixture
            .read_task(&app, held.id)
            .await
            .lease_holder_session_id,
        Some(holder.id),
    );
}

// ---- what is never dispatched ----

/// A state the profile does not serve, a blocked task and a task waiting for a
/// person are all out of reach, and a run that finds only those does nothing
/// at all (`ARCHITECTURE.md`, "Dispatcher"; ADR 0042).
#[tokio::test]
async fn unserved_blocked_and_human_tasks_are_left_alone() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    // Serves `ready` and nothing else.
    fixture
        .auto_profile(&app, "auto-worker", &["ready"], 2)
        .await;

    let unserved = fixture
        .task(&app, "Groom the backlog", "backlog", Priority::CRITICAL)
        .await;
    let blocked = fixture
        .task(&app, "Blocked on the parser", "ready", Priority::CRITICAL)
        .await;
    common::tracker::block(&app.pool, fixture.project_id, blocked.id).await;
    let human = fixture
        .task(&app, "Somebody must decide", "ready", Priority::CRITICAL)
        .await;
    common::tracker::move_to(&app.pool, fixture.project_id, human.id, "needs_human").await;

    let report = dispatch(&app).await;
    assert_eq!(
        report.items, 0,
        "something unreachable was dispatched: {report:?}"
    );
    assert_eq!(report.failures, 0, "{report:?}");
    assert!(fixture.sessions(&app).await.is_empty());

    for (task, what) in [
        (unserved.id, "a task in an unserved state"),
        (blocked.id, "a blocked task"),
        (human.id, "a task waiting for a person"),
    ] {
        let read = fixture.read_task(&app, task).await;
        assert_eq!(read.lease_holder_session_id, None, "{what} was claimed");
        assert_eq!(read.attempts, 0, "{what} was attempted");
    }
}

/// A profile whose agent credential has been deleted since it was saved is
/// skipped, claims nothing, and does not stop the profile beside it.
#[tokio::test]
async fn a_profile_without_a_resolvable_credential_claims_nothing() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture
        .auto_profile(&app, "auto-worker", &["ready"], 2)
        .await;
    let task = fixture
        .task(&app, "Fix the login form", "ready", Priority::HIGH)
        .await;

    // The save was legitimate; the credential is what went away afterwards
    // (`ARCHITECTURE.md`, "Eligibility").
    let removed = app
        .delete_as(
            &fixture.user,
            &format!("/api/secrets/{}", fixture.credential_id),
        )
        .await;
    removed.assert_status(StatusCode::NO_CONTENT);

    let report = dispatch(&app).await;
    assert_eq!(report.items, 0, "{report:?}");
    assert_eq!(report.skipped, 1, "{report:?}");
    assert_eq!(report.failures, 0, "{report:?}");
    assert!(fixture.sessions(&app).await.is_empty());
    assert_eq!(
        fixture.read_task(&app, task.id).await.attempts,
        0,
        "a profile that cannot authenticate claimed a task",
    );
}

/// A `conversational` profile is never dispatched, whatever its row says.
///
/// `auto_launch` is refused on one at save, so the only way to reach the
/// backstop is to write the column the API will not: the one raw statement in
/// this suite, for the one fact no interface can arrange.
#[tokio::test]
async fn a_conversational_profile_is_never_dispatched() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    let response = app
        .post_as(
            &fixture.user,
            &format!("/api/projects/{}/profiles", fixture.project_id),
        )
        .json(&json!({
            "name": "pair",
            "kind": "conversational",
            "serves_states": ["ready"],
        }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let profile_id = id_of(&response.json::<Value>());

    // The state `auto_launch` validation exists to prevent.
    sqlx::query("UPDATE agent_profiles SET auto_launch = TRUE WHERE id = $1")
        .bind(profile_id)
        .execute(&app.pool)
        .await
        .expect("the column is writable");

    let task = fixture
        .task(&app, "Fix the login form", "ready", Priority::HIGH)
        .await;

    let report = dispatch(&app).await;
    assert_eq!(report.items, 0, "a conversation was launched: {report:?}");
    assert_eq!(report.skipped, 1, "{report:?}");
    assert!(fixture.sessions(&app).await.is_empty());
    assert_eq!(fixture.read_task(&app, task.id).await.attempts, 0);
}

/// A project that is not `ready` has no repository to clone and is not swept
/// at all.
#[tokio::test]
async fn a_project_that_is_not_ready_is_not_swept() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture
        .auto_profile(&app, "auto-worker", &["ready"], 2)
        .await;
    let task = fixture
        .task(&app, "Fix the login form", "ready", Priority::HIGH)
        .await;

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    ProjectRepository::new(&app.pool)
        .set_status(
            &mut tx,
            fixture.project_id,
            ProjectStatus::Error,
            Some("the upstream went away"),
        )
        .await
        .expect("the project moves to error")
        .expect("the project is there");
    tx.commit().await.expect("the update commits");

    let report = dispatch(&app).await;
    assert_eq!(report.items, 0, "{report:?}");
    assert!(fixture.sessions(&app).await.is_empty());
    assert_eq!(fixture.read_task(&app, task.id).await.attempts, 0);
}

// ---- the wake-up ----

/// How long a wake-up scenario waits for a launch it expects.
///
/// Well inside `DISPATCHER_INTERVAL_SECS`, which the suite sets to 45
/// (`tests/common/app.rs`): a session that appears within this window cannot
/// be the timer's doing, because the timer's next tick is a good half minute
/// after the startup one every scenario below waits for first.
const WAKE_PATIENCE: Duration = Duration::from_secs(15);

/// Long enough for a wake-up and its debounce to have happened, for the
/// assertions that nothing *more* was launched.
const SETTLE: Duration = Duration::from_secs(1);

/// The dispatcher's loop and its waker, running as `CronService::start` runs
/// them and stopped by [`RunningDispatcher::stop`] the way the drain does.
struct RunningDispatcher {
    shutdown: watch::Sender<bool>,
    handles: Vec<JoinHandle<()>>,
}

impl RunningDispatcher {
    /// Start the job on its own timer, plus the waker that subscribes to the
    /// fan-out.
    fn start(app: &TestApp) -> Self {
        let (shutdown, rx) = watch::channel(false);
        let handles = Arc::new(app.cron()).start_job(JobName::Dispatcher, rx);
        assert_eq!(handles.len(), 2, "the dispatcher runs a loop and a waker");

        RunningDispatcher { shutdown, handles }
    }

    /// Stop both tasks and wait for them, as the drain does.
    async fn stop(self) {
        self.shutdown.send(true).expect("both tasks are listening");
        for handle in self.handles {
            tokio::time::timeout(PATIENCE, handle)
                .await
                .expect("the dispatcher stops within the drain grace")
                .expect("neither task panics");
        }
    }
}

/// Wait until the project has at least `count` sessions, or fail.
async fn wait_for_sessions(
    app: &TestApp,
    fixture: &Fixture,
    count: usize,
    patience: Duration,
) -> Vec<Session> {
    let deadline = tokio::time::Instant::now() + patience;

    loop {
        let sessions = fixture.sessions(app).await;
        if sessions.len() >= count {
            return sessions;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "only {} of {count} sessions were launched within {patience:?}",
            sessions.len(),
        );
        tokio::time::sleep(POLL).await;
    }
}

/// A task moved into a served state launches a session without anybody
/// calling the job: the `task_events` notification wakes it long before its
/// next tick (`ARCHITECTURE.md`, "Dispatcher").
#[tokio::test]
async fn a_task_event_wakes_the_dispatcher_before_its_next_tick() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture
        .auto_profile(&app, "auto-worker", &["ready"], 2)
        .await;

    // One task the startup tick can dispatch, so that the tick is observable
    // and everything after it is provably not the timer.
    let first = fixture
        .task(&app, "Fix the login form", "ready", Priority::HIGH)
        .await;
    // And one the sweep must not touch yet: `backlog` is not served.
    let later = fixture
        .task(&app, "Fix the signup form", "backlog", Priority::HIGH)
        .await;

    let dispatcher = RunningDispatcher::start(&app);
    let launched = wait_for_sessions(&app, &fixture, 1, PATIENCE).await;
    assert_eq!(launched[0].task_id, Some(first.id));
    wait_until_running(&app, launched[0].id).await;

    // The next tick is 45 seconds away. Move the second task into the served
    // state: the tracker's `pg_notify` is the only thing that can start it.
    common::tracker::move_to(&app.pool, fixture.project_id, later.id, "ready").await;

    let sessions = wait_for_sessions(&app, &fixture, 2, WAKE_PATIENCE).await;
    assert_eq!(
        sessions[1].task_id,
        Some(later.id),
        "the wake-up run dispatched something else",
    );
    assert_eq!(
        sessions[1].launch_source,
        SessionLaunchSource::Dispatcher,
        "a wake-up run launches as the dispatcher, like any other run",
    );

    dispatcher.stop().await;
}

/// A burst of task events is one wake-up run, and that run launches up to the
/// cap and no further: five tasks filed at once are not five sessions, and no
/// task is claimed twice by two runs racing each other.
#[tokio::test]
async fn a_burst_of_task_events_launches_up_to_the_cap_and_no_further() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture
        .auto_profile(&app, "auto-worker", &["ready"], 2)
        .await;

    // Arranged where the sweep cannot see them, so the startup tick has
    // nothing to do and every launch below is the burst's.
    let mut queued = Vec::new();
    for number in 0..5 {
        queued.push(
            fixture
                .task(&app, &format!("Task {number}"), "backlog", Priority::HIGH)
                .await,
        );
    }

    let dispatcher = RunningDispatcher::start(&app);

    // The planner files its work: five state changes in a row, five
    // notifications, one debounced run.
    for task in &queued {
        common::tracker::move_to(&app.pool, fixture.project_id, task.id, "ready").await;
    }

    let sessions = wait_for_sessions(&app, &fixture, 2, WAKE_PATIENCE).await;
    for session in &sessions {
        wait_until_running(&app, session.id).await;
    }

    // The follow-up run that every launch's own `claimed` events wake finds
    // the cap full, launches nothing, and the job goes quiet.
    tokio::time::sleep(SETTLE).await;
    let settled = fixture.sessions(&app).await;
    assert_eq!(
        settled.len(),
        2,
        "the burst launched past the profile cap: {settled:?}",
    );

    // Two sessions, two different tasks: no task was claimed twice, which two
    // runs overlapping on one burst would have done.
    let mut held: Vec<Uuid> = settled
        .iter()
        .filter_map(|session| session.task_id)
        .collect();
    held.sort();
    held.dedup();
    assert_eq!(held.len(), 2, "one task was dispatched twice: {settled:?}");

    // And the three the cap held back were left claimable.
    let mut still_free = 0;
    for task in &queued {
        if !held.contains(&task.id) {
            assert_eq!(
                fixture
                    .read_task(&app, task.id)
                    .await
                    .lease_holder_session_id,
                None,
                "a task was claimed with no capacity to run it",
            );
            still_free += 1;
        }
    }
    assert_eq!(still_free, 3);

    dispatcher.stop().await;
}
