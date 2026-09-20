//! The session service: what a user asks of a session (`ARCHITECTURE.md`,
//! "Session lifecycle", "Stop semantics", "Storage", "Git model" → Fetch-back;
//! `SPEC.md`, "Sessions"; ADR 0003, ADR 0020; `CLAUDE.md`, "Testing
//! expectations").
//!
//! Every scenario drives the real [`SessionService`] over a real project
//! repository and a real session work clone — git is never mocked — with
//! `MockEngine` standing in for the container, because a signal, an exit code
//! and a removal are the whole of what the stop and end rules read of it.
//!
//! What the assertions are about: which state each action leads to, that the
//! documented refusals are conflicts and name the state, that an end leaves
//! nothing behind (no container, no `container_id`, no registry entry) and
//! publishes the branch with a `git { op: "sync" }` event, and that a delete
//! removes the session's directory and its CLI transcript before its row.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use common::TestApp;
use futures_util::FutureExt;
use mars_orchestrator::engine::{
    ContainerEngine, ContainerId, ContainerSpec, LABEL_PROJECT_ID, LABEL_SESSION_ID, Signal,
};
use mars_orchestrator::events::SessionInput;
use mars_orchestrator::git::testutil::{TestUpstream, run_git, test_identity};
use mars_orchestrator::git::{
    DataPaths, create_work_clone, init_project_repo, resolve_base, session_branch,
};
use mars_orchestrator::models::{
    BranchName, NewProject, NewSession, ProfileKind, Project, ProjectStatus, RemoteUrl, Session,
    SessionState,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository, Transition};
use mars_orchestrator::session::{
    LaunchMode, Launcher, Phase, SessionDirs, SessionService, initial_token,
};
use serde_json::Value;
use uuid::Uuid;

/// Not a real remote: the fixture value every project test stores (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// Not a real image: the stub the session fixtures name.
const TEST_IMAGE: &str = "localhost/mars-session:test";

/// Not a real id: what a CLI `init` event would have recorded.
const FAKE_CLI_SESSION_ID: &str = "fake-cli-session-id";

/// How long a test waits for something another task has to do.
const WITHIN: Duration = Duration::from_secs(20);

/// How often it looks while it waits.
const POLL: Duration = Duration::from_millis(25);

/// A user, a `ready` project with a real repository, a profile and one session
/// with a work clone on its own branch.
struct Fixture {
    /// Held so the upstream repository outlives the test.
    _upstream: TestUpstream,
    user_id: Uuid,
    project: Project,
    session: Session,
}

impl Fixture {
    /// A conversational session in `creating`, ready to be launched.
    async fn create(app: &TestApp) -> Self {
        Self::of_kind(app, ProfileKind::Conversational).await
    }

    /// The same for a session of either kind.
    async fn of_kind(app: &TestApp, kind: ProfileKind) -> Self {
        let suffix = &Uuid::new_v4().simple().to_string()[..8];
        let upstream = TestUpstream::create().await;

        let user = app
            .insert_user(
                &format!("user-{suffix}"),
                &format!("user-{suffix}@example.invalid"),
                false,
                false,
            )
            .await;
        let project = seed_ready_project(app, &format!("project-{suffix}"), &upstream).await;

        let profile_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO agent_profiles (id, project_id, name, kind, image, partial_messages)
             VALUES ($1, $2, 'default', $3, $4, FALSE)",
        )
        .bind(profile_id)
        .bind(project.id)
        .bind(kind)
        .bind(TEST_IMAGE)
        .execute(&app.pool)
        .await
        .expect("the profile seeds");

        let mut new_session = NewSession::new(
            project.id,
            profile_id,
            kind,
            "refs/heads/main",
            initial_token().hash(),
        );
        new_session.created_by = Some(user.id);

        let mut tx = app.pool.begin().await.expect("a transaction begins");
        let session = SessionRepository::new(&app.pool)
            .insert(&mut tx, &new_session)
            .await
            .expect("the session inserts");
        tx.commit().await.expect("the transaction commits");

        let dirs = SessionDirs::from_config(&app.state.config, session.id);
        dirs.ensure()
            .await
            .expect("the session directories are made");

        // A real clone on this session's branch, so the fetch-back has a commit
        // to publish and a resume has a checkout to reuse.
        let paths = DataPaths::from_config(&app.state.config);
        {
            let guard = app.state.git_locks.lock(project.id).await;
            let base = resolve_base(&guard, &paths, None, "main")
                .await
                .expect("main resolves");
            create_work_clone(&guard, &paths, session.id, &base, &test_identity())
                .await
                .expect("the session work clone is created");
        }

        Self {
            _upstream: upstream,
            user_id: user.id,
            project,
            session,
        }
    }

    /// Put the session in `state`, through the edges the lifecycle has.
    async fn move_to(&self, app: &TestApp, state: SessionState) {
        let path: &[(SessionState, SessionState)] = match state {
            SessionState::Creating => &[],
            SessionState::Running => &[(SessionState::Creating, SessionState::Running)],
            SessionState::Parked => &[
                (SessionState::Creating, SessionState::Running),
                (SessionState::Running, SessionState::Parked),
            ],
            SessionState::Done => &[
                (SessionState::Creating, SessionState::Running),
                (SessionState::Running, SessionState::Done),
            ],
            SessionState::Failed => &[(SessionState::Creating, SessionState::Failed)],
        };

        let repository = SessionRepository::new(&app.pool);
        for (from, to) in path {
            let mut tx = app.pool.begin().await.expect("a transaction begins");
            let mut change = Transition::new(*from, *to, "arranged by the test");
            if *to == SessionState::Failed {
                change = change.with_error("arranged by the test");
            }
            repository
                .transition(&mut tx, self.session.id, &change)
                .await
                .expect("the arranged transition applies");
            tx.commit().await.expect("the transaction commits");
        }
    }

    fn id(&self) -> Uuid {
        self.session.id
    }
}

/// A `ready` project with a repository on disk, pointed at `upstream`.
async fn seed_ready_project(app: &TestApp, name: &str, upstream: &TestUpstream) -> Project {
    let mut new_project = NewProject::new(name, TEST_REMOTE).expect("the test project is valid");
    new_project.default_branch = Some(BranchName::parse("main").expect("main is a branch name"));

    let projects = ProjectRepository::new(&app.pool);
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = projects
        .insert(&mut tx, &new_project)
        .await
        .expect("the project inserts");
    tx.commit().await.expect("the transaction commits");

    {
        let guard = app.state.git_locks.lock(inserted.id).await;
        init_project_repo(
            &guard,
            &DataPaths::from_config(&app.state.config),
            &RemoteUrl::local_for_tests(&upstream.path),
            Some("main"),
            None,
        )
        .await
        .expect("the project repository is initialised");
    }

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let ready = projects
        .set_status(&mut tx, inserted.id, ProjectStatus::Ready, None)
        .await
        .expect("the status is set")
        .expect("the project exists");
    tx.commit().await.expect("the transaction commits");

    ready
}

/// The service under test, over a state whose end-of-session hook counts the
/// sessions it was told about.
fn service_with_hook(app: &TestApp) -> (SessionService, Arc<Mutex<Vec<Uuid>>>) {
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

    (SessionService::new(&state), ended)
}

/// The session row as it is now.
async fn reload(app: &TestApp, session_id: Uuid) -> Session {
    SessionRepository::new(&app.pool)
        .get(session_id)
        .await
        .expect("the session is readable")
}

/// Record a `cli_session_id`, the way the owner's first `init` event does.
async fn record_cli_session_id(app: &TestApp, session_id: Uuid) {
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .set_cli_session_id(&mut tx, session_id, FAKE_CLI_SESSION_ID)
        .await
        .expect("the CLI session id is recorded");
    tx.commit().await.expect("the transaction commits");
}

/// Create and start a container for a session and record its id, the way a
/// launch would.
async fn start_container(app: &TestApp, fixture: &Fixture) -> ContainerId {
    let spec = ContainerSpec {
        image: TEST_IMAGE.to_string(),
        name: format!("mars-session-{}", fixture.id()),
        labels: BTreeMap::from([
            (LABEL_SESSION_ID.to_string(), fixture.id().to_string()),
            (LABEL_PROJECT_ID.to_string(), fixture.project.id.to_string()),
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
        .set_container_id(&mut tx, fixture.id(), Some(&container_id.0))
        .await
        .expect("the container id is recorded");
    tx.commit().await.expect("the transaction commits");

    container_id
}

/// Every event kind this session has, in order.
async fn kinds(app: &TestApp, session_id: Uuid) -> Vec<String> {
    sqlx::query_scalar::<_, String>("SELECT kind FROM events WHERE session_id = $1 ORDER BY seq")
        .bind(session_id)
        .fetch_all(&app.pool)
        .await
        .expect("the events are readable")
}

/// The payload of the last event of `kind` this session has.
async fn last_event(app: &TestApp, session_id: Uuid, kind: &str) -> Value {
    sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM events WHERE session_id = $1 AND kind = $2
         ORDER BY seq DESC LIMIT 1",
    )
    .bind(session_id)
    .bind(kind)
    .fetch_one(&app.pool)
    .await
    .unwrap_or_else(|err| panic!("a {kind} event is readable: {err}"))
}

/// Wait until `condition` holds, or fail saying what was being waited for.
async fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + WITHIN;
    while tokio::time::Instant::now() < deadline {
        if condition() {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("timed out waiting for {what}");
}

/// Wait until the session row reads `state`, or fail saying what it reads.
async fn wait_for_state(app: &TestApp, session_id: Uuid, state: SessionState) {
    let deadline = tokio::time::Instant::now() + WITHIN;
    while tokio::time::Instant::now() < deadline {
        let now = reload(app, session_id).await.state;
        if now == state {
            return;
        }
        tokio::time::sleep(POLL).await;
    }

    panic!(
        "timed out waiting for the session to reach {}; it reads {}",
        state.as_str(),
        reload(app, session_id).await.state.as_str(),
    );
}

/// Wait until the session row names a container, or give up and return `None`.
///
/// The launcher records `container_id` in a transaction of its own right after
/// the create, so a test standing inside `creating` reads it from the row
/// rather than guessing the mock's next id.
async fn wait_for_container_id(app: &TestApp, session_id: Uuid) -> Option<String> {
    let deadline = tokio::time::Instant::now() + WITHIN;
    while tokio::time::Instant::now() < deadline {
        if let Some(container_id) = reload(app, session_id).await.container_id {
            return Some(container_id);
        }
        tokio::time::sleep(POLL).await;
    }

    None
}

/// Wait until the session has an event of `kind`, or fail.
async fn wait_for_event(app: &TestApp, session_id: Uuid, kind: &str) {
    let deadline = tokio::time::Instant::now() + WITHIN;
    while tokio::time::Instant::now() < deadline {
        if kinds(app, session_id).await.iter().any(|had| had == kind) {
            return;
        }
        tokio::time::sleep(POLL).await;
    }

    panic!("timed out waiting for a {kind} event");
}

/// The commit `refs/sessions/<sid>` points at in the project repository, or
/// `None` when the mirror has no such ref.
async fn mirror_session_ref(app: &TestApp, fixture: &Fixture) -> Option<String> {
    let repo = DataPaths::from_config(&app.state.config).project_repo(fixture.project.id);
    let listed = run_git(
        &repo,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            &format!("refs/sessions/{}", fixture.id()),
        ],
    )
    .await;

    let commit = listed.trim().to_string();
    (!commit.is_empty()).then_some(commit)
}

/// The message a refusal answers, once it has been checked to be a 409.
///
/// A lifecycle refusal reaches the caller either as [`Error::Conflict`] — the
/// service's own check — or as the session model's own
/// [`Error::Session`], which [`Error::status`] maps to the same 409 (`SPEC.md`,
/// "Sessions"). What the API sends is identical, so the assertions read the
/// status and the message rather than the variant.
fn conflict(error: Error) -> String {
    assert_eq!(
        error.status(),
        axum::http::StatusCode::CONFLICT,
        "a lifecycle refusal is a 409: {error:?}",
    );

    error.to_string()
}

#[tokio::test]
async fn a_message_to_a_parked_session_resumes_it() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture.move_to(&app, SessionState::Parked).await;
    record_cli_session_id(&app, fixture.id()).await;

    let service = SessionService::new(&app.state);
    service
        .send_input(
            fixture.id(),
            SessionInput::Message {
                text: "carry on".to_string(),
            },
            Some(fixture.user_id),
            Some("client-1".to_string()),
        )
        .await
        .expect("a parked conversational session accepts a message");

    // The registry knew nothing of this session — a restart's `parked` row —
    // and the input queued all the same, which is what the resume drains.
    let engine = app.engine();
    wait_until("the resume to create a container", || {
        !engine.specs().is_empty()
    })
    .await;

    let spec = engine.specs().remove(0);
    assert!(
        spec.cmd
            .windows(2)
            .any(|pair| pair == ["--resume".to_string(), FAKE_CLI_SESSION_ID.to_string()]),
        "the relaunch did not resume the conversation: {:?}",
        spec.cmd,
    );

    wait_until("the resumed session to reach running", || {
        app.session_registry().phase(fixture.id()) == Some(Phase::Running)
    })
    .await;
    assert_eq!(
        reload(&app, fixture.id()).await.state,
        SessionState::Running
    );
}

/// The window task `dthk8` closed: a message sent the instant the row reads
/// `parked` used to be answered 202 and dropped.
///
/// The session's owner writes the `parked` state change and then removes the
/// container, which takes a few hundred milliseconds on a real engine, so the
/// message arrived while the registry still said `Running` with a channel the
/// owner had stopped reading. Holding the mock's removal is what makes that
/// window a moment the test chooses rather than one it hopes for: the row reads
/// `parked`, the owner is inside its clean-up, and the message goes in there.
#[tokio::test]
async fn a_message_that_arrives_as_the_session_parks_relaunches_it() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    record_cli_session_id(&app, fixture.id()).await;

    // A real owner on a real (mock) container, the way the other lifecycle
    // scenarios here get one.
    Launcher::from_state(&app.state)
        .launch(fixture.id(), LaunchMode::Resume)
        .expect("nothing else is launching this session")
        .await
        .expect("the launch task does not panic");
    let container_id = ContainerId(
        reload(&app, fixture.id())
            .await
            .container_id
            .expect("a running session records its container"),
    );

    let engine = app.engine();
    engine.hold_removals();
    // A clean exit parks a conversational session.
    assert!(engine.exit(&container_id, 0), "the container is the mock's");
    wait_for_state(&app, fixture.id(), SessionState::Parked).await;

    // Exactly the window: the row says `parked` and the owner is still in its
    // clean-up, holding the container it has not removed yet.
    assert!(
        engine.state_of(&container_id).is_some(),
        "the removal was not held",
    );
    SessionService::new(&app.state)
        .send_input(
            fixture.id(),
            SessionInput::Message {
                text: "are you still there".to_string(),
            },
            Some(fixture.user_id),
            Some("client-race".to_string()),
        )
        .await
        .expect("a parked session accepts a message");

    engine.resume_removals();

    wait_for_state(&app, fixture.id(), SessionState::Running).await;
    wait_for_event(&app, fixture.id(), "user_message").await;
    let recorded = last_event(&app, fixture.id(), "user_message").await;
    assert_eq!(recorded["text"], "are you still there");
    assert_eq!(recorded["client_id"], "client-race");
}

#[tokio::test]
async fn an_ephemeral_session_accepts_no_input() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::of_kind(&app, ProfileKind::Ephemeral).await;
    fixture.move_to(&app, SessionState::Running).await;

    let error = SessionService::new(&app.state)
        .send_input(
            fixture.id(),
            SessionInput::Message {
                text: "and another thing".to_string(),
            },
            Some(fixture.user_id),
            None,
        )
        .await
        .expect_err("an ephemeral session takes only its launch prompt");

    assert_eq!(conflict(error), "ephemeral sessions accept no input");
    assert!(app.engine().specs().is_empty(), "nothing was launched");
}

#[tokio::test]
async fn a_done_session_accepts_no_input() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture.move_to(&app, SessionState::Done).await;

    let error = SessionService::new(&app.state)
        .send_input(
            fixture.id(),
            SessionInput::Message {
                text: "one more".to_string(),
            },
            Some(fixture.user_id),
            None,
        )
        .await
        .expect_err("a done session is not resumable");

    assert_eq!(conflict(error), "session is done");
}

#[tokio::test]
async fn only_a_running_session_can_be_stopped() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture.move_to(&app, SessionState::Parked).await;

    let error = SessionService::new(&app.state)
        .stop(fixture.id())
        .await
        .expect_err("a parked session has no run to stop");

    assert_eq!(conflict(error), "session is parked");
}

#[tokio::test]
async fn ending_a_running_session_stops_it_publishes_its_branch_and_closes_it() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    // A resume rather than a fresh launch: the fixture already has the work
    // clone every other scenario here needs, and what this test wants of the
    // launcher is only a real owner attached to a real container.
    Launcher::from_state(&app.state)
        .launch(fixture.id(), LaunchMode::Resume)
        .expect("nothing else is launching this session")
        .await
        .expect("the launch task does not panic");

    let session = reload(&app, fixture.id()).await;
    assert_eq!(session.state, SessionState::Running, "the launch failed");
    let container_id = ContainerId(
        session
            .container_id
            .clone()
            .expect("a running session records its container"),
    );

    // The CLI the mock runs traps nothing, so the SIGINT is recorded and the
    // container keeps going until something ends it: this is the process
    // exiting cleanly on the stop, which is what parks the session. The signal
    // has to be observed here rather than after the end, because the owner
    // removes the container it was recorded on.
    let engine = Arc::clone(&app.engine);
    let stopped = tokio::spawn({
        let container_id = container_id.clone();
        async move {
            let deadline = tokio::time::Instant::now() + WITHIN;
            while tokio::time::Instant::now() < deadline {
                if engine.signals(&container_id).contains(&Signal::Sigint) {
                    engine.exit(&container_id, 0);
                    return true;
                }
                tokio::time::sleep(POLL).await;
            }
            false
        }
    });

    let (service, ended) = service_with_hook(&app);
    let closed = service.end(fixture.id()).await.expect("the session ends");
    assert!(
        stopped.await.expect("the exit helper does not panic"),
        "the stop did not reach the container as a SIGINT",
    );

    assert_eq!(closed.state, SessionState::Done);
    assert_eq!(closed.container_id, None, "the DTO still names a container");
    assert!(closed.ended_at.is_some());

    let row = reload(&app, fixture.id()).await;
    assert_eq!(row.state, SessionState::Done);
    assert_eq!(row.container_id, None);

    assert_eq!(
        app.engine().state_of(&container_id),
        None,
        "the container was not removed",
    );

    // The branch reached the mirror, and the transcript says so.
    let sync = last_event(&app, fixture.id(), "git").await;
    assert_eq!(sync["op"], "sync");
    assert_eq!(sync["ok"], true, "{sync}");
    assert_eq!(
        sync["detail"]["ref"],
        format!("refs/sessions/{}", fixture.id()),
    );
    let published = mirror_session_ref(&app, &fixture)
        .await
        .expect("the mirror has the session ref");
    assert_eq!(sync["detail"]["commit"], published);

    let change = last_event(&app, fixture.id(), "state_change").await;
    assert_eq!(change["to"], "done");
    assert_eq!(change["reason"], "ended by user");

    assert_eq!(
        *ended.lock().expect("the hook lock is healthy"),
        vec![fixture.id()],
        "the end-of-session hook must run exactly once",
    );
    assert_eq!(app.session_registry().phase(fixture.id()), None);
}

#[tokio::test]
async fn ending_a_parked_session_never_starts_a_container() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture.move_to(&app, SessionState::Parked).await;

    let (service, ended) = service_with_hook(&app);
    let closed = service.end(fixture.id()).await.expect("the session ends");

    assert_eq!(closed.state, SessionState::Done);
    assert!(
        app.engine().specs().is_empty(),
        "ending a parked session started a container",
    );

    let sync = last_event(&app, fixture.id(), "git").await;
    assert_eq!(sync["ok"], true, "{sync}");
    assert!(mirror_session_ref(&app, &fixture).await.is_some());

    assert_eq!(
        *ended.lock().expect("the hook lock is healthy"),
        vec![fixture.id()],
    );
}

/// A session nothing is launching — a `creating` row a crashed launch left
/// behind — closes with nothing to wait for (`ARCHITECTURE.md`, "Session
/// lifecycle", "A session ended while it is creating"; task `qhyhw`).
#[tokio::test]
async fn ending_a_creating_session_with_no_launch_closes_it_at_once() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    let (service, ended) = service_with_hook(&app);
    let closed = service.end(fixture.id()).await.expect("the session ends");

    assert_eq!(closed.state, SessionState::Done);
    assert!(closed.ended_at.is_some());
    assert!(
        app.engine().specs().is_empty(),
        "ending a creating session started a container",
    );

    let change = last_event(&app, fixture.id(), "state_change").await;
    assert_eq!(change["from"], "creating");
    assert_eq!(change["to"], "done");
    assert_eq!(change["reason"], "ended by user");

    assert_eq!(
        *ended.lock().expect("the hook lock is healthy"),
        vec![fixture.id()],
        "the end-of-session hook releases the tasks a creating session held",
    );
}

/// The leak this task exists to end: the end of a session that is still
/// `creating` used to be refused 409, and the launch it refused went on to
/// start a container nothing would remove (task `qhyhw`).
///
/// Holding the mock's `start` is what makes the window a moment the test
/// chooses: the launch has created its container and is inside the start, the
/// row still reads `creating`, and the end arrives there. The launch then
/// reads the cancellation at its next checkpoint, removes the container and
/// lets go of the session without ever reaching `running`.
#[tokio::test]
async fn ending_a_session_during_creating_cancels_its_launch_and_leaves_no_container() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    let engine = app.engine();
    engine.hold_starts();

    // A resume rather than a fresh launch, for the reason the other lifecycle
    // scenarios here use one: the fixture already has the work clone. The row
    // is `creating`, which is the state a launch is cancelled in.
    let launch = Launcher::from_state(&app.state)
        .launch(fixture.id(), LaunchMode::Resume)
        .expect("nothing else is launching this session");

    wait_until("the launch to create its container", || {
        !engine.specs().is_empty()
    })
    .await;
    let container_id = ContainerId(
        wait_for_container_id(&app, fixture.id())
            .await
            .expect("the launch records the container it created"),
    );
    assert_eq!(
        reload(&app, fixture.id()).await.state,
        SessionState::Creating,
        "the held start let the launch reach running",
    );

    let (service, ended) = service_with_hook(&app);
    let session_id = fixture.id();
    let ending = tokio::spawn(async move { service.end(session_id).await });

    let registry = app.session_registry();
    wait_until("the end to cancel the launch", || {
        registry.launch_cancelled(session_id)
    })
    .await;
    engine.resume_starts();

    let closed = ending
        .await
        .expect("the ending task does not panic")
        .expect("a creating session can be ended");
    launch.await.expect("the launch task does not panic");

    assert_eq!(closed.state, SessionState::Done);
    assert_eq!(closed.container_id, None, "the DTO still names a container");
    assert!(closed.ended_at.is_some());

    let row = reload(&app, fixture.id()).await;
    assert_eq!(row.state, SessionState::Done);
    assert_eq!(row.container_id, None);
    assert_eq!(
        engine.state_of(&container_id),
        None,
        "the cancelled launch left its container behind",
    );

    // The session never ran: the only state change it has is the end's own.
    let changes: Vec<String> = kinds(&app, fixture.id())
        .await
        .into_iter()
        .filter(|kind| kind == "state_change")
        .collect();
    assert_eq!(changes.len(), 1, "the session reached running after all");
    let change = last_event(&app, fixture.id(), "state_change").await;
    assert_eq!(change["from"], "creating");
    assert_eq!(change["to"], "done");

    assert_eq!(
        *ended.lock().expect("the hook lock is healthy"),
        vec![fixture.id()],
        "the end-of-session hook must run exactly once",
    );
    assert_eq!(app.session_registry().phase(fixture.id()), None);
    assert!(!app.session_registry().is_launching(fixture.id()));
}

#[tokio::test]
async fn a_retry_moves_a_failed_session_back_to_parked_and_clears_its_error() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture.move_to(&app, SessionState::Failed).await;
    assert!(reload(&app, fixture.id()).await.error.is_some());

    let parked = SessionService::new(&app.state)
        .retry(fixture.id(), None, Some(fixture.user_id))
        .await
        .expect("a failed conversational session is retried");

    assert_eq!(parked.state, SessionState::Parked);
    assert_eq!(parked.error, None, "the retry did not clear the error");
    assert_eq!(parked.ended_at, None);
    assert!(
        app.engine().specs().is_empty(),
        "a retry with no message launched a container",
    );

    let change = last_event(&app, fixture.id(), "state_change").await;
    assert_eq!(change["from"], "failed");
    assert_eq!(change["to"], "parked");
    assert_eq!(change["reason"], "retried by user");
}

#[tokio::test]
async fn a_retry_with_a_message_relaunches_the_session_at_once() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture.move_to(&app, SessionState::Failed).await;
    record_cli_session_id(&app, fixture.id()).await;

    SessionService::new(&app.state)
        .retry(
            fixture.id(),
            Some("try that again".to_string()),
            Some(fixture.user_id),
        )
        .await
        .expect("a failed conversational session is retried");

    let engine = Arc::clone(&app.engine);
    wait_until("the retry to create a container", || {
        !engine.specs().is_empty()
    })
    .await;
    wait_until("the retried session to reach running", || {
        app.session_registry().phase(fixture.id()) == Some(Phase::Running)
    })
    .await;

    let running = reload(&app, fixture.id()).await;
    assert_eq!(running.state, SessionState::Running);
    assert_eq!(running.error, None);
}

#[tokio::test]
async fn an_ephemeral_session_is_not_retried() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::of_kind(&app, ProfileKind::Ephemeral).await;
    fixture.move_to(&app, SessionState::Failed).await;

    let error = SessionService::new(&app.state)
        .retry(fixture.id(), None, Some(fixture.user_id))
        .await
        .expect_err("an ephemeral session is never retried");

    assert_eq!(
        conflict(error),
        "ephemeral sessions are not retried; launch a new one",
    );
    assert_eq!(reload(&app, fixture.id()).await.state, SessionState::Failed);
}

#[tokio::test]
async fn a_parked_session_that_is_not_failed_cannot_be_retried() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture.move_to(&app, SessionState::Parked).await;

    let error = SessionService::new(&app.state)
        .retry(fixture.id(), None, Some(fixture.user_id))
        .await
        .expect_err("only a failed session is retried");

    assert_eq!(
        conflict(error),
        "session is parked, only a failed session can be retried",
    );
}

#[tokio::test]
async fn a_sync_publishes_the_branch_and_records_the_event() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture.move_to(&app, SessionState::Parked).await;

    let synced = SessionService::new(&app.state)
        .sync(fixture.id())
        .await
        .expect("a parked session with a work tree syncs");

    assert_eq!(synced.git_ref, format!("refs/sessions/{}", fixture.id()));
    assert_eq!(
        Some(synced.commit.clone()),
        mirror_session_ref(&app, &fixture).await,
        "the mirror does not point at the commit the sync reported",
    );

    let sync = last_event(&app, fixture.id(), "git").await;
    assert_eq!(sync["op"], "sync");
    assert_eq!(sync["ok"], true, "{sync}");
    assert_eq!(sync["detail"]["commit"], synced.commit);
}

#[tokio::test]
async fn a_creating_session_cannot_be_synced() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;

    let error = SessionService::new(&app.state)
        .sync(fixture.id())
        .await
        .expect_err("a creating session has no work tree to sync");

    assert_eq!(conflict(error), "session is creating");
}

#[tokio::test]
async fn deleting_a_done_session_removes_its_directory_its_transcript_and_its_row() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture.move_to(&app, SessionState::Done).await;
    record_cli_session_id(&app, fixture.id()).await;

    // The CLI's own files: one transcript and the directory it keeps beside it.
    let layout = app.state.config.project_layout(fixture.project.id);
    let (transcript, directory) = layout
        .cli_transcript_paths(FAKE_CLI_SESSION_ID)
        .expect("the fixture id is a path component");
    tokio::fs::create_dir_all(&directory)
        .await
        .expect("the CLI transcript directory is made");
    tokio::fs::write(&transcript, b"{\"type\":\"system\"}\n")
        .await
        .expect("the CLI transcript is written");
    let session_directory: PathBuf =
        DataPaths::from_config(&app.state.config).session_dir(fixture.id());
    assert!(session_directory.is_dir(), "the fixture has no directory");

    SessionService::new(&app.state)
        .delete(fixture.id())
        .await
        .expect("a done session is deleted");

    assert!(!session_directory.exists(), "the session directory is left");
    assert!(!transcript.exists(), "the CLI transcript is left");
    assert!(!directory.exists(), "the CLI transcript directory is left");
    // The state directory itself is the project's and is never removed here.
    assert!(layout.claude_dir().is_dir());

    assert!(
        SessionRepository::new(&app.pool)
            .find(fixture.id())
            .await
            .expect("the lookup runs")
            .is_none(),
        "the session row is left",
    );
    assert!(
        kinds(&app, fixture.id()).await.is_empty(),
        "the session's events did not cascade",
    );
    assert_eq!(app.session_registry().phase(fixture.id()), None);
}

#[tokio::test]
async fn deleting_a_done_session_removes_the_container_it_still_records() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let container_id = start_container(&app, &fixture).await;
    fixture.move_to(&app, SessionState::Done).await;

    SessionService::new(&app.state)
        .delete(fixture.id())
        .await
        .expect("a done session is deleted");

    assert_eq!(
        app.engine().state_of(&container_id),
        None,
        "the recorded container was left behind",
    );
}

#[tokio::test]
async fn a_running_session_cannot_be_deleted() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture.move_to(&app, SessionState::Running).await;

    let error = SessionService::new(&app.state)
        .delete(fixture.id())
        .await
        .expect_err("a live session has to end first");

    assert_eq!(conflict(error), "session must be done or failed");
    assert!(
        DataPaths::from_config(&app.state.config)
            .session_dir(fixture.id())
            .is_dir(),
        "a refused delete removed the directory",
    );
    assert_eq!(
        reload(&app, fixture.id()).await.state,
        SessionState::Running
    );
}

/// The branch name the session model derives is the one the mirror publishes,
/// which is what ties a sync's `ref` to the work clone's branch.
#[tokio::test]
async fn the_published_ref_is_the_session_branch() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    fixture.move_to(&app, SessionState::Parked).await;

    let synced = SessionService::new(&app.state)
        .sync(fixture.id())
        .await
        .expect("the session syncs");

    let work = DataPaths::from_config(&app.state.config).session_work(fixture.id());
    let head = run_git(&work, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_string();
    let branch = run_git(&work, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .trim()
        .to_string();

    assert_eq!(branch, session_branch(fixture.id()));
    assert_eq!(synced.commit, head);
}
