//! Startup recovery (`ARCHITECTURE.md`, "Restart procedure", "Durability and
//! recovery"; `CLAUDE.md`, "Testing expectations").
//!
//! Every scenario here is a restart: rows and a transcript survive, the registry
//! does not, and `TestApp::recover()` runs the very function `main.rs` calls over
//! what is left. The container engine is `MockEngine`, seeded by creating and
//! starting containers with the session label the way the launcher does, because
//! a label, a state and an exit code are the whole of what recovery reads from an
//! engine.
//!
//! What the assertions are about: that an adopted session keeps tailing where the
//! database said it stopped and republishes nothing, that its MCP token and
//! `mcp.json` are untouched because the process behind them is still running
//! (ADR 0029), that a session whose container is gone is parked with the reason
//! the document names, and that a session caught in `creating` is failed, its
//! container removed and its task lease released.
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::FutureExt;
use mars_orchestrator::agent::{AgentBackend, CLAUDE_CLI_VERSION, ClaudeBackend, TranslateConfig};
use mars_orchestrator::engine::{
    ContainerEngine, ContainerId, ContainerInfo, ContainerSpec, ContainerSummary, EngineError,
    EngineKind, ExecSession, ExitStatus, LABEL_PROJECT_ID, LABEL_SESSION_ID, Signal, StdinWriter,
};
use mars_orchestrator::models::{NewSession, ProfileKind, SessionState};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{SessionRepository, Transition};
use mars_orchestrator::session::{
    CREATING_REASON, MISSING_CONTAINER_REASON, OwnerContext, Phase, RecoveryReport, SessionDirs,
    SessionOwner, initial_token, write_mcp_json,
};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;
use uuid::Uuid;

use common::TestApp;

/// Not a credential: an obviously fake stand-in for the seeded user's Argon2id
/// PHC string (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// How long a committed line is waited for before the test fails. Generous: the
/// owner polls every 100 ms and a loaded machine runs several test binaries.
const COMMIT_WITHIN: Duration = Duration::from_secs(10);

/// How long "nothing happened" is given to be proved wrong: several tail ticks.
const QUIET_FOR: Duration = Duration::from_millis(600);

/// One session, ready to be recovered.
struct Fixture {
    session_id: Uuid,
    project_id: Uuid,
    dirs: SessionDirs,
}

/// Seed a project, a profile and one session of `kind`, and create its
/// directories.
///
/// Seeded with unchecked statements for the foreign keys only, as the other
/// repository suites do.
async fn fixture_of_kind(app: &TestApp, kind: ProfileKind) -> Fixture {
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
        "INSERT INTO agent_profiles (id, project_id, name, image, kind, partial_messages)
         VALUES ($1, $2, 'default', $3, $4, FALSE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind("localhost/mars-session:test")
    .bind(kind)
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

    let repository = SessionRepository::new(&app.pool);
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let session = repository
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
        dirs,
    }
}

/// A conversational session, which is what every scenario but the ephemeral one
/// uses.
async fn fixture(app: &TestApp) -> Fixture {
    fixture_of_kind(app, ProfileKind::Conversational).await
}

/// Move the session to `running` the way the launcher does when it attaches
/// stdin (ADR 0032), writing the `state_change` recovery reads the process
/// boundary from.
async fn mark_running(app: &TestApp, session_id: Uuid) {
    transition(
        app,
        session_id,
        Transition::new(
            SessionState::Creating,
            SessionState::Running,
            "container started, stdin attached",
        ),
    )
    .await;
}

/// Write one transition the way its owner would.
async fn transition(app: &TestApp, session_id: Uuid, change: Transition<'_>) {
    let repository = SessionRepository::new(&app.pool);
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    repository
        .transition(&mut tx, session_id, &change)
        .await
        .expect("the session transitions");
    tx.commit().await.expect("the transaction commits");
}

/// Create and start a labelled container for this session, the way the launcher
/// does, and record its id on the row.
///
/// `cmd` is the session's own `claude`, which traps nothing, so only
/// [`MockEngine::exit`] ends it (`ARCHITECTURE.md`, "Engine adapter", Normalised
/// semantics).
async fn start_container(app: &TestApp, fixture: &Fixture) -> ContainerId {
    let container_id = create_container(app, fixture, &fixture.session_id.to_string()).await;
    app.engine()
        .start(&container_id)
        .await
        .expect("the container starts");
    record_container(app, fixture.session_id, Some(&container_id)).await;

    container_id
}

/// Create a container carrying `label` as its `mars.session_id`, without
/// starting it.
async fn create_container(app: &TestApp, fixture: &Fixture, label: &str) -> ContainerId {
    let spec = ContainerSpec {
        image: "mars-session-stub:test".to_string(),
        name: format!("mars-session-{}", Uuid::new_v4()),
        labels: BTreeMap::from([
            (LABEL_SESSION_ID.to_string(), label.to_string()),
            (LABEL_PROJECT_ID.to_string(), fixture.project_id.to_string()),
        ]),
        user: "1000:1000".to_string(),
        working_dir: "/session/work".to_string(),
        cmd: vec!["claude".to_string()],
        env: Vec::new(),
        binds: Vec::new(),
        network: "mars-sessions".to_string(),
        extra_hosts: Vec::new(),
        runtime: None,
        open_stdin: true,
    };

    app.engine()
        .create(&spec)
        .await
        .expect("the container is created")
}

/// Set or clear `sessions.container_id`.
async fn record_container(app: &TestApp, session_id: Uuid, container_id: Option<&ContainerId>) {
    let repository = SessionRepository::new(&app.pool);
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    repository
        .set_container_id(&mut tx, session_id, container_id.map(|id| id.0.as_str()))
        .await
        .expect("the container id is recorded");
    tx.commit().await.expect("the transaction commits");
}

/// Write the `mcp.json` a launch would have left behind, so a test can prove
/// adoption did not touch it (ADR 0029).
async fn write_config(app: &TestApp, fixture: &Fixture) -> Vec<u8> {
    write_mcp_json(&fixture.dirs, &app.state.config.mcp_url, &initial_token())
        .await
        .expect("the mcp config is written");

    tokio::fs::read(fixture.dirs.mcp_json())
        .await
        .expect("the mcp config is readable")
}

/// The fixture directory for the pinned CLI version.
fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude")
        .join(CLAUDE_CLI_VERSION)
}

/// The native lines of one recorded scenario.
fn native_lines(scenario: &str) -> Vec<String> {
    let path = fixture_dir().join(format!("{scenario}.jsonl"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{} is readable: {err}", path.display()))
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// The kinds one uninterrupted pass over `scenario` must end up with: the
/// launch's `state_change`, then the fixture's own expected events, with the
/// owner's MCP `launch_warning` spliced in after the `init` it belongs to.
fn expected_kinds(scenario: &str) -> Vec<String> {
    let path = fixture_dir().join(format!("{scenario}.expected.json"));
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(&path).expect("the expected file is readable"),
    )
    .expect("the expected file is JSON");

    let mut kinds = vec!["state_change".to_string()];
    for event in expected["events"]
        .as_array()
        .expect("the expected file has events")
    {
        let kind = event["kind"]
            .as_str()
            .expect("every expected event has a kind")
            .to_string();
        let is_init = kind == "init";
        kinds.push(kind);
        if is_init {
            // Every recorded run reported the server as `failed`.
            kinds.push("launch_warning".to_string());
        }
    }

    kinds
}

/// Append one complete line to the transcript and answer its new length, which
/// is the offset the owner must commit for it.
async fn append_line(dirs: &SessionDirs, line: &str) -> u64 {
    let path = dirs.stream_jsonl();
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .expect("the transcript is writable");
    file.write_all(format!("{line}\n").as_bytes())
        .await
        .expect("the bytes are written");
    file.flush().await.expect("the bytes are flushed");

    tokio::fs::metadata(&path)
        .await
        .expect("the transcript is there")
        .len()
}

/// `COUNT(*)`, `MAX(seq)` and `MAX(_offset)` in one read.
async fn snapshot(pool: &PgPool, session_id: Uuid) -> (i64, i64, Option<i64>) {
    sqlx::query_as::<_, (i64, i64, Option<i64>)>(
        "SELECT COUNT(*), COALESCE(MAX(seq), 0), MAX((payload->>'_offset')::bigint)
         FROM events WHERE session_id = $1",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
    .expect("the events are readable")
}

/// Every event's `(seq, kind, _offset)`, in sequence order.
async fn rows(pool: &PgPool, session_id: Uuid) -> Vec<(i64, String, Option<i64>)> {
    sqlx::query_as::<_, (i64, String, Option<i64>)>(
        "SELECT seq, kind, (payload->>'_offset')::bigint
         FROM events WHERE session_id = $1 ORDER BY seq",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .expect("the events are readable")
}

/// The kinds of a session's events, in order.
async fn kinds(pool: &PgPool, session_id: Uuid) -> Vec<String> {
    rows(pool, session_id)
        .await
        .into_iter()
        .map(|(_, kind, _)| kind)
        .collect()
}

/// `state`, `container_id`, `error` and the MCP token hash of one session.
async fn session_row(
    pool: &PgPool,
    session_id: Uuid,
) -> (String, Option<String>, Option<String>, String) {
    sqlx::query_as::<_, (String, Option<String>, Option<String>, String)>(
        "SELECT state::text, container_id, error, mcp_token_hash FROM sessions WHERE id = $1",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
    .expect("the session is readable")
}

/// The payload of the session's last event of one kind.
async fn last_event(pool: &PgPool, session_id: Uuid, kind: &str) -> Value {
    sqlx::query_as::<_, (Value,)>(
        "SELECT payload FROM events
         WHERE session_id = $1 AND kind = $2 ORDER BY seq DESC LIMIT 1",
    )
    .bind(session_id)
    .bind(kind)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|err| panic!("a {kind} event is stored: {err}"))
    .0
}

/// Wait until the session has `expected` events, or fail.
async fn wait_for_events(pool: &PgPool, session_id: Uuid, expected: i64) {
    let deadline = tokio::time::Instant::now() + COMMIT_WITHIN;
    loop {
        let (count, _, _) = snapshot(pool, session_id).await;
        if count >= expected {
            assert_eq!(count, expected, "the owner committed more than it should");
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "only {count} of {expected} events were committed",
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Wait until the session's highest committed offset is `expected`, or fail.
async fn wait_for_offset(pool: &PgPool, session_id: Uuid, expected: u64) {
    let deadline = tokio::time::Instant::now() + COMMIT_WITHIN;
    loop {
        let (_, _, offset) = snapshot(pool, session_id).await;
        if offset == Some(expected as i64) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the committed offset is {offset:?}, not {expected}",
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Wait until the session's state is `expected`, or fail.
async fn wait_for_state(pool: &PgPool, session_id: Uuid, expected: &str) {
    let deadline = tokio::time::Instant::now() + COMMIT_WITHIN;
    loop {
        let (state, _, _, _) = session_row(pool, session_id).await;
        if state == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session is {state}, not {expected}",
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Spawn the owner a launch would have spawned, to commit the first part of a
/// transcript before the restart being simulated.
///
/// `adopted: false`, because this is the process's own owner; the owner recovery
/// creates is the adopting one.
fn spawn_launched_owner(app: &TestApp, fixture: &Fixture, container_id: &ContainerId) -> Launched {
    let commands = app.session_registry().register(
        fixture.session_id,
        ProfileKind::Conversational,
        Phase::Running,
    );

    let handle = SessionOwner::spawn(OwnerContext {
        session_id: fixture.session_id,
        kind: ProfileKind::Conversational,
        dirs: fixture.dirs.clone(),
        backend: Arc::new(ClaudeBackend::new()) as Arc<dyn AgentBackend>,
        start_offset: 0,
        translate: TranslateConfig::default(),
        adopted: false,
        stdin: None,
        container_id: Some(container_id.clone()),
        commands,
        state: app.state.clone(),
    });

    Launched { handle }
}

/// The owner spawned before a simulated crash.
struct Launched {
    handle: JoinHandle<()>,
}

impl Launched {
    /// Kill the task where it stands, the way a crashed orchestrator would.
    async fn crash(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

/// Run recovery over a state of the test's own — the way `TestApp::recover` does,
/// with the registry emptied first — so a test can install an end-of-session
/// hook or a different engine.
async fn recover_with(state: &AppState) -> RecoveryReport {
    state.session_registry.clear();

    mars_orchestrator::session::recover(state)
        .await
        .expect("startup recovery runs")
}

/// A `running` session whose container is still running is adopted: stdin is
/// reattached, tailing resumes at `MAX(_offset)`, the MCP token and `mcp.json`
/// are untouched and nothing already committed is committed again
/// (`ARCHITECTURE.md`, "Restart procedure", step 2; ADR 0029).
///
/// The crash is placed between a subagent call and its result, so the adopted
/// owner also has to have reconstructed the open call: a `subagent_end` is
/// emitted exactly once, by an owner that never saw the `subagent_start`.
#[tokio::test]
async fn a_live_container_is_adopted_and_keeps_its_token_and_config() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let container = start_container(&app, &fixture).await;
    let config_before = write_config(&app, &fixture).await;
    let (_, _, _, hash_before) = session_row(&app.pool, fixture.session_id).await;

    // The process's own owner commits the first five lines: the `init`, its
    // warning, the `Agent` tool call and the `subagent_start`, with the
    // subagent's own result still to come.
    let lines = native_lines("subagent");
    let launched = spawn_launched_owner(&app, &fixture, &container);
    let mut length = 0;
    for line in &lines[..5] {
        length = append_line(&fixture.dirs, line).await;
    }
    wait_for_offset(&app.pool, fixture.session_id, length).await;
    let (before, _, _) = snapshot(&app.pool, fixture.session_id).await;
    assert!(before >= 5, "only {before} events were committed");
    assert!(
        kinds(&app.pool, fixture.session_id)
            .await
            .contains(&"subagent_start".to_string()),
        "the subagent call was not committed before the crash",
    );
    launched.crash().await;

    let report = app.recover().await;
    assert_eq!(
        report,
        RecoveryReport {
            adopted: 1,
            parked: 0,
            failed: 0
        },
    );

    // A live owner, already past `init`: an adopted process has nothing to wait
    // for (ADR 0020).
    assert!(app.session_registry().is_live(fixture.session_id));
    assert_eq!(
        app.session_registry().phase(fixture.session_id),
        Some(Phase::Running),
    );

    // The replay of the committed region publishes nothing at all.
    tokio::time::sleep(QUIET_FOR).await;
    let (after_adoption, _, _) = snapshot(&app.pool, fixture.session_id).await;
    assert_eq!(
        after_adoption, before,
        "adoption replayed committed history into the events table",
    );

    // Adoption writes no configuration: the running CLI's token is still its
    // token.
    let (state, container_column, _, hash_after) = session_row(&app.pool, fixture.session_id).await;
    assert_eq!(state, "running");
    assert_eq!(container_column.as_deref(), Some(container.0.as_str()));
    assert_eq!(hash_after, hash_before, "the MCP token hash was rewritten");
    assert_eq!(
        tokio::fs::read(fixture.dirs.mcp_json())
            .await
            .expect("the mcp config is readable"),
        config_before,
        "mcp.json was rewritten under a running CLI",
    );

    for line in &lines[5..] {
        length = append_line(&fixture.dirs, line).await;
    }
    let expected = expected_kinds("subagent");
    wait_for_events(&app.pool, fixture.session_id, expected.len() as i64).await;
    wait_for_offset(&app.pool, fixture.session_id, length).await;

    let rows = rows(&app.pool, fixture.session_id).await;
    assert_eq!(
        rows.iter().map(|(seq, _, _)| *seq).collect::<Vec<_>>(),
        (1..=rows.len() as i64).collect::<Vec<_>>(),
        "a gap or a duplicate sequence",
    );
    let offsets: Vec<i64> = rows.iter().filter_map(|(_, _, offset)| *offset).collect();
    assert_eq!(
        offsets.iter().copied().collect::<BTreeSet<_>>().len(),
        offsets.len(),
        "two events carry the same transcript offset",
    );
    assert!(
        offsets.windows(2).all(|pair| pair[0] < pair[1]),
        "the committed offsets are not increasing: {offsets:?}",
    );

    let kinds = kinds(&app.pool, fixture.session_id).await;
    assert_eq!(kinds, expected, "the adopted owner's history differs");
    assert_eq!(
        kinds.iter().filter(|kind| *kind == "subagent_end").count(),
        1,
        "the delayed subagent result did not end the reconstructed call once",
    );

    app.session_registry().remove(fixture.session_id);
}

/// A `running` session the engine lists no container for is parked with the
/// documented reason and forgets its container id (`ARCHITECTURE.md`, "Restart
/// procedure", step 2).
#[tokio::test]
async fn a_running_session_with_no_container_is_parked() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    // The row still names the container the last process had; the engine has
    // nothing by that name any more.
    record_container(
        &app,
        fixture.session_id,
        Some(&ContainerId("mock-gone".into())),
    )
    .await;

    let report = app.recover().await;
    assert_eq!(
        report,
        RecoveryReport {
            adopted: 0,
            parked: 1,
            failed: 0
        },
    );

    let (state, container_column, error, _) = session_row(&app.pool, fixture.session_id).await;
    assert_eq!(state, "parked");
    assert_eq!(container_column, None, "the container id was not cleared");
    assert_eq!(error, None, "a parked session carries an error");

    let change = last_event(&app.pool, fixture.session_id, "state_change").await;
    assert_eq!(change["from"], "running");
    assert_eq!(change["to"], "parked");
    assert_eq!(change["reason"], MISSING_CONTAINER_REASON);
    assert!(change.get("signal").is_none(), "{change}");

    // Nothing is tailing it: a parked session has no owner.
    assert!(!app.session_registry().is_live(fixture.session_id));
}

/// An ephemeral session is never parked, so a restart that finds its container
/// gone fails it with the same reason (ADR 0003; `ARCHITECTURE.md`, "Restart
/// procedure").
#[tokio::test]
async fn an_ephemeral_session_whose_container_is_gone_fails_instead_of_parking() {
    let app = TestApp::spawn().await;
    let fixture = fixture_of_kind(&app, ProfileKind::Ephemeral).await;
    mark_running(&app, fixture.session_id).await;

    let report = app.recover().await;
    assert_eq!(
        report,
        RecoveryReport {
            adopted: 0,
            parked: 0,
            failed: 1
        },
    );

    let (state, _, error, _) = session_row(&app.pool, fixture.session_id).await;
    assert_eq!(state, "failed");
    assert_eq!(error.as_deref(), Some(MISSING_CONTAINER_REASON));

    let change = last_event(&app.pool, fixture.session_id, "state_change").await;
    assert_eq!(change["to"], "failed");
    assert_eq!(change["reason"], MISSING_CONTAINER_REASON);
}

/// A session caught in `creating` is failed with the documented reason, its
/// labelled container is removed and the end-of-session hook runs, which is what
/// releases a task claimed at launch (`ARCHITECTURE.md`, "Restart procedure",
/// step 3).
#[tokio::test]
async fn a_creating_session_is_failed_and_its_container_removed() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    // Created, never started: the launcher got this far and no further.
    let container = create_container(&app, &fixture, &fixture.session_id.to_string()).await;
    record_container(&app, fixture.session_id, Some(&container)).await;

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

    let report = recover_with(&state).await;
    assert_eq!(
        report,
        RecoveryReport {
            adopted: 0,
            parked: 0,
            failed: 1
        },
    );

    let (session_state, container_column, error, _) =
        session_row(&app.pool, fixture.session_id).await;
    assert_eq!(session_state, "failed");
    assert_eq!(error.as_deref(), Some(CREATING_REASON));
    assert_eq!(container_column, None, "the container id was not cleared");

    let change = last_event(&app.pool, fixture.session_id, "state_change").await;
    assert_eq!(change["from"], "creating");
    assert_eq!(change["to"], "failed");
    assert_eq!(change["reason"], CREATING_REASON);

    assert_eq!(
        app.engine().state_of(&container),
        None,
        "the half-built container was not removed",
    );
    assert_eq!(
        *ended.lock().expect("the hook lock is healthy"),
        vec![fixture.session_id],
        "the end-of-session hook did not run for the failed session",
    );
}

/// A session that is already `parked` is not touched, whatever the engine lists
/// for it: collecting its stray container is the orphan-cleanup job's work
/// (`ARCHITECTURE.md`, "Background jobs").
#[tokio::test]
async fn a_parked_session_with_a_stray_container_is_left_alone() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    transition(
        &app,
        fixture.session_id,
        Transition::new(
            SessionState::Running,
            SessionState::Parked,
            "stopped by user",
        ),
    )
    .await;
    let container = start_container(&app, &fixture).await;
    let (_, _, _, hash_before) = session_row(&app.pool, fixture.session_id).await;
    let events_before = snapshot(&app.pool, fixture.session_id).await;

    let report = app.recover().await;
    assert_eq!(report, RecoveryReport::default());

    let (state, container_column, _, hash_after) = session_row(&app.pool, fixture.session_id).await;
    assert_eq!(state, "parked");
    assert_eq!(
        container_column.as_deref(),
        Some(container.0.as_str()),
        "a parked session's row was rewritten",
    );
    assert_eq!(hash_after, hash_before);
    assert_eq!(
        snapshot(&app.pool, fixture.session_id).await,
        events_before,
        "an untouched session gained an event",
    );
    assert!(
        app.engine().state_of(&container).is_some(),
        "recovery removed a stray container instead of leaving it to the orphan job",
    );
    assert!(!app.session_registry().is_live(fixture.session_id));
}

/// A container that has already exited is adopted too, and the owner's exit path
/// is what reads it: the transcript is drained and exit 0 parks the session with
/// the lifecycle rule's own reason (`ARCHITECTURE.md`, "Session lifecycle").
#[tokio::test]
async fn an_exited_container_parks_the_session_through_the_exit_rule() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let container = start_container(&app, &fixture).await;
    assert!(app.engine().exit(&container, 0));

    let report = app.recover().await;
    assert_eq!(
        report,
        RecoveryReport {
            adopted: 1,
            parked: 0,
            failed: 0
        },
    );

    wait_for_state(&app.pool, fixture.session_id, "parked").await;
    let (_, container_column, error, _) = session_row(&app.pool, fixture.session_id).await;
    assert_eq!(container_column, None);
    assert_eq!(error, None);

    let change = last_event(&app.pool, fixture.session_id, "state_change").await;
    assert_eq!(change["to"], "parked");
    assert_eq!(change["reason"], "CLI exited");
    assert_eq!(
        app.engine().state_of(&container),
        None,
        "the exited container was not removed",
    );
}

/// Two containers carrying one session's label is a crash between the create and
/// the remove of a resume: the running one is adopted and the other is removed.
#[tokio::test]
async fn a_duplicate_container_is_removed_and_the_running_one_adopted() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let leftover = create_container(&app, &fixture, &fixture.session_id.to_string()).await;
    let live = start_container(&app, &fixture).await;

    let report = app.recover().await;
    assert_eq!(report.adopted, 1);

    assert!(app.session_registry().is_live(fixture.session_id));
    assert_eq!(
        app.engine().state_of(&leftover),
        None,
        "the leftover container was not removed",
    );
    assert!(
        app.engine().state_of(&live).is_some(),
        "the adopted container was removed",
    );
    let (state, container_column, _, _) = session_row(&app.pool, fixture.session_id).await;
    assert_eq!(state, "running");
    assert_eq!(container_column.as_deref(), Some(live.0.as_str()));

    app.session_registry().remove(fixture.session_id);
}

/// A container whose label is not a uuid belongs to nothing recovery can
/// reconcile, so it is skipped rather than removed, and a `running` session with
/// no container of its own is still parked.
#[tokio::test]
async fn an_unparseable_label_is_skipped() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let stray = create_container(&app, &fixture, "not-a-uuid").await;

    let report = app.recover().await;
    assert_eq!(report.parked, 1);

    assert!(
        app.engine().state_of(&stray).is_some(),
        "a container with an unreadable label was removed",
    );
    let (state, _, _, _) = session_row(&app.pool, fixture.session_id).await;
    assert_eq!(state, "parked");
}

/// An engine that cannot reattach one session's stdin parks that session with
/// the reason naming the engine's own message, and the sweep carries on:
/// the other session is still adopted (`ARCHITECTURE.md`, "Restart procedure").
#[tokio::test]
async fn an_attach_failure_parks_only_its_own_session() {
    let app = TestApp::spawn().await;
    let refused = fixture(&app).await;
    mark_running(&app, refused.session_id).await;
    let refused_container = start_container(&app, &refused).await;

    let adopted = fixture(&app).await;
    mark_running(&app, adopted.session_id).await;
    let adopted_container = start_container(&app, &adopted).await;

    let mut state = app.state.clone();
    state.engine = Arc::new(RefusingAttach::new(
        Arc::clone(&app.state.engine),
        refused_container.clone(),
    )) as Arc<dyn ContainerEngine>;

    let report = recover_with(&state).await;
    assert_eq!(
        report,
        RecoveryReport {
            adopted: 1,
            parked: 1,
            failed: 0
        },
    );

    let (state_column, container_column, _, _) = session_row(&app.pool, refused.session_id).await;
    assert_eq!(state_column, "parked");
    assert_eq!(container_column, None);
    let change = last_event(&app.pool, refused.session_id, "state_change").await;
    assert_eq!(
        change["reason"],
        Value::String(format!(
            "could not reattach after restart: {}",
            RefusingAttach::MESSAGE,
        )),
    );
    assert!(!app.session_registry().is_live(refused.session_id));

    let (state_column, container_column, _, _) = session_row(&app.pool, adopted.session_id).await;
    assert_eq!(state_column, "running", "the other session was not adopted");
    assert_eq!(
        container_column.as_deref(),
        Some(adopted_container.0.as_str())
    );
    assert!(app.session_registry().is_live(adopted.session_id));

    app.session_registry().remove(adopted.session_id);
}

/// `MockEngine` with one container whose `attach_stdin` fails, which is the one
/// thing recovery has to survive that the mock has no way to arrange: every
/// other call is the mock's own answer.
///
/// A wrapper here rather than a hook on the mock, because an attach that fails
/// on a container the engine otherwise reports as perfectly running is a
/// property of a broken engine, not of a container Mars created.
type EngineResult<T> = std::result::Result<T, EngineError>;

struct RefusingAttach {
    inner: Arc<dyn ContainerEngine>,
    refuse: ContainerId,
}

impl RefusingAttach {
    /// What the refused attach says, and therefore what the `state_change`
    /// reason ends with.
    const MESSAGE: &'static str = "the container engine has no such object: attach refused";

    fn new(inner: Arc<dyn ContainerEngine>, refuse: ContainerId) -> Self {
        Self { inner, refuse }
    }
}

#[async_trait]
impl ContainerEngine for RefusingAttach {
    fn kind(&self) -> EngineKind {
        self.inner.kind()
    }

    async fn ping(&self) -> EngineResult<()> {
        self.inner.ping().await
    }

    async fn ensure_network(&self, name: &str, internal: bool) -> EngineResult<()> {
        self.inner.ensure_network(name, internal).await
    }

    async fn image_exists(&self, image: &str) -> EngineResult<bool> {
        self.inner.image_exists(image).await
    }

    async fn pull_image(&self, image: &str) -> EngineResult<()> {
        self.inner.pull_image(image).await
    }

    async fn create(&self, spec: &ContainerSpec) -> EngineResult<ContainerId> {
        self.inner.create(spec).await
    }

    async fn connect_network(&self, id: &ContainerId, network: &str) -> EngineResult<()> {
        self.inner.connect_network(id, network).await
    }

    async fn start(&self, id: &ContainerId) -> EngineResult<()> {
        self.inner.start(id).await
    }

    async fn stop(&self, id: &ContainerId, grace_secs: u32) -> EngineResult<()> {
        self.inner.stop(id, grace_secs).await
    }

    async fn kill(&self, id: &ContainerId, signal: Signal) -> EngineResult<()> {
        self.inner.kill(id, signal).await
    }

    async fn remove(&self, id: &ContainerId, force: bool) -> EngineResult<()> {
        self.inner.remove(id, force).await
    }

    async fn inspect(&self, id: &ContainerId) -> EngineResult<ContainerInfo> {
        self.inner.inspect(id).await
    }

    async fn wait(&self, id: &ContainerId) -> EngineResult<ExitStatus> {
        self.inner.wait(id).await
    }

    async fn list_by_label(&self, label_key: &str) -> EngineResult<Vec<ContainerSummary>> {
        self.inner.list_by_label(label_key).await
    }

    async fn attach_stdin(&self, id: &ContainerId) -> EngineResult<Box<dyn StdinWriter>> {
        if *id == self.refuse {
            return Err(EngineError::NotFound("attach refused".to_string()));
        }

        self.inner.attach_stdin(id).await
    }

    async fn exec_pty(
        &self,
        id: &ContainerId,
        cmd: &[String],
        user: &str,
        cols: u16,
        rows: u16,
    ) -> EngineResult<Box<dyn ExecSession>> {
        self.inner.exec_pty(id, cmd, user, cols, rows).await
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
