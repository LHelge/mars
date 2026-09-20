//! The idle reaper (`ARCHITECTURE.md`, "Background jobs", the idle reaper row;
//! "Session lifecycle", "Stop semantics", "Task tracker" → "Liveness comes from
//! the session"; `docs/data-model.md`, `agent_profiles.idle_timeout_secs`,
//! `sessions.last_activity_at`; ADR 0003).
//!
//! Every scenario runs the real job through `app.cron().idle_reaper(now)` with
//! the clock it chooses, which is the whole reason the job takes a `now`: a
//! timeout is asserted from either side of it without a second of sleeping.
//! `MockEngine` stands in for the container, because a signal, an exit code and
//! a removal are the whole of what the stop rules read of it, and the owner
//! under test is the real [`SessionOwner`] reached through the real
//! [`SessionRegistry`] — the reaper's normal path writes nothing itself, so a
//! mocked owner would prove nothing.
//!
//! What the assertions are about: which sessions the join selects, which signal
//! each kind's stop sends, which state and `state_change` the exit that follows
//! produces, that a pending stop is skipped until it has outlived the whole
//! documented sequence and then escalated to `SIGKILL`, and that a `running`
//! row with no owner at all is transitioned, cleaned up and — when it ends
//! `failed` — handed to the end-of-session hook.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use common::TestApp;
use futures_util::FutureExt;
use mars_orchestrator::agent::{AgentBackend, ClaudeBackend, TranslateConfig};
use mars_orchestrator::cron::{CronService, JobReport};
use mars_orchestrator::engine::{
    ContainerEngine, ContainerId, ContainerSpec, LABEL_PROJECT_ID, LABEL_SESSION_ID, Signal,
};
use mars_orchestrator::events::{AgentEvent, AgentEventBody};
use mars_orchestrator::models::{NewSession, ProfileKind, SessionState};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{SessionRepository, Transition};
use mars_orchestrator::session::{OwnerContext, Phase, SessionDirs, SessionOwner, sigkill_after};
use serde_json::Value;
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for a seeded user's Argon2id
/// PHC string (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// Not a real image: the stub every session fixture names.
const TEST_IMAGE: &str = "localhost/mars-session:test";

/// How long something another task has to do is waited for.
const WITHIN: Duration = Duration::from_secs(10);

/// How long "nothing happened" is given to be proved wrong: several owner tail
/// ticks.
const QUIET_FOR: Duration = Duration::from_millis(600);

/// The clock every scenario arranges around.
///
/// An hour ago, so that a tick a minute past it is still in the past: an event
/// a scenario really appends lands far beyond the tick, which is what makes
/// "a user message resets the clock" assertable without sleeping.
fn base() -> DateTime<Utc> {
    Utc::now() - TimeDelta::hours(1)
}

/// One seeded session, with everything the reaper reads of it.
struct Fixture {
    session_id: Uuid,
    project_id: Uuid,
    kind: ProfileKind,
    dirs: SessionDirs,
}

/// Seed a project, a profile with `idle_timeout_secs` and one session of
/// `kind`, and create its directories.
///
/// Unchecked statements for the foreign keys only, as the other session suites
/// do; the session itself goes in through [`SessionRepository::insert`].
async fn fixture(app: &TestApp, kind: ProfileKind, idle_timeout_secs: i32) -> Fixture {
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email, password_hash) VALUES ($1, $2, $3, $4)")
        .bind(user_id)
        .bind(format!("user-{}", &user_id.simple().to_string()[..8]))
        .bind(format!("{user_id}@example.test"))
        .bind(FAKE_PASSWORD_HASH)
        .execute(&app.pool)
        .await
        .expect("the user seeds");

    let project_id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, name, remote_url) VALUES ($1, $2, $3)")
        .bind(project_id)
        .bind(format!("project-{project_id}"))
        .bind("https://git.example.test/mars.git")
        .execute(&app.pool)
        .await
        .expect("the project seeds");

    let profile_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_profiles
             (id, project_id, name, kind, image, partial_messages, idle_timeout_secs)
         VALUES ($1, $2, 'default', $3, $4, FALSE, $5)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind(kind)
    .bind(TEST_IMAGE)
    .bind(idle_timeout_secs)
    .execute(&app.pool)
    .await
    .expect("the profile seeds");

    let mut new = NewSession::new(
        project_id,
        profile_id,
        kind,
        "main",
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );
    new.created_by = Some(user_id);

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let session = SessionRepository::new(&app.pool)
        .insert(&mut tx, &new)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    let dirs = SessionDirs::from_config(&app.state.config, session.id);
    dirs.ensure()
        .await
        .expect("the session directories are made");

    Fixture {
        session_id: session.id,
        project_id,
        kind,
        dirs,
    }
}

/// Move the session to `running` the way the launcher does when it attaches
/// stdin (ADR 0032).
async fn mark_running(app: &TestApp, fixture: &Fixture) {
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .transition(
            &mut tx,
            fixture.session_id,
            &Transition::new(
                SessionState::Creating,
                SessionState::Running,
                "container started, stdin attached",
            ),
        )
        .await
        .expect("the session goes running");
    tx.commit().await.expect("the transaction commits");
}

/// Put this session's idle clock at `at`.
///
/// Raw SQL because `last_activity_at` is only ever written as a side effect of
/// appending an event, and no interface sets it to a chosen instant — which is
/// exactly what a timeout test needs.
async fn set_last_activity(app: &TestApp, session_id: Uuid, at: DateTime<Utc>) {
    sqlx::query("UPDATE sessions SET last_activity_at = $2 WHERE id = $1")
        .bind(session_id)
        .bind(at)
        .execute(&app.pool)
        .await
        .expect("the idle clock is arranged");
}

/// Create and start a mock container for this session and record its id, the
/// way the launcher does.
async fn start_container(app: &TestApp, fixture: &Fixture) -> ContainerId {
    let spec = ContainerSpec {
        image: TEST_IMAGE.to_string(),
        name: format!("mars-session-{}", fixture.session_id),
        labels: BTreeMap::from([
            (LABEL_SESSION_ID.to_string(), fixture.session_id.to_string()),
            (LABEL_PROJECT_ID.to_string(), fixture.project_id.to_string()),
        ]),
        user: "1000:1000".to_string(),
        working_dir: "/session/work".to_string(),
        cmd: vec!["claude".to_string()],
        env: Vec::new(),
        secret_env: Vec::new(),
        binds: Vec::new(),
        network: "mars-sessions".to_string(),
        extra_hosts: Vec::new(),
        runtime: None,
    };

    let engine = app.engine();
    let container_id = engine
        .create(&spec)
        .await
        .expect("the container is created");
    engine
        .start(&container_id)
        .await
        .expect("the container starts");

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .set_container_id(&mut tx, fixture.session_id, Some(&container_id.0))
        .await
        .expect("the container id is recorded");
    tx.commit().await.expect("the transaction commits");

    container_id
}

/// Spawn a real owner for this session, registered in the app's own registry.
///
/// The registry's receiver is the owner's, which is what makes
/// `SessionRegistry::stop` reach it: an entry whose receiver has been dropped
/// answers `NoOwner`, which is the fallback rather than the normal path.
fn spawn_owner(
    app: &TestApp,
    fixture: &Fixture,
    container_id: &ContainerId,
    state: AppState,
) -> JoinHandle<()> {
    let commands =
        app.session_registry()
            .register(fixture.session_id, fixture.kind, Phase::Running);

    SessionOwner::spawn(OwnerContext {
        session_id: fixture.session_id,
        kind: fixture.kind,
        dirs: fixture.dirs.clone(),
        backend: Arc::new(ClaudeBackend::new()) as Arc<dyn AgentBackend>,
        start_offset: 0,
        translate: TranslateConfig::default(),
        adopted: false,
        stdin: None,
        container_id: Some(container_id.clone()),
        commands,
        state,
    })
}

/// A state whose end-of-session hook records the sessions it is told about, and
/// the record.
fn state_with_hook(app: &TestApp) -> (AppState, Arc<Mutex<Vec<Uuid>>>) {
    let ended: Arc<Mutex<Vec<Uuid>>> = Arc::default();
    let state = app.state.clone().with_session_ended_hook({
        let ended = Arc::clone(&ended);
        Arc::new(move |session_id| {
            let ended = Arc::clone(&ended);
            async move {
                ended
                    .lock()
                    .expect("the hook lock is healthy")
                    .push(session_id);
            }
            .boxed()
        })
    });

    (state, ended)
}

/// The sessions the hook was told about.
fn ended(record: &Arc<Mutex<Vec<Uuid>>>) -> Vec<Uuid> {
    record.lock().expect("the hook lock is healthy").clone()
}

/// The session row's whole picture, in one read.
async fn lifecycle(app: &TestApp, session_id: Uuid) -> (String, Option<String>, Option<String>) {
    sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
        "SELECT state::text, container_id, error FROM sessions WHERE id = $1",
    )
    .bind(session_id)
    .fetch_one(&app.pool)
    .await
    .expect("the session is readable")
}

/// The payload of this session's last `state_change`.
async fn last_state_change(app: &TestApp, session_id: Uuid) -> Value {
    sqlx::query_as::<_, (Value,)>(
        "SELECT payload FROM events
         WHERE session_id = $1 AND kind = 'state_change' ORDER BY seq DESC LIMIT 1",
    )
    .bind(session_id)
    .fetch_one(&app.pool)
    .await
    .expect("a state_change is stored")
    .0
}

/// Wait until the container has been sent `signal`, or fail saying what it got.
async fn wait_for_signal(app: &TestApp, container_id: &ContainerId, signal: Signal) {
    let deadline = tokio::time::Instant::now() + WITHIN;
    loop {
        let signals = app.engine().signals(container_id);
        if signals.contains(&signal) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the container was sent {signals:?}, never {signal}",
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Wait for an owner that ends by itself to leave its loop.
async fn owner_ended(handle: JoinHandle<()>) {
    tokio::time::timeout(WITHIN, handle)
        .await
        .expect("the owner leaves its loop")
        .expect("the owner task did not panic");
}

/// A conversational session that has gone quiet past its profile's timeout is
/// asked to stop, and the exit that follows parks it with `idle timeout`.
#[tokio::test]
async fn an_idle_conversational_session_is_parked() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app, ProfileKind::Conversational, 60).await;
    mark_running(&app, &fixture).await;
    let container = start_container(&app, &fixture).await;
    let owner = spawn_owner(&app, &fixture, &container, app.state.clone());

    let base = base();
    set_last_activity(&app, fixture.session_id, base).await;

    let report = app
        .cron()
        .idle_reaper(base + TimeDelta::seconds(61))
        .await
        .expect("the idle reaper runs");
    assert_eq!(
        report,
        JobReport {
            items: 1,
            skipped: 0,
            failures: 0
        },
    );

    wait_for_signal(&app, &container, Signal::Sigint).await;

    // The recorded behaviour of a SIGINT-ed CLI: it ends its turn and exits 0.
    assert!(app.engine().exit(&container, 0));
    owner_ended(owner).await;

    let (state, container_column, error) = lifecycle(&app, fixture.session_id).await;
    assert_eq!(state, "parked");
    assert_eq!(container_column, None, "the container id was not cleared");
    assert_eq!(error, None, "an idle session failed");

    let change = last_state_change(&app, fixture.session_id).await;
    assert_eq!(change["from"], "running");
    assert_eq!(change["to"], "parked");
    assert_eq!(change["reason"], "idle timeout");
    assert_eq!(change["signal"], "SIGINT");
}

/// Each session is judged against its own profile's timeout: a shorter one
/// elsewhere means nothing to it (`docs/data-model.md`,
/// `agent_profiles.idle_timeout_secs`).
#[tokio::test]
async fn a_session_inside_its_own_profiles_timeout_is_left_alone() {
    let app = TestApp::spawn().await;
    let impatient = fixture(&app, ProfileKind::Conversational, 60).await;
    let patient = fixture(&app, ProfileKind::Conversational, 3600).await;

    let base = base();
    for fixture in [&impatient, &patient] {
        mark_running(&app, fixture).await;
        set_last_activity(&app, fixture.session_id, base).await;
    }
    let impatient_container = start_container(&app, &impatient).await;
    let patient_container = start_container(&app, &patient).await;
    let impatient_owner = spawn_owner(&app, &impatient, &impatient_container, app.state.clone());
    let patient_owner = spawn_owner(&app, &patient, &patient_container, app.state.clone());

    let report = app
        .cron()
        .idle_reaper(base + TimeDelta::seconds(61))
        .await
        .expect("the idle reaper runs");
    assert_eq!(report.items, 1, "both sessions were reaped: {report:?}");
    assert_eq!(report.skipped, 0);
    assert_eq!(report.failures, 0);

    wait_for_signal(&app, &impatient_container, Signal::Sigint).await;
    assert!(
        app.engine().signals(&patient_container).is_empty(),
        "the session inside its own timeout was signalled",
    );
    assert_eq!(
        lifecycle(&app, patient.session_id).await.0,
        "running",
        "the session inside its own timeout left running",
    );

    assert!(app.engine().exit(&impatient_container, 0));
    owner_ended(impatient_owner).await;
    patient_owner.abort();
}

/// An ephemeral session that goes quiet is stalled, not parked: it ends
/// `failed` with `sessions.error = "stalled"`, and the end-of-session hook
/// releases what it held (ADR 0003; `ARCHITECTURE.md`, "Task tracker").
#[tokio::test]
async fn an_idle_ephemeral_session_is_stalled() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app, ProfileKind::Ephemeral, 3600).await;
    mark_running(&app, &fixture).await;
    let container = start_container(&app, &fixture).await;
    let (state, record) = state_with_hook(&app);
    let owner = spawn_owner(&app, &fixture, &container, state.clone());

    let base = base();
    set_last_activity(&app, fixture.session_id, base).await;

    let report = CronService::new(state)
        .idle_reaper(base + TimeDelta::seconds(3601))
        .await
        .expect("the idle reaper runs");
    assert_eq!(report.items, 1);

    wait_for_signal(&app, &container, Signal::Sigint).await;
    assert!(app.engine().exit(&container, 130));
    owner_ended(owner).await;

    let (state_name, container_column, error) = lifecycle(&app, fixture.session_id).await;
    assert_eq!(state_name, "failed", "an ephemeral session was parked");
    assert_eq!(container_column, None);
    assert_eq!(error.as_deref(), Some("stalled"));

    let change = last_state_change(&app, fixture.session_id).await;
    assert_eq!(change["to"], "failed");
    assert_eq!(change["reason"], "stalled");

    assert_eq!(
        ended(&record),
        vec![fixture.session_id],
        "the end-of-session hook did not run for a stalled session",
    );
}

/// A parked session is alive by definition — it is waiting for a person — so
/// however old its last event is, the reaper never selects it
/// (`ARCHITECTURE.md`, "Task tracker", "Liveness comes from the session").
#[tokio::test]
async fn a_parked_session_is_never_selected() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app, ProfileKind::Conversational, 60).await;
    mark_running(&app, &fixture).await;

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .transition(
            &mut tx,
            fixture.session_id,
            &Transition::new(SessionState::Running, SessionState::Parked, "CLI exited"),
        )
        .await
        .expect("the session parks");
    tx.commit().await.expect("the transaction commits");

    let base = base();
    set_last_activity(&app, fixture.session_id, base).await;

    let report = app
        .cron()
        .idle_reaper(base + TimeDelta::days(1))
        .await
        .expect("the idle reaper runs");
    assert_eq!(report, JobReport::default(), "a parked session was reaped");
    assert_eq!(lifecycle(&app, fixture.session_id).await.0, "parked");
}

/// Idle is measured from `last_activity_at`, which every appended event
/// advances — a `user_message` included — so a message to a running session
/// resets its clock (`docs/data-model.md`, `sessions.last_activity_at`).
#[tokio::test]
async fn a_user_message_resets_the_idle_clock() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app, ProfileKind::Conversational, 60).await;
    mark_running(&app, &fixture).await;
    let container = start_container(&app, &fixture).await;
    let owner = spawn_owner(&app, &fixture, &container, app.state.clone());

    let base = base();
    set_last_activity(&app, fixture.session_id, base).await;

    // The message lands now, an hour past `base`, which is what the tick below
    // is a minute past.
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .append_event(
            &mut tx,
            fixture.session_id,
            &AgentEvent::new(AgentEventBody::UserMessage {
                text: "still here".to_string(),
                user_id: None,
                client_id: None,
            }),
        )
        .await
        .expect("the user message appends");
    tx.commit().await.expect("the transaction commits");

    let report = app
        .cron()
        .idle_reaper(base + TimeDelta::seconds(61))
        .await
        .expect("the idle reaper runs");
    assert_eq!(
        report,
        JobReport::default(),
        "a session that had just been messaged was reaped",
    );

    tokio::time::sleep(QUIET_FOR).await;
    assert!(
        app.engine().signals(&container).is_empty(),
        "a session that had just been messaged was signalled",
    );
    assert_eq!(lifecycle(&app, fixture.session_id).await.0, "running");

    owner.abort();
}

/// A second tick while the first stop is still running its course sends
/// nothing: the registry answers `AlreadyStopping` and the session is skipped
/// until the escalation is due (`ARCHITECTURE.md`, "Stop semantics").
#[tokio::test]
async fn a_second_tick_while_a_stop_is_pending_is_skipped() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app, ProfileKind::Conversational, 60).await;
    mark_running(&app, &fixture).await;
    let container = start_container(&app, &fixture).await;
    let owner = spawn_owner(&app, &fixture, &container, app.state.clone());

    let base = base();
    set_last_activity(&app, fixture.session_id, base).await;
    let now = base + TimeDelta::seconds(61);

    assert_eq!(
        app.cron()
            .idle_reaper(now)
            .await
            .expect("the first tick runs")
            .items,
        1,
    );
    wait_for_signal(&app, &container, Signal::Sigint).await;

    let report = app
        .cron()
        .idle_reaper(now)
        .await
        .expect("the second tick runs");
    assert_eq!(
        report,
        JobReport {
            items: 0,
            skipped: 1,
            failures: 0
        },
    );
    assert_eq!(
        app.engine()
            .signals(&container)
            .iter()
            .filter(|signal| **signal == Signal::Sigint)
            .count(),
        1,
        "the second tick sent a second SIGINT",
    );

    assert!(app.engine().exit(&container, 0));
    owner_ended(owner).await;
}

/// A CLI that survives `SIGINT` and `SIGTERM` is sent `SIGKILL` on a later
/// tick, once its stop has outlived the whole documented sequence
/// (`ARCHITECTURE.md`, "Background jobs", the idle reaper row).
#[tokio::test]
async fn a_stop_the_cli_ignores_is_escalated_to_sigkill() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app, ProfileKind::Conversational, 60).await;
    mark_running(&app, &fixture).await;
    let container = start_container(&app, &fixture).await;
    let owner = spawn_owner(&app, &fixture, &container, app.state.clone());

    let base = base();
    set_last_activity(&app, fixture.session_id, base).await;
    let now = base + TimeDelta::seconds(61);

    app.cron()
        .idle_reaper(now)
        .await
        .expect("the first tick runs");
    // The mock container's `claude` traps nothing, so both signals are recorded
    // and neither ends it: exactly the case SIGKILL exists for.
    wait_for_signal(&app, &container, Signal::Sigterm).await;

    // `STOP_GRACE_SECS` is 1 in tests, so the escalation is due 62 s after the
    // stop was taken; the registry's `since` is aged rather than waited out.
    assert!(
        app.session_registry().age_stop(
            fixture.session_id,
            sigkill_after(&app.state.config) + WITHIN
        ),
        "the pending stop could not be aged",
    );

    let report = app
        .cron()
        .idle_reaper(now)
        .await
        .expect("the escalating tick runs");
    assert_eq!(
        report,
        JobReport {
            items: 1,
            skipped: 0,
            failures: 0
        },
    );
    assert_eq!(
        app.engine().signals(&container),
        vec![Signal::Sigint, Signal::Sigterm, Signal::Sigkill],
    );

    assert!(app.engine().exit(&container, 137));
    owner_ended(owner).await;
    assert_eq!(lifecycle(&app, fixture.session_id).await.0, "parked");
}

/// A `running` row with no owner has nobody to ask, so the reaper writes the
/// transition itself and removes the container behind it.
#[tokio::test]
async fn a_running_session_with_no_owner_is_parked_by_the_fallback() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app, ProfileKind::Conversational, 60).await;
    mark_running(&app, &fixture).await;
    let container = start_container(&app, &fixture).await;

    let base = base();
    set_last_activity(&app, fixture.session_id, base).await;

    let report = app
        .cron()
        .idle_reaper(base + TimeDelta::seconds(61))
        .await
        .expect("the idle reaper runs");
    assert_eq!(report.items, 1);
    assert_eq!(report.failures, 0);

    let (state, container_column, error) = lifecycle(&app, fixture.session_id).await;
    assert_eq!(state, "parked");
    assert_eq!(container_column, None, "the container id was not cleared");
    assert_eq!(error, None);

    let change = last_state_change(&app, fixture.session_id).await;
    assert_eq!(change["to"], "parked");
    assert_eq!(change["reason"], "idle timeout");
    assert!(change.get("signal").is_none(), "{change}");

    // The signals a removed container was sent go with it, so the removal is
    // the assertable half of the fallback's clean-up.
    assert_eq!(
        app.engine().state_of(&container),
        None,
        "the container was not removed",
    );
}

/// The same fallback for an ephemeral session ends it `failed` with `stalled`
/// and runs the end-of-session hook (ADR 0003).
#[tokio::test]
async fn an_ephemeral_session_with_no_owner_is_failed_as_stalled() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app, ProfileKind::Ephemeral, 60).await;
    mark_running(&app, &fixture).await;
    let container = start_container(&app, &fixture).await;
    let (state, record) = state_with_hook(&app);

    let base = base();
    set_last_activity(&app, fixture.session_id, base).await;

    let report = CronService::new(state)
        .idle_reaper(base + TimeDelta::seconds(61))
        .await
        .expect("the idle reaper runs");
    assert_eq!(report.items, 1);
    assert_eq!(report.failures, 0);

    let (state_name, container_column, error) = lifecycle(&app, fixture.session_id).await;
    assert_eq!(state_name, "failed");
    assert_eq!(container_column, None);
    assert_eq!(error.as_deref(), Some("stalled"));

    let change = last_state_change(&app, fixture.session_id).await;
    assert_eq!(change["to"], "failed");
    assert_eq!(change["reason"], "stalled");

    assert_eq!(
        app.engine().state_of(&container),
        None,
        "the container was not removed",
    );
    assert_eq!(ended(&record), vec![fixture.session_id]);
}
