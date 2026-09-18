//! The session owner (`ARCHITECTURE.md`, "Session owner task", "Session
//! lifecycle", "Stop semantics", "Durability and recovery"; `CLAUDE.md`,
//! "Testing expectations").
//!
//! Every test here drives a real owner task over a real transcript file with
//! the real Claude adapter and the recorded fixtures, because the contract
//! under test is a contract between three things that all have to agree: what
//! the translator makes of a line, what one transaction commits, and what the
//! next owner reads back. A mock of any of them would prove nothing about the
//! interesting failure, which is a restart in the middle of the file. The
//! container is the one thing that *is* mocked, through `MockEngine`'s scripted
//! exits and capturing stdin, because an exit code and a signal are the whole
//! of what the exit and stop rules read.
//!
//! The assertions that matter are the durability ones: `events.seq` is
//! contiguous with no duplicates, every `_offset` is distinct and increasing,
//! and the kinds a restarted owner ends up with are exactly the kinds one
//! uninterrupted pass would have produced. On the exit side they are the
//! lifecycle ones: which state an exit code leads to, what the `state_change`
//! says, and that a session which left `running` has no container, no
//! `container_id` and the right registry entry left behind.
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::FutureExt;
use mars_orchestrator::agent::{AgentBackend, CLAUDE_CLI_VERSION, ClaudeBackend, TranslateConfig};
use mars_orchestrator::engine::{
    ContainerEngine, ContainerId, ContainerSpec, LABEL_PROJECT_ID, LABEL_SESSION_ID, Signal,
};
use mars_orchestrator::events::SessionInput;
use mars_orchestrator::git::testutil::{TestUpstream, test_identity};
use mars_orchestrator::git::{DataPaths, create_work_clone, init_project_repo, resolve_base};
use mars_orchestrator::models::{NewSession, ProfileKind, RemoteUrl, SessionState};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{SessionRepository, Transition};
use mars_orchestrator::session::{
    OwnerCommand, OwnerContext, Phase, QueuedInput, SessionDirs, SessionOwner,
};
use serde_json::Value;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

use common::TestApp;

/// Not a credential: an obviously fake stand-in for the seeded user's Argon2id
/// PHC string (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// How long a committed line is waited for before the test fails. Generous: the
/// owner polls every 100 ms and a loaded machine runs several test binaries.
const COMMIT_WITHIN: Duration = Duration::from_secs(10);

/// How long "nothing is committed" is given to be proved wrong: several tail
/// ticks.
const QUIET_FOR: Duration = Duration::from_millis(600);

/// One session, ready to be owned.
struct Fixture {
    session_id: Uuid,
    project_id: Uuid,
    dirs: SessionDirs,
    user_id: Uuid,
}

/// A running owner and the channel to it.
struct Running {
    handle: JoinHandle<()>,
    commands: mpsc::Sender<OwnerCommand>,
}

impl Running {
    /// Wait for a loop that ends by itself — a container exit, an ephemeral
    /// end-of-run — to return.
    async fn ended(self) {
        tokio::time::timeout(COMMIT_WITHIN, self.handle)
            .await
            .expect("the owner leaves its loop")
            .expect("the owner task did not panic");
    }

    /// Ask the owner to stand down and wait for its loop to return.
    async fn shutdown(self) {
        self.commands
            .send(OwnerCommand::Shutdown)
            .await
            .expect("the owner is listening");
        tokio::time::timeout(COMMIT_WITHIN, self.handle)
            .await
            .expect("the owner leaves its loop")
            .expect("the owner task did not panic");
    }

    /// Kill the task where it stands, the way a crashed orchestrator would.
    async fn abort(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

/// Seed a project, a profile and one conversational session, and create its
/// directories.
///
/// Seeded with unchecked statements for the foreign keys only, as the other
/// repository suites do.
async fn fixture(app: &TestApp) -> Fixture {
    fixture_of_kind(app, ProfileKind::Conversational).await
}

/// The same seed for a session of either kind.
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
        "INSERT INTO agent_profiles (id, project_id, name, image, partial_messages)
         VALUES ($1, $2, 'default', $3, FALSE)",
    )
    .bind(profile_id)
    .bind(project_id)
    .bind("localhost/mars-session:test")
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
        user_id,
    }
}

/// Move the session to `running` the way the launcher does when it attaches
/// stdin (ADR 0032), writing the `state_change` every later assertion counts.
async fn mark_running(app: &TestApp, session_id: Uuid) {
    let repository = SessionRepository::new(&app.pool);
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    repository
        .transition(
            &mut tx,
            session_id,
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

/// Spawn an owner over `fixture`'s transcript with the real Claude adapter.
fn spawn_owner(app: &TestApp, fixture: &Fixture, start_offset: u64, adopted: bool) -> Running {
    Owner::new(app, fixture)
        .start_offset(start_offset)
        .adopted(adopted)
        .spawn()
}

/// One owner to spawn, with everything the launcher would decide for it.
///
/// A builder rather than a widening argument list: most tests want the plain
/// tailing owner and only the exit and stop scenarios want a container, an
/// attached stdin, a registry entry or a hook.
struct Owner<'a> {
    app: &'a TestApp,
    fixture: &'a Fixture,
    kind: ProfileKind,
    start_offset: u64,
    adopted: bool,
    container_id: Option<ContainerId>,
    stdin: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    register: bool,
    state: Option<AppState>,
}

impl<'a> Owner<'a> {
    fn new(app: &'a TestApp, fixture: &'a Fixture) -> Self {
        Self {
            app,
            fixture,
            kind: ProfileKind::Conversational,
            start_offset: 0,
            adopted: false,
            container_id: None,
            stdin: None,
            register: false,
            state: None,
        }
    }

    fn kind(mut self, kind: ProfileKind) -> Self {
        self.kind = kind;
        self
    }

    fn start_offset(mut self, start_offset: u64) -> Self {
        self.start_offset = start_offset;
        self
    }

    fn adopted(mut self, adopted: bool) -> Self {
        self.adopted = adopted;
        self
    }

    fn container(mut self, container_id: &ContainerId) -> Self {
        self.container_id = Some(container_id.clone());
        self
    }

    /// Attach the container's real stdin, so what the owner writes lands where
    /// [`MockEngine::stdin_lines`] reads it back.
    async fn attached(mut self, container_id: &ContainerId) -> Self {
        let writer = self
            .app
            .engine()
            .attach_stdin(container_id)
            .await
            .expect("the mock engine attaches");
        self.stdin = Some(Box::new(writer) as Box<dyn AsyncWrite + Send + Unpin>);
        self
    }

    /// Give the session a registry entry, so a test can assert what the owner
    /// left of it. The owner still takes its commands from this builder's own
    /// channel, which is what keeps `mark_parked` — which drops the registry's
    /// sender — from ending the loop under the assertion.
    fn registered(mut self) -> Self {
        self.register = true;
        self
    }

    /// Run the owner over `state` instead of the test app's own, which is how a
    /// test installs the end-of-session hook.
    fn state(mut self, state: AppState) -> Self {
        self.state = Some(state);
        self
    }

    fn spawn(self) -> Running {
        if self.register {
            self.app.session_registry().register(
                self.fixture.session_id,
                self.kind,
                Phase::Running,
            );
        }

        let (commands, rx) = mpsc::channel(64);
        let handle = SessionOwner::spawn(OwnerContext {
            session_id: self.fixture.session_id,
            kind: self.kind,
            dirs: self.fixture.dirs.clone(),
            backend: Arc::new(ClaudeBackend::new()) as Arc<dyn AgentBackend>,
            start_offset: self.start_offset,
            translate: TranslateConfig::default(),
            adopted: self.adopted,
            stdin: self.stdin,
            container_id: self.container_id,
            commands: rx,
            state: self.state.unwrap_or_else(|| self.app.state.clone()),
        });

        Running { handle, commands }
    }
}

/// Create and start a mock container for this session, the way the launcher
/// does.
///
/// `cmd` is the session's own `claude`, which traps nothing: a `kill` is
/// recorded and delivered, and only [`MockEngine::exit`] ends the container, so
/// a stop test asserts the signals and decides the exit itself
/// (`ARCHITECTURE.md`, "Engine adapter", Normalised semantics).
async fn start_container(app: &TestApp, fixture: &Fixture) -> ContainerId {
    let spec = ContainerSpec {
        image: "mars-session-stub:test".to_string(),
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
        open_stdin: true,
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

    let repository = SessionRepository::new(&app.pool);
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    repository
        .set_container_id(&mut tx, fixture.session_id, Some(&container_id.0))
        .await
        .expect("the container id is recorded");
    tx.commit().await.expect("the transaction commits");

    container_id
}

/// A project repository and a session work clone under the test app's own
/// `DATA_DIR`, so the end-of-run fetch-back has something real to publish
/// (`CLAUDE.md`, "Testing expectations": git is never mocked).
///
/// The upstream is returned because it owns its temporary directory.
async fn prepare_git(app: &TestApp, fixture: &Fixture) -> TestUpstream {
    let upstream = TestUpstream::create().await;
    let paths = DataPaths::from_config(&app.state.config);
    let guard = app.state.git_locks.lock(fixture.project_id).await;

    init_project_repo(
        &guard,
        &paths,
        &RemoteUrl::local_for_tests(&upstream.path),
        None,
        None,
    )
    .await
    .expect("the project repository is initialised");

    let base = resolve_base(&guard, &paths, None, "main")
        .await
        .expect("main resolves");
    create_work_clone(&guard, &paths, fixture.session_id, &base, &test_identity())
        .await
        .expect("the session work clone is created");

    upstream
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

/// Append `bytes` to the transcript verbatim and answer its new length.
async fn append_bytes(dirs: &SessionDirs, bytes: &[u8]) -> u64 {
    let path = dirs.stream_jsonl();
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .await
        .expect("the transcript is writable");
    file.write_all(bytes).await.expect("the bytes are written");
    file.flush().await.expect("the bytes are flushed");

    tokio::fs::metadata(&path)
        .await
        .expect("the transcript is there")
        .len()
}

/// Append one complete line and answer the transcript's new length, which is
/// the offset the owner must commit for it.
async fn append_line(dirs: &SessionDirs, line: &str) -> u64 {
    append_bytes(dirs, format!("{line}\n").as_bytes()).await
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

/// The session's counters and the timestamp the reaper measures idleness from.
async fn counters(pool: &PgPool, session_id: Uuid) -> (f64, i64, i64, DateTime<Utc>) {
    sqlx::query_as::<_, (f64, i64, i64, DateTime<Utc>)>(
        "SELECT cost_usd, input_tokens, output_tokens, last_activity_at
         FROM sessions WHERE id = $1",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
    .expect("the session is readable")
}

/// The session's `cli_session_id` and state.
async fn session_state(pool: &PgPool, session_id: Uuid) -> (Option<String>, String) {
    sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT cli_session_id, state::text FROM sessions WHERE id = $1",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
    .expect("the session is readable")
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

/// The `_offset` column of every event that carries one.
fn offsets(rows: &[(i64, String, Option<i64>)]) -> Vec<i64> {
    rows.iter().filter_map(|(_, _, offset)| *offset).collect()
}

/// The kinds one uninterrupted pass over `scenario` must end up with: the
/// launch's `state_change`, then the fixture's own expected events, with the
/// owner's MCP `launch_warning` spliced in right after the `init` it belongs to.
///
/// Read from the fixture's `.expected.json` rather than written out here, so the
/// restart assertion is measured against the same expectation the translation
/// suite holds the adapter to.
fn expected_kinds(scenario: &str) -> Vec<String> {
    let path = fixture_dir().join(format!("{scenario}.expected.json"));
    let expected: serde_json::Value = serde_json::from_str(
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

/// The `launch_warning` the MCP rule produces for the recorded runs, every one
/// of which reported the server as `failed` (`ARCHITECTURE.md`, "MCP design").
const FAILED_SERVER_WARNING: &str =
    "MCP server mars-orchestrator is not connected (status: failed)";

/// Every complete line commits its events with the offset it ended at, and a
/// line the translator has no event for commits nothing at all.
#[tokio::test]
async fn each_complete_line_commits_its_events_and_its_end_offset() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let owner = spawn_owner(&app, &fixture, 0, false);

    let lines = native_lines("multi_turn");
    let mut committed = 1; // the `state_change` of the launch
    let mut last_offset;

    for line in &lines {
        let length = append_line(&fixture.dirs, line).await;
        // One line at a time, so a commit can only be the answer to this line.
        let before = committed;
        let deadline = tokio::time::Instant::now() + QUIET_FOR;
        loop {
            let (count, _, offset) = snapshot(&app.pool, fixture.session_id).await;
            committed = count;
            last_offset = offset;
            if count > before || tokio::time::Instant::now() > deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        if committed > before {
            // A line that produced events advanced the offset to exactly the
            // file's length, so nothing is re-read after a restart.
            assert_eq!(
                last_offset,
                Some(length as i64),
                "the committed offset is not the end of the line",
            );
        } else {
            // A line the translator produces nothing for — a
            // `rate_limit_event`, or the `init` the CLI repeats each turn —
            // leaves the stored offset where it was.
            assert!(
                last_offset.unwrap_or(0) < length as i64,
                "a line with no events advanced the stored offset",
            );
        }
    }

    let rows = rows(&app.pool, fixture.session_id).await;
    assert_eq!(
        rows.iter().map(|(seq, _, _)| *seq).collect::<Vec<_>>(),
        (1..=rows.len() as i64).collect::<Vec<_>>(),
        "sequences are contiguous from 1",
    );
    // `state_change`, then the fixture's own events, then the MCP warning the
    // `init` earned.
    assert_eq!(
        kinds(&app.pool, fixture.session_id).await,
        [
            "state_change",
            "init",
            "launch_warning",
            "text",
            "result",
            "text",
            "result"
        ],
    );

    owner.shutdown().await;
}

/// A line without its newline is a line the CLI has not finished writing.
#[tokio::test]
async fn a_partial_line_waits_for_its_newline() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let owner = spawn_owner(&app, &fixture, 0, false);

    let line = native_lines("multi_turn").remove(0);
    let (head, tail) = line.split_at(line.len() / 2);

    append_bytes(&fixture.dirs, head.as_bytes()).await;
    tokio::time::sleep(QUIET_FOR).await;
    let (count, _, offset) = snapshot(&app.pool, fixture.session_id).await;
    assert_eq!(count, 1, "only the launch's state_change is stored");
    assert_eq!(offset, None, "a partial line committed an offset");

    let length = append_line(&fixture.dirs, tail).await;
    // The `init` and its MCP warning, in one batch.
    wait_for_events(&app.pool, fixture.session_id, 3).await;
    wait_for_offset(&app.pool, fixture.session_id, length).await;
    assert_eq!(
        kinds(&app.pool, fixture.session_id).await,
        ["state_change", "init", "launch_warning"],
    );

    owner.shutdown().await;
}

/// The restart test: an owner killed mid-file and replaced by one that resumes
/// from the committed offset leaves no gaps and no duplicates, and the subagent
/// call committed before the kill still ends exactly once.
#[tokio::test]
async fn an_owner_killed_mid_file_leaves_no_gaps_and_no_duplicates() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;

    let lines = native_lines("subagent");
    let first = spawn_owner(&app, &fixture, 0, false);

    // Five lines: the `init`, its warning, the `Agent` tool call and the
    // `subagent_start` are committed, and the subagent's own result is still to
    // come.
    let mut length = 0;
    for line in &lines[..5] {
        length = append_line(&fixture.dirs, line).await;
    }
    wait_for_offset(&app.pool, fixture.session_id, length).await;
    let (before, _, _) = snapshot(&app.pool, fixture.session_id).await;
    assert!(
        kinds(&app.pool, fixture.session_id)
            .await
            .contains(&"subagent_start".to_string()),
        "the subagent call was not committed before the kill",
    );

    // Killed where it stands, as a crashed orchestrator would leave it.
    first.abort().await;

    let repository = SessionRepository::new(&app.pool);
    let resume_from = repository
        .max_offset(fixture.session_id)
        .await
        .expect("the offset is readable")
        .unwrap_or(0) as u64;
    assert_eq!(resume_from, length);

    let second = spawn_owner(&app, &fixture, resume_from, true);
    // The replay of the committed region must publish nothing.
    tokio::time::sleep(QUIET_FOR).await;
    let (after_adoption, _, _) = snapshot(&app.pool, fixture.session_id).await;
    assert_eq!(
        after_adoption, before,
        "adoption replayed committed history into the events table",
    );

    for line in &lines[5..] {
        length = append_line(&fixture.dirs, line).await;
    }
    let expected = expected_kinds("subagent");
    wait_for_events(&app.pool, fixture.session_id, expected.len() as i64).await;
    wait_for_offset(&app.pool, fixture.session_id, length).await;

    let rows = rows(&app.pool, fixture.session_id).await;
    let (count, max_seq, _) = snapshot(&app.pool, fixture.session_id).await;
    assert_eq!(count, max_seq, "a gap or a duplicate sequence");
    assert_eq!(
        rows.iter().map(|(seq, _, _)| *seq).collect::<Vec<_>>(),
        (1..=count).collect::<Vec<_>>(),
    );

    let offsets = offsets(&rows);
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
    assert_eq!(kinds, expected, "the restarted owner's history differs");
    assert_eq!(
        kinds.iter().filter(|kind| *kind == "subagent_end").count(),
        1,
        "the delayed subagent result did not end the call exactly once",
    );

    second.shutdown().await;
}

/// The registry path end to end: an input submitted the way the REST handler
/// does is recorded, a `Stop` does not end the loop, and an owner whose channel
/// is gone takes itself out of the registry.
///
/// `OwnerCommand::Shutdown` itself is what every other test here leaves through
/// (`Running::shutdown`); no registry method sends it yet, because the
/// orchestrator shutdown hook is the next task's.
#[tokio::test]
async fn the_registry_path_records_an_input_and_the_owner_deregisters_itself() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;

    // Registered as the launcher would, then owned through the registry's own
    // channel, so the deregistration is observable.
    let rx = app.session_registry().register(
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
        container_id: None,
        commands: rx,
        state: app.state.clone(),
    });

    let lines = native_lines("multi_turn");
    let length = append_line(&fixture.dirs, &lines[0]).await;
    wait_for_offset(&app.pool, fixture.session_id, length).await;
    assert!(app.session_registry().is_live(fixture.session_id));

    // An input through the very path the REST handler uses.
    let submitted = app.session_registry().submit(
        fixture.session_id,
        QueuedInput {
            input: SessionInput::Message {
                text: "and now the second turn".to_string(),
            },
            user_id: Some(fixture.user_id),
            client_id: Some("client-1".to_string()),
            accepted_at: Utc::now(),
        },
    );
    assert!(matches!(
        submitted,
        mars_orchestrator::session::SubmitResult::Forwarded
    ));
    wait_for_events(&app.pool, fixture.session_id, 4).await;
    assert_eq!(
        kinds(&app.pool, fixture.session_id).await,
        ["state_change", "init", "launch_warning", "user_message"],
    );

    // This owner has no container, so the stop is ignored rather than acted on
    // (the route answers 409 first); either way it must not end the loop.
    app.session_registry()
        .stop(fixture.session_id)
        .expect("the owner takes a stop");
    tokio::time::sleep(QUIET_FOR).await;
    assert!(app.session_registry().is_live(fixture.session_id));

    // Dropping the registry's sender closes the channel, which the loop treats
    // exactly as a `Shutdown`.
    app.session_registry().remove(fixture.session_id);
    tokio::time::timeout(COMMIT_WITHIN, handle)
        .await
        .expect("the owner leaves its loop when its channel closes")
        .expect("the owner task did not panic");

    // The rest of the transcript is never read: the owner is gone.
    let length = append_line(&fixture.dirs, &lines[2]).await;
    tokio::time::sleep(QUIET_FOR).await;
    let (_, _, offset) = snapshot(&app.pool, fixture.session_id).await;
    assert!(
        offset.unwrap_or(0) < length as i64,
        "a stopped owner kept reading"
    );
    assert!(!app.session_registry().is_live(fixture.session_id));
}

/// `init` stores `cli_session_id` and warns about the MCP server, and the
/// `init` the CLI repeats at the next turn changes nothing (ADR 0032: no
/// transition, no queue drain).
#[tokio::test]
async fn an_init_stores_the_cli_session_id_and_warns_about_the_mcp_server() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let owner = spawn_owner(&app, &fixture, 0, false);

    let (stored, state) = session_state(&app.pool, fixture.session_id).await;
    assert_eq!(
        stored, None,
        "a running session starts with no cli session id"
    );
    assert_eq!(state, "running");

    let lines = native_lines("multi_turn");
    let length = append_line(&fixture.dirs, &lines[0]).await;
    wait_for_offset(&app.pool, fixture.session_id, length).await;

    let (stored, state) = session_state(&app.pool, fixture.session_id).await;
    assert_eq!(
        stored.as_deref(),
        Some("daf41536-4488-45cc-9bdb-5a2c7244e0ba"),
        "the CLI's own session id was not stored",
    );
    assert_eq!(state, "running");
    // One `state_change`: the launch's. The `init` adds none (ADR 0032).
    let kinds = kinds(&app.pool, fixture.session_id).await;
    assert_eq!(kinds, ["state_change", "init", "launch_warning"]);

    let warning = sqlx::query_as::<_, (String,)>(
        "SELECT payload->>'message' FROM events WHERE session_id = $1 AND kind = 'launch_warning'",
    )
    .bind(fixture.session_id)
    .fetch_one(&app.pool)
    .await
    .expect("the warning is stored");
    assert_eq!(warning.0, FAILED_SERVER_WARNING);

    // The `init` the CLI repeats at the start of the next turn: same id, one
    // event per process, so nothing new is written.
    let before = snapshot(&app.pool, fixture.session_id).await;
    append_line(&fixture.dirs, &lines[4]).await;
    tokio::time::sleep(QUIET_FOR).await;
    assert_eq!(
        snapshot(&app.pool, fixture.session_id).await,
        before,
        "a repeated init wrote something",
    );

    owner.shutdown().await;
}

/// An `init` whose `mcp_servers` lists Mars's server as connected earns no
/// warning; one that does not list it at all is warned about as missing.
#[tokio::test]
async fn a_connected_server_earns_no_warning_and_an_absent_one_is_missing() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let owner = spawn_owner(&app, &fixture, 0, false);

    let connected = init_line(
        "sess-connected",
        r#"[{"name":"mars-orchestrator","status":"connected","source":"dynamic"}]"#,
    );
    let length = append_line(&fixture.dirs, &connected).await;
    wait_for_offset(&app.pool, fixture.session_id, length).await;
    assert_eq!(
        kinds(&app.pool, fixture.session_id).await,
        ["state_change", "init"],
        "a connected server was warned about",
    );

    // A different session id is a different conversation, so this is a second
    // `init` event rather than a repeat (`SPEC.md`, "AgentEvent", the `init`
    // rule) — and its server list mentions Mars's server nowhere.
    let absent = init_line(
        "sess-absent",
        r#"[{"name":"repo-local","status":"connected","source":"project"}]"#,
    );
    let length = append_line(&fixture.dirs, &absent).await;
    wait_for_offset(&app.pool, fixture.session_id, length).await;

    let kinds = kinds(&app.pool, fixture.session_id).await;
    assert_eq!(kinds, ["state_change", "init", "init", "launch_warning"]);
    let warnings = sqlx::query_as::<_, (String,)>(
        "SELECT payload->>'message' FROM events WHERE session_id = $1 AND kind = 'launch_warning'",
    )
    .bind(fixture.session_id)
    .fetch_all(&app.pool)
    .await
    .expect("the warnings are stored");
    assert_eq!(
        warnings.into_iter().map(|row| row.0).collect::<Vec<_>>(),
        ["MCP server mars-orchestrator is not connected (status: missing)"],
    );

    owner.shutdown().await;
}

/// One hand-written `system`/`init` line, for the server statuses no recorded
/// run produced (every recording reported `failed`).
fn init_line(session_id: &str, mcp_servers: &str) -> String {
    format!(
        r#"{{"type":"system","subtype":"init","session_id":"{session_id}","model":"claude-sonnet-5","tools":["Bash"],"mcp_servers":{mcp_servers}}}"#,
    )
}

/// An `init` for a session that is not `running` is a warning in the log, not a
/// transition: `running` is the launcher's decision at stdin attach (ADR 0032).
#[tokio::test]
async fn an_init_for_a_session_that_is_not_running_still_stores_the_id() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    // Deliberately left in `creating`.
    let owner = spawn_owner(&app, &fixture, 0, false);

    let length = append_line(&fixture.dirs, &native_lines("multi_turn")[0]).await;
    wait_for_offset(&app.pool, fixture.session_id, length).await;

    let (stored, state) = session_state(&app.pool, fixture.session_id).await;
    assert_eq!(
        stored.as_deref(),
        Some("daf41536-4488-45cc-9bdb-5a2c7244e0ba")
    );
    assert_eq!(state, "creating", "the owner transitioned the session");
    assert_eq!(
        kinds(&app.pool, fixture.session_id).await,
        ["init", "launch_warning"],
        "the owner wrote a state_change",
    );

    owner.shutdown().await;
}

/// Two `result` lines move the counters by the increase over the previous one,
/// and every batch advances `last_activity_at`.
#[tokio::test]
async fn two_results_accumulate_the_counters_by_their_increase() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let owner = spawn_owner(&app, &fixture, 0, false);

    let (_, _, _, before) = counters(&app.pool, fixture.session_id).await;
    let lines = native_lines("multi_turn");

    // Through the first `result`.
    let mut length = 0;
    for line in &lines[..4] {
        length = append_line(&fixture.dirs, line).await;
    }
    wait_for_offset(&app.pool, fixture.session_id, length).await;
    let (cost, input, output, _) = counters(&app.pool, fixture.session_id).await;
    assert!(
        (cost - 0.0412002).abs() < 1e-9,
        "the first result cost {cost}"
    );
    assert_eq!((input, output), (2, 5));

    for line in &lines[4..] {
        length = append_line(&fixture.dirs, line).await;
    }
    wait_for_offset(&app.pool, fixture.session_id, length).await;

    // 0.0470666 is the process's running total, not the second turn's own cost,
    // so the session's total is the later number and not the sum of the two
    // (`ARCHITECTURE.md`, "Cost accounting").
    let (cost, input, output, after) = counters(&app.pool, fixture.session_id).await;
    assert!(
        (cost - 0.0470666).abs() < 1e-9,
        "the two results cost {cost}"
    );
    assert_eq!((input, output), (4, 13), "usage is summed per turn");
    assert!(after > before, "last_activity_at did not advance");

    owner.shutdown().await;
}

/// The cumulative baseline survives adoption: the second process's `result` is
/// still charged only its increase over the first, which the restarted owner
/// recovered by replaying the committed transcript.
#[tokio::test]
async fn the_cumulative_baseline_is_restored_across_adoption() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;

    let lines = native_lines("multi_turn");
    let first = spawn_owner(&app, &fixture, 0, false);
    let mut length = 0;
    for line in &lines[..4] {
        length = append_line(&fixture.dirs, line).await;
    }
    wait_for_offset(&app.pool, fixture.session_id, length).await;
    first.shutdown().await;

    let second = spawn_owner(&app, &fixture, length, true);
    for line in &lines[4..] {
        length = append_line(&fixture.dirs, line).await;
    }
    wait_for_offset(&app.pool, fixture.session_id, length).await;

    let (cost, input, output, _) = counters(&app.pool, fixture.session_id).await;
    assert!(
        (cost - 0.0470666).abs() < 1e-9,
        "the adopted owner charged {cost} instead of the increase",
    );
    assert_eq!((input, output), (4, 13));

    second.shutdown().await;
}

/// A recorded input's echo is suppressed even when it arrives after the owner
/// that wrote it was replaced (`ARCHITECTURE.md`, "Durability and recovery").
#[tokio::test]
async fn a_delayed_echo_of_a_recorded_input_stays_suppressed_across_adoption() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;

    let first = spawn_owner(&app, &fixture, 0, false);
    // The text of the echo fixture's first two lines.
    first
        .commands
        .send(OwnerCommand::Input(QueuedInput {
            input: SessionInput::Message {
                text: "Reply with exactly the word PONG.".to_string(),
            },
            user_id: Some(fixture.user_id),
            client_id: None,
            accepted_at: Utc::now(),
        }))
        .await
        .expect("the owner is listening");
    wait_for_events(&app.pool, fixture.session_id, 2).await;
    first.shutdown().await;

    let resume_from = SessionRepository::new(&app.pool)
        .max_offset(fixture.session_id)
        .await
        .expect("the offset is readable")
        .unwrap_or(0) as u64;
    assert_eq!(
        resume_from, 0,
        "nothing from the transcript is committed yet"
    );

    let second = spawn_owner(&app, &fixture, resume_from, true);
    let echoes = native_lines("synthetic_echo");
    // The CLI's echo of the recorded input, then a line nobody wrote.
    let length = append_line(&fixture.dirs, &echoes[0]).await;
    tokio::time::sleep(QUIET_FOR).await;
    let (_, _, offset) = snapshot(&app.pool, fixture.session_id).await;
    assert_eq!(offset, None, "the echo was stored as an event");

    let length_after = append_line(&fixture.dirs, &echoes[2]).await;
    wait_for_offset(&app.pool, fixture.session_id, length_after).await;
    assert!(length < length_after);
    assert_eq!(
        kinds(&app.pool, fixture.session_id).await,
        ["state_change", "user_message", "raw"],
        "only the message the orchestrator never wrote is stored",
    );

    second.shutdown().await;
}

/// Two owners on one transcript is an invariant failure, and the loser says so
/// and stands down rather than appending the same line twice.
#[tokio::test]
async fn a_second_tailer_stands_down_instead_of_double_appending() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;

    let one = spawn_owner(&app, &fixture, 0, false);
    let two = spawn_owner(&app, &fixture, 0, false);

    let length = append_line(&fixture.dirs, &native_lines("multi_turn")[0]).await;
    wait_for_offset(&app.pool, fixture.session_id, length).await;
    // Long enough for the loser's own commit attempt to be rejected.
    tokio::time::sleep(QUIET_FOR).await;

    assert_eq!(
        kinds(&app.pool, fixture.session_id).await,
        ["state_change", "init", "launch_warning"],
        "the line was committed twice",
    );
    assert!(
        one.handle.is_finished() ^ two.handle.is_finished(),
        "exactly one of the two owners stands down",
    );

    // Whichever is left keeps tailing; the other's loop is already gone.
    for owner in [one, two] {
        if owner.handle.is_finished() {
            owner.abort().await;
        } else {
            owner.shutdown().await;
        }
    }
}

/// A transcript that shrank is never rewound: the owner skips to the new end
/// rather than re-reading history the database already holds.
#[tokio::test]
async fn a_truncated_transcript_is_not_rewound() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let owner = spawn_owner(&app, &fixture, 0, false);

    let lines = native_lines("multi_turn");
    let mut length = 0;
    for line in &lines[..4] {
        length = append_line(&fixture.dirs, line).await;
    }
    wait_for_offset(&app.pool, fixture.session_id, length).await;
    let before = snapshot(&app.pool, fixture.session_id).await;

    // Replaced under the running owner, as a manual cleanup would.
    tokio::fs::write(fixture.dirs.stream_jsonl(), b"")
        .await
        .expect("the transcript is truncated");
    tokio::time::sleep(QUIET_FOR).await;

    assert_eq!(
        snapshot(&app.pool, fixture.session_id).await,
        before,
        "the owner re-read the transcript from its start",
    );

    owner.shutdown().await;
}

/// The session row's whole lifecycle picture in one read.
async fn lifecycle(
    pool: &PgPool,
    session_id: Uuid,
) -> (
    String,
    Option<String>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<String>,
) {
    sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
            Option<String>,
        ),
    >(
        "SELECT state::text, container_id, parked_at, ended_at, error
         FROM sessions WHERE id = $1",
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

/// Wait until the container has been signalled with `signal`, or fail.
async fn wait_for_signal(app: &TestApp, container_id: &ContainerId, signal: Signal) {
    let deadline = tokio::time::Instant::now() + COMMIT_WITHIN;
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

/// One accepted message, as the registry would hand it over.
fn message(fixture: &Fixture, text: &str, client_id: Option<&str>) -> QueuedInput {
    QueuedInput {
        input: SessionInput::Message {
            text: text.to_string(),
        },
        user_id: Some(fixture.user_id),
        client_id: client_id.map(str::to_string),
        accepted_at: Utc::now(),
    }
}

/// An input is a `user_message` event first and a line on the CLI's stdin
/// second, so the conversation shows it even if the write never lands
/// (`ARCHITECTURE.md`, "Session owner task", step 2).
#[tokio::test]
async fn an_input_is_recorded_before_the_cli_sees_it() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let container = start_container(&app, &fixture).await;
    let (_, _, _, before) = counters(&app.pool, fixture.session_id).await;

    let owner = Owner::new(&app, &fixture)
        .container(&container)
        .attached(&container)
        .await
        .spawn();

    owner
        .commands
        .send(OwnerCommand::Input(message(
            &fixture,
            "the first turn",
            Some("client-7"),
        )))
        .await
        .expect("the owner is listening");

    // The event is committed before the write is attempted, so the moment the
    // container's stdin has the line the `user_message` must already be there.
    let deadline = tokio::time::Instant::now() + COMMIT_WITHIN;
    let written = loop {
        let lines = app.engine().stdin_lines(&container);
        if let Some(line) = lines.first() {
            assert_eq!(
                kinds(&app.pool, fixture.session_id).await,
                ["state_change", "user_message"],
                "the input reached the CLI before it was recorded",
            );
            assert_eq!(lines.len(), 1, "the input was written more than once");
            break line.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "nothing was written to the CLI's stdin",
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    let native: Value = serde_json::from_str(&written).expect("the encoded input is one JSON line");
    assert_eq!(native["type"], "user");
    assert_eq!(native["message"]["content"][0]["text"], "the first turn");

    let event = last_event(&app.pool, fixture.session_id, "user_message").await;
    assert_eq!(event["text"], "the first turn");
    assert_eq!(event["user_id"], fixture.user_id.to_string());
    assert_eq!(event["client_id"], "client-7");

    // Every appended event advances the timestamp the idle reaper measures from,
    // `user_message` included (`ARCHITECTURE.md`, "Session owner task", step 4).
    let (_, _, _, after) = counters(&app.pool, fixture.session_id).await;
    assert!(after > before, "last_activity_at did not advance");

    owner.shutdown().await;
}

/// A write that cannot land is a non-fatal `error` event and nothing else: v1
/// does not retry an input (ADR 0020).
///
/// The owner deliberately has no container to watch, so the exit that breaks the
/// attach stream does not also end the session; what is under test is the write.
#[tokio::test]
async fn a_failed_write_records_a_non_fatal_error_and_does_not_retry() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let container = start_container(&app, &fixture).await;

    let owner = Owner::new(&app, &fixture)
        .attached(&container)
        .await
        .spawn();
    assert!(
        app.engine().exit(&container, 0),
        "the mock container is there to end",
    );

    owner
        .commands
        .send(OwnerCommand::Input(message(
            &fixture,
            "into a broken pipe",
            None,
        )))
        .await
        .expect("the owner is listening");

    wait_for_events(&app.pool, fixture.session_id, 3).await;
    assert_eq!(
        kinds(&app.pool, fixture.session_id).await,
        ["state_change", "user_message", "error"],
    );

    let event = last_event(&app.pool, fixture.session_id, "error").await;
    assert_eq!(event["fatal"], false);
    let reported = event["message"].as_str().expect("the error has a message");
    assert!(
        reported.starts_with("failed to write input to CLI: "),
        "{reported}",
    );
    assert!(
        app.engine().stdin_bytes(&container).is_empty(),
        "bytes reached a container that had exited",
    );

    // Nothing is retried: the transcript stays as it was.
    tokio::time::sleep(QUIET_FOR).await;
    let (count, _, _) = snapshot(&app.pool, fixture.session_id).await;
    assert_eq!(count, 3, "the failed write was retried");

    owner.shutdown().await;
}

/// Exit 0 with no stop is the CLI finishing: `parked`, container removed,
/// `container_id` cleared, and the registry entry kept so a later message
/// resumes it (`ARCHITECTURE.md`, "Session lifecycle").
#[tokio::test]
async fn a_clean_exit_parks_the_session_and_removes_its_container() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let container = start_container(&app, &fixture).await;

    let owner = Owner::new(&app, &fixture)
        .container(&container)
        .attached(&container)
        .await
        .registered()
        .spawn();

    assert!(app.engine().exit(&container, 0));
    owner.ended().await;

    let (state, container_column, parked_at, ended_at, error) =
        lifecycle(&app.pool, fixture.session_id).await;
    assert_eq!(state, "parked");
    assert_eq!(container_column, None, "the container id was not cleared");
    assert!(parked_at.is_some(), "parked_at was not set");
    assert_eq!(ended_at, None, "a parked session ended");
    assert_eq!(error, None);

    let change = last_event(&app.pool, fixture.session_id, "state_change").await;
    assert_eq!(change["from"], "running");
    assert_eq!(change["to"], "parked");
    assert_eq!(change["reason"], "CLI exited");
    assert!(change.get("signal").is_none(), "{change}");

    assert_eq!(
        app.engine().state_of(&container),
        None,
        "the container was not removed",
    );
    // The entry stays, without a channel: that is where the next message queues
    // until a resume drains it.
    assert_eq!(
        app.session_registry().phase(fixture.session_id),
        Some(Phase::Parked),
    );
}

/// A non-zero exit outside a stop is a failure, and `sessions.error` says what
/// the code was.
#[tokio::test]
async fn a_non_zero_exit_fails_the_session_with_its_status() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let container = start_container(&app, &fixture).await;

    let owner = Owner::new(&app, &fixture)
        .container(&container)
        .registered()
        .spawn();

    assert!(app.engine().exit(&container, 137));
    owner.ended().await;

    let (state, container_column, _, ended_at, error) =
        lifecycle(&app.pool, fixture.session_id).await;
    assert_eq!(state, "failed");
    assert_eq!(container_column, None);
    assert!(ended_at.is_some(), "ended_at was not set");
    assert_eq!(error.as_deref(), Some("CLI exited with status 137"));

    let change = last_event(&app.pool, fixture.session_id, "state_change").await;
    assert_eq!(change["to"], "failed");
    assert_eq!(change["reason"], "CLI exited with status 137");

    // A failed session has nothing more to say to, so its entry goes.
    assert_eq!(app.session_registry().phase(fixture.session_id), None);
}

/// A container the engine no longer has is `parked`, not `failed`: there is no
/// exit code to read and nothing for a user to retry
/// (`ARCHITECTURE.md`, "Restart procedure").
#[tokio::test]
async fn a_container_that_disappeared_parks_the_session() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let container = start_container(&app, &fixture).await;

    let owner = Owner::new(&app, &fixture).container(&container).spawn();
    assert!(app.engine().vanish(&container));
    owner.ended().await;

    let (state, _, parked_at, _, _) = lifecycle(&app.pool, fixture.session_id).await;
    assert_eq!(state, "parked");
    assert!(parked_at.is_some());
    let change = last_event(&app.pool, fixture.session_id, "state_change").await;
    assert_eq!(change["reason"], "container disappeared");
}

/// A stop is `SIGINT`, then `SIGTERM` when the grace period runs out, and the
/// `state_change` names the last signal sent so the UI can say killed rather
/// than stopped (`ARCHITECTURE.md`, "Stop semantics").
#[tokio::test]
async fn a_stop_escalates_to_sigterm_and_parks_the_session_as_killed() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let container = start_container(&app, &fixture).await;

    let owner = Owner::new(&app, &fixture).container(&container).spawn();
    owner
        .commands
        .send(OwnerCommand::Stop)
        .await
        .expect("the owner is listening");

    wait_for_signal(&app, &container, Signal::Sigint).await;
    // A second request while one is pending changes nothing.
    owner
        .commands
        .send(OwnerCommand::Stop)
        .await
        .expect("the owner is listening");
    // `STOP_GRACE_SECS` is 1 in tests; the CLI here traps nothing and keeps
    // running, which is exactly the case SIGTERM exists for.
    wait_for_signal(&app, &container, Signal::Sigterm).await;
    assert_eq!(
        app.engine().signals(&container),
        vec![Signal::Sigint, Signal::Sigterm],
        "the stop sequence was not SIGINT then SIGTERM exactly once each",
    );

    assert!(app.engine().exit(&container, 143));
    owner.ended().await;

    let (state, _, parked_at, _, error) = lifecycle(&app.pool, fixture.session_id).await;
    assert_eq!(state, "parked", "a stopped session failed");
    assert!(parked_at.is_some());
    assert_eq!(error, None);

    let change = last_event(&app.pool, fixture.session_id, "state_change").await;
    assert_eq!(change["to"], "parked");
    assert_eq!(change["reason"], "stopped by user");
    assert_eq!(change["signal"], "SIGTERM");
}

/// A CLI that exits inside the grace period is never sent `SIGTERM`, and the
/// `state_change` says `SIGINT`: stopped, not killed.
#[tokio::test]
async fn a_stop_the_cli_answers_within_the_grace_period_records_sigint() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;
    let container = start_container(&app, &fixture).await;

    let owner = Owner::new(&app, &fixture).container(&container).spawn();
    owner
        .commands
        .send(OwnerCommand::Stop)
        .await
        .expect("the owner is listening");

    wait_for_signal(&app, &container, Signal::Sigint).await;
    assert_eq!(
        app.engine().signals(&container),
        vec![Signal::Sigint],
        "the stop sent more than the SIGINT it starts with",
    );

    // The recorded behaviour of a SIGINT-ed CLI: it ends its turn and exits 0
    // (`ARCHITECTURE.md`, "Stop semantics").
    assert!(app.engine().exit(&container, 0));
    owner.ended().await;

    let (state, _, _, _, _) = lifecycle(&app.pool, fixture.session_id).await;
    assert_eq!(state, "parked");
    let change = last_event(&app.pool, fixture.session_id, "state_change").await;
    assert_eq!(change["reason"], "stopped by user");
    // The last signal *sent*, so a SIGTERM after the exit would show here; the
    // container itself is gone by now, removed with the session's clean-up.
    assert_eq!(change["signal"], "SIGINT");
}

/// A stop for a session that has no container yet is logged and ignored; the
/// route answers 409 long before it gets here.
#[tokio::test]
async fn a_stop_without_a_container_is_ignored() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    mark_running(&app, fixture.session_id).await;

    let owner = Owner::new(&app, &fixture).spawn();
    owner
        .commands
        .send(OwnerCommand::Stop)
        .await
        .expect("the owner is listening");
    tokio::time::sleep(QUIET_FOR).await;

    let (state, _, _, _, _) = lifecycle(&app.pool, fixture.session_id).await;
    assert_eq!(state, "running", "an ignored stop changed the session");
    assert_eq!(
        kinds(&app.pool, fixture.session_id).await,
        ["state_change"],
        "an ignored stop wrote an event",
    );

    owner.shutdown().await;
}

/// The ephemeral end-of-run: the `result` publishes the branch, stops the
/// container and marks the session `done`, and the end-of-session hook runs
/// (`ARCHITECTURE.md`, "Claude Code invocation").
#[tokio::test]
async fn an_ephemeral_result_syncs_the_branch_and_ends_the_session() {
    let app = TestApp::spawn().await;
    let fixture = fixture_of_kind(&app, ProfileKind::Ephemeral).await;
    mark_running(&app, fixture.session_id).await;
    let _upstream = prepare_git(&app, &fixture).await;
    let container = start_container(&app, &fixture).await;

    let ended = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&ended);
    let state = app
        .state
        .clone()
        .with_session_ended_hook(Arc::new(move |_| {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
            }
            .boxed()
        }));

    let owner = Owner::new(&app, &fixture)
        .kind(ProfileKind::Ephemeral)
        .container(&container)
        .registered()
        .state(state)
        .spawn();

    // The recorded run's first turn: `init`, its text and the `result` that ends
    // an ephemeral session.
    for line in &native_lines("multi_turn")[..4] {
        append_line(&fixture.dirs, line).await;
    }

    owner.ended().await;

    let (state, container_column, parked_at, ended_at, error) =
        lifecycle(&app.pool, fixture.session_id).await;
    assert_eq!(state, "done", "an ephemeral result did not end the session");
    assert_eq!(container_column, None);
    assert_eq!(parked_at, None, "an ephemeral session was parked");
    assert!(ended_at.is_some(), "ended_at was not set");
    assert_eq!(error, None);

    let sync = last_event(&app.pool, fixture.session_id, "git").await;
    assert_eq!(sync["op"], "sync");
    assert_eq!(sync["ok"], true, "{sync}");
    assert_eq!(
        sync["detail"]["ref"],
        format!("refs/sessions/{}", fixture.session_id),
    );
    assert_eq!(
        sync["detail"]["commit"]
            .as_str()
            .expect("the sync names a commit")
            .len(),
        40,
    );

    let change = last_event(&app.pool, fixture.session_id, "state_change").await;
    assert_eq!(change["to"], "done");
    assert_eq!(change["reason"], "result received");

    assert_eq!(
        app.engine().state_of(&container),
        None,
        "the container was not removed",
    );
    assert_eq!(app.session_registry().phase(fixture.session_id), None);
    assert_eq!(
        ended.load(Ordering::SeqCst),
        1,
        "the end-of-session hook did not run exactly once",
    );
}

/// An ephemeral session whose CLI exits before any `result` produced nothing, so
/// it is `failed` rather than parked: it is never retried.
#[tokio::test]
async fn an_ephemeral_exit_before_its_result_fails_the_session() {
    let app = TestApp::spawn().await;
    let fixture = fixture_of_kind(&app, ProfileKind::Ephemeral).await;
    mark_running(&app, fixture.session_id).await;
    let container = start_container(&app, &fixture).await;

    let owner = Owner::new(&app, &fixture)
        .kind(ProfileKind::Ephemeral)
        .container(&container)
        .spawn();

    assert!(app.engine().exit(&container, 1));
    owner.ended().await;

    let (state, _, parked_at, ended_at, error) = lifecycle(&app.pool, fixture.session_id).await;
    assert_eq!(state, "failed");
    assert_eq!(parked_at, None);
    assert!(ended_at.is_some());
    assert_eq!(
        error.as_deref(),
        Some("CLI exited with status 1 before result"),
    );
}

/// An ephemeral session is never parked (ADR 0003), so a stop — the idle reaper
/// sends one to a stalled run — fails it, with the signal still recorded
/// (`ARCHITECTURE.md`, "Stop semantics").
#[tokio::test]
async fn a_stopped_ephemeral_session_fails_instead_of_parking() {
    let app = TestApp::spawn().await;
    let fixture = fixture_of_kind(&app, ProfileKind::Ephemeral).await;
    mark_running(&app, fixture.session_id).await;
    let container = start_container(&app, &fixture).await;

    let owner = Owner::new(&app, &fixture)
        .kind(ProfileKind::Ephemeral)
        .container(&container)
        .spawn();
    owner
        .commands
        .send(OwnerCommand::Stop)
        .await
        .expect("the owner is listening");

    wait_for_signal(&app, &container, Signal::Sigint).await;
    assert!(app.engine().exit(&container, 0));
    owner.ended().await;

    let (state, _, parked_at, ended_at, error) = lifecycle(&app.pool, fixture.session_id).await;
    assert_eq!(state, "failed");
    assert_eq!(parked_at, None);
    assert!(ended_at.is_some());
    assert_eq!(error.as_deref(), Some("stopped by user"));

    let change = last_event(&app.pool, fixture.session_id, "state_change").await;
    assert_eq!(change["to"], "failed");
    assert_eq!(change["signal"], "SIGINT");
}

/// A container that exits while the session is still `creating` never got as far
/// as its CLI, so the launch failed (`ARCHITECTURE.md`, "Session lifecycle").
#[tokio::test]
async fn an_exit_while_still_creating_fails_before_init() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    // Deliberately left in `creating`: no stdin was ever attached (ADR 0032).
    let container = start_container(&app, &fixture).await;

    let owner = Owner::new(&app, &fixture).container(&container).spawn();
    assert!(app.engine().exit(&container, 2));
    owner.ended().await;

    let (state, container_column, _, ended_at, error) =
        lifecycle(&app.pool, fixture.session_id).await;
    assert_eq!(state, "failed");
    assert_eq!(container_column, None);
    assert!(ended_at.is_some());
    assert_eq!(
        error.as_deref(),
        Some("CLI exited with status 2 before init"),
    );

    let change = last_event(&app.pool, fixture.session_id, "state_change").await;
    assert_eq!(change["from"], "creating");
    assert_eq!(change["to"], "failed");
}

/// An input for an ephemeral session is dropped: its prompt is in argv and its
/// stdin is never written to (ADR 0003).
#[tokio::test]
async fn an_input_to_an_ephemeral_session_is_dropped() {
    let app = TestApp::spawn().await;
    let fixture = fixture_of_kind(&app, ProfileKind::Ephemeral).await;
    mark_running(&app, fixture.session_id).await;
    let container = start_container(&app, &fixture).await;

    let owner = Owner::new(&app, &fixture)
        .kind(ProfileKind::Ephemeral)
        .attached(&container)
        .await
        .spawn();

    owner
        .commands
        .send(OwnerCommand::Input(message(&fixture, "not for you", None)))
        .await
        .expect("the owner is listening");
    tokio::time::sleep(QUIET_FOR).await;

    assert_eq!(
        kinds(&app.pool, fixture.session_id).await,
        ["state_change"],
        "an ephemeral session recorded an input",
    );
    assert!(
        app.engine().stdin_bytes(&container).is_empty(),
        "an ephemeral session's stdin was written to",
    );

    owner.shutdown().await;
}
