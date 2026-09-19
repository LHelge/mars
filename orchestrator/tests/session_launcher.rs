//! The session launcher (`ARCHITECTURE.md`, "Launch sequence", "Session
//! container specification", "Storage", "Git model"; ADR 0029, ADR 0032;
//! `CLAUDE.md`, "Testing expectations").
//!
//! Every scenario here drives the real [`Launcher`] over a real project
//! repository — git is never mocked — with `MockEngine` recording the container
//! specification it was asked for. That recorded spec is what the "Session
//! container specification" assertions read: the table is a contract about
//! bytes the engine receives, so it is asserted field by field, in order, and
//! not through the builder that produced it (which has its own unit tests).
//!
//! The other half is the failure contract: a launch that cannot finish leaves
//! the session `failed` with a message an operator can act on, no container
//! behind it and no `container_id` on the row.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use common::TestApp;
use mars_orchestrator::engine::{
    ContainerEngine, ContainerId, ContainerInfo, ContainerSpec, ContainerSummary, EngineError,
    EngineKind, ExecSession, ExitStatus, LABEL_PROFILE_ID, LABEL_PROJECT_ID, LABEL_SESSION_ID,
    Signal, StdinWriter,
};
use mars_orchestrator::git::testutil::TestUpstream;
use mars_orchestrator::git::testutil::{run_git, test_identity};
use mars_orchestrator::git::{
    DataPaths, create_work_clone, init_project_repo, resolve_base, session_branch,
};
use mars_orchestrator::models::{
    AgentProfile, BranchName, NewAgentProfile, NewProject, NewSecret, NewSession, NewSharedDir,
    ProfileKind, Project, ProjectStatus, RemoteUrl, ScopeRef, Secret, SecretName, Session,
    SessionState, User,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{
    ProjectRepository, SecretRepository, SessionRepository, Transition,
};
use mars_orchestrator::secrets::{SealedSecret, SecretIdentity};
use mars_orchestrator::session::{
    BOTH_CREDENTIALS_ERROR, LaunchMode, Launcher, McpToken, Phase, SessionDirs, initial_token,
    write_mcp_json,
};
use serde_json::Value;
use uuid::Uuid;

/// Not a real remote: the fixture value every project test stores (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// Not a real image: the stub the session fixtures name.
const TEST_IMAGE: &str = "localhost/mars-session:test";

/// Everything one launch needs to exist: a user, a `ready` project with a real
/// repository, a profile and a `creating` session.
struct Fixture {
    upstream: TestUpstream,
    user: User,
    project: Project,
    profile: AgentProfile,
    session: Session,
    /// The token the creating route would have handed the launcher: the row
    /// carries its hash (ADR 0029). Taken by the launch that uses it, because a
    /// token is one launch's (ADR 0029) and the type holds no second copy.
    token: Option<McpToken>,
}

impl Fixture {
    /// A conversational session with no task and no declared secrets.
    async fn create(app: &TestApp) -> Self {
        Self::with(app, ProfileKind::Conversational, &[], None).await
    }

    /// A session of `kind` whose profile declares `secrets`, launched for
    /// `task_id`.
    async fn with(
        app: &TestApp,
        kind: ProfileKind,
        secrets: &[&str],
        task_id: Option<Uuid>,
    ) -> Self {
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

        let mut new_profile = NewAgentProfile::new(project.id, "default", TEST_IMAGE)
            .expect("the test profile is valid");
        new_profile.kind = kind;
        new_profile.secrets = secrets.iter().map(|name| (*name).to_string()).collect();
        new_profile
            .validate()
            .expect("the adjusted test profile is valid");

        let token = initial_token();
        let mut new_session = NewSession::new(
            project.id,
            new_profile.id,
            kind,
            "refs/heads/main",
            token.hash(),
        );
        new_session.created_by = Some(user.id);
        new_session.task_id = task_id;

        let mut tx = app.pool.begin().await.expect("a transaction begins");
        let profile = ProjectRepository::new(&app.pool)
            .insert_profile(&mut tx, &new_profile)
            .await
            .expect("the profile inserts");
        let session = SessionRepository::new(&app.pool)
            .insert(&mut tx, &new_session)
            .await
            .expect("the session inserts");
        tx.commit().await.expect("the transaction commits");

        Self {
            upstream,
            user,
            project,
            profile,
            session,
            token: Some(token),
        }
    }

    /// The fresh launch of this session, consuming its one token.
    fn fresh(&mut self) -> LaunchMode {
        LaunchMode::fresh(self.token.take().expect("the launch token is used once"))
    }

    /// This session's directory layout.
    fn dirs(&self, app: &TestApp) -> SessionDirs {
        SessionDirs::from_config(&app.state.config, self.session.id)
    }

    /// The branch a fresh launch checks out.
    fn branch(&self) -> String {
        session_branch(self.session.id)
    }
}

/// A `ready` project with a repository on disk, pointed at `upstream`.
///
/// Inserted `cloning` and moved to `ready` the way the clone job does it,
/// because `projects` refuses `ready` without a `default_branch`.
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

/// Seal `value` for `scope` and commit the row.
async fn seed_secret(app: &TestApp, scope: ScopeRef, raw_name: &str, value: &str) -> Secret {
    let secret_name = SecretName::parse(raw_name).expect("the test secret name is valid");
    let sealed = SealedSecret::seal(
        &app.state.keyring,
        SecretIdentity::new(&scope, &secret_name),
        value.as_bytes(),
    )
    .expect("the value seals");

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let inserted = SecretRepository::new(&app.pool)
        .insert(&mut tx, &NewSecret::new(sealed))
        .await
        .expect("the secret inserts");
    tx.commit().await.expect("the transaction commits");

    inserted
}

/// Run one launch to completion, through the launcher the handlers use.
async fn launch(app: &TestApp, session_id: Uuid, mode: LaunchMode) {
    launch_with(&app.state, session_id, mode).await;
}

/// The same, over a state whose engine a test has replaced.
async fn launch_with(state: &AppState, session_id: Uuid, mode: LaunchMode) {
    Launcher::from_state(state)
        .launch(session_id, mode)
        .expect("nothing else is launching this session")
        .await
        .expect("the launch task does not panic");
}

/// The session row as it is now.
async fn reload(app: &TestApp, session_id: Uuid) -> Session {
    SessionRepository::new(&app.pool)
        .get(session_id)
        .await
        .expect("the session is readable")
}

/// The project row as it is now.
async fn reload_project(app: &TestApp, project_id: Uuid) -> Project {
    ProjectRepository::new(&app.pool)
        .find(project_id)
        .await
        .expect("the lookup runs")
        .expect("the project exists")
}

/// Every event kind this session has, in order.
async fn kinds(app: &TestApp, session_id: Uuid) -> Vec<String> {
    sqlx::query_scalar::<_, String>("SELECT kind FROM events WHERE session_id = $1 ORDER BY seq")
        .bind(session_id)
        .fetch_all(&app.pool)
        .await
        .expect("the events are readable")
}

/// Every `launch_warning` message this session recorded, in order.
async fn warnings(app: &TestApp, session_id: Uuid) -> Vec<String> {
    sqlx::query_scalar::<_, Value>(
        "SELECT payload FROM events
         WHERE session_id = $1 AND kind = 'launch_warning' ORDER BY seq",
    )
    .bind(session_id)
    .fetch_all(&app.pool)
    .await
    .expect("the events are readable")
    .into_iter()
    .map(|payload| {
        payload["message"]
            .as_str()
            .expect("a launch warning carries a message")
            .to_string()
    })
    .collect()
}

/// The container specification the engine was asked for, which must be exactly
/// one.
fn recorded_spec(app: &TestApp) -> ContainerSpec {
    let mut specs = app.engine().specs();
    assert_eq!(specs.len(), 1, "one launch creates one container");

    specs.remove(0)
}

/// The `(container_target, read_only)` pairs of a spec's binds, in order.
fn bind_targets(spec: &ContainerSpec) -> Vec<(String, bool)> {
    spec.binds
        .iter()
        .map(|bind| (bind.container_target.clone(), bind.read_only))
        .collect()
}

/// The host path of a bind by its container target.
fn bind_source(spec: &ContainerSpec, target: &str) -> PathBuf {
    spec.binds
        .iter()
        .find(|bind| bind.container_target == target)
        .unwrap_or_else(|| panic!("{target} is bound"))
        .host_source
        .clone()
}

#[tokio::test]
async fn a_fresh_launch_prepares_the_session_and_runs_it() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app).await;
    let dirs = fixture.dirs(&app);

    launch(&app, fixture.session.id, fixture.fresh()).await;

    // The directories and the transcript the owner tails.
    assert!(dirs.home().is_dir(), "home was not created");
    assert!(dirs.log().is_dir(), "log was not created");
    assert!(dirs.stream_jsonl().is_file(), "the transcript is missing");
    let config = std::fs::read_to_string(dirs.mcp_json()).expect("mcp.json was written");
    assert!(config.contains(&app.state.config.mcp_url), "{config}");

    // The clone, on this session's branch, at the base's commit.
    let head = head_of(&dirs.work()).await;
    assert_eq!(head, fixture.branch(), "the work tree is on another branch");

    // The project's CLI state directory, which every session of it shares.
    assert!(
        app.state
            .config
            .project_layout(fixture.project.id)
            .claude_dir()
            .is_dir(),
        "the CLI state directory was not created",
    );

    // Running at the stdin attach, not at `init` (ADR 0032).
    let session = reload(&app, fixture.session.id).await;
    assert_eq!(session.state, SessionState::Running);
    let recorded = session
        .container_id
        .clone()
        .expect("a running session records its container");
    assert_eq!(
        app.engine()
            .container_id_for_session(fixture.session.id)
            .map(|id| id.0),
        Some(recorded),
        "the row's container id is not the one the engine created",
    );
    assert_eq!(kinds(&app, fixture.session.id).await, vec!["state_change"]);
    assert_eq!(
        app.session_registry().phase(fixture.session.id),
        Some(Phase::Running),
    );

    // The egress network is connected, and only after creation.
    let container_id = ContainerId(session.container_id.expect("a container id"));
    assert_eq!(
        app.engine().connections(&container_id),
        vec![app.state.config.session_network_egress.clone()],
    );
}

#[tokio::test]
async fn the_recorded_spec_is_the_documented_table() {
    let app = TestApp::spawn().await;
    let task_id = None;
    let mut fixture = Fixture::with(
        &app,
        ProfileKind::Conversational,
        &["DEPLOY_TOKEN"],
        task_id,
    )
    .await;
    seed_secret(
        &app,
        ScopeRef::project(fixture.project.id),
        "DEPLOY_TOKEN",
        "not-a-real-token",
    )
    .await;

    launch(&app, fixture.session.id, fixture.fresh()).await;

    let spec = recorded_spec(&app);
    let session_id = fixture.session.id;
    let project_id = fixture.project.id;
    let data_dir = app.state.config.data_dir.clone();

    assert_eq!(spec.image, TEST_IMAGE);
    assert_eq!(spec.name, format!("mars-session-{session_id}"));
    assert_eq!(
        spec.labels,
        BTreeMap::from([
            (LABEL_SESSION_ID.to_string(), session_id.to_string()),
            (LABEL_PROJECT_ID.to_string(), project_id.to_string()),
            (LABEL_PROFILE_ID.to_string(), fixture.profile.id.to_string()),
        ]),
    );
    assert_eq!(spec.user, "1000:1000");
    assert_eq!(spec.working_dir, "/session/work");
    assert_eq!(spec.runtime, None, "the profile names no runtime");
    assert_eq!(spec.network, app.state.config.session_network_internal);
    assert_eq!(spec.extra_hosts, app.state.config.session_extra_hosts);

    // The command is the backend's, and carries the MCP configuration this
    // launch wrote.
    assert_eq!(spec.cmd.first().map(String::as_str), Some("claude"));
    assert!(
        spec.cmd
            .windows(2)
            .any(|pair| pair == ["--mcp-config".to_string(), "/session/mcp.json".to_string()]),
        "{:?}",
        spec.cmd,
    );
    assert!(
        !spec.cmd.iter().any(|arg| arg == "--resume"),
        "a fresh launch does not resume: {:?}",
        spec.cmd,
    );

    // The fixed environment, in the documented order, with no `MARS_TASK_ID`
    // because this session was not launched for a task.
    let claude_config_dir = data_dir
        .join("projects")
        .join(project_id.to_string())
        .join("claude")
        .display()
        .to_string();
    assert_eq!(
        spec.env,
        vec![
            ("HOME".to_string(), "/session/home".to_string()),
            ("CLAUDE_CONFIG_DIR".to_string(), claude_config_dir.clone()),
            ("MARS_SESSION_ID".to_string(), session_id.to_string()),
            ("MARS_PROJECT_ID".to_string(), project_id.to_string()),
        ],
    );
    // The secrets are a field of their own, appended after the fixed
    // environment by the adapter, and there is exactly the one the profile
    // declared.
    assert_eq!(
        spec.secret_env
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["DEPLOY_TOKEN"],
    );

    // Every bind the table names, with its documented mode, and nothing else.
    let project_data = data_dir.join("projects").join(project_id.to_string());
    let expected: BTreeMap<String, bool> = BTreeMap::from([
        ("/session/work".to_string(), false),
        ("/session/home".to_string(), false),
        ("/session/log".to_string(), false),
        ("/session/mcp.json".to_string(), true),
        (path(&project_data.join("repo.git")), true),
        (path(&project_data.join("claude")), false),
    ]);
    assert_eq!(
        bind_targets(&spec).into_iter().collect::<BTreeMap<_, _>>(),
        expected,
    );
    // And no parent is mounted after something inside it, which is the ordering
    // rule the nesting depends on (`ARCHITECTURE.md`, "Engine adapter").
    assert_parents_first(&spec);

    // And every source is a host path under `DATA_DIR_HOST`.
    let session_host = app
        .state
        .config
        .data_dir_host
        .join("sessions")
        .join(session_id.to_string());
    assert_eq!(
        bind_source(&spec, "/session/work"),
        session_host.join("work")
    );
    assert_eq!(
        bind_source(&spec, "/session/mcp.json"),
        session_host.join("mcp.json"),
    );
}

#[tokio::test]
async fn a_session_launched_for_a_task_carries_mars_task_id() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app).await;
    let task_id = seed_task(&app, fixture.project.id).await;
    sqlx::query("UPDATE sessions SET task_id = $2 WHERE id = $1")
        .bind(fixture.session.id)
        .bind(task_id)
        .execute(&app.pool)
        .await
        .expect("the task id is set");

    launch(&app, fixture.session.id, fixture.fresh()).await;

    let spec = recorded_spec(&app);
    assert_eq!(
        spec.env.last(),
        Some(&("MARS_TASK_ID".to_string(), task_id.to_string())),
        "{:?}",
        spec.env,
    );
}

#[tokio::test]
async fn shared_directory_binds_are_ordered_parent_before_child() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app).await;

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    for (name, path) in [("target", "/session/work/target"), ("cache", "/opt/cache")] {
        ProjectRepository::new(&app.pool)
            .insert_shared_dir(
                &mut tx,
                fixture.project.id,
                &NewSharedDir::new(name, path).expect("the shared directory is valid"),
            )
            .await
            .expect("the shared directory inserts");
    }
    tx.commit().await.expect("the transaction commits");

    launch(&app, fixture.session.id, fixture.fresh()).await;

    let spec = recorded_spec(&app);
    let targets: Vec<String> = spec
        .binds
        .iter()
        .map(|bind| bind.container_target.clone())
        .collect();

    let work = targets
        .iter()
        .position(|target| target == "/session/work")
        .expect("the work tree is bound");
    let nested = targets
        .iter()
        .position(|target| target == "/session/work/target")
        .expect("the nested shared directory is bound");
    assert!(work < nested, "a parent must be mounted first: {targets:?}");

    // Both directories exist on disk, created lazily by this launch.
    let layout = app.state.config.project_layout(fixture.project.id);
    for name in ["target", "cache"] {
        assert!(
            layout
                .shared_dir(name)
                .expect("a valid shared directory name")
                .is_dir(),
            "{name} was not created",
        );
    }
}

#[tokio::test]
async fn a_recently_fetched_mirror_is_not_fetched_again_and_an_old_one_is() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app).await;

    // Five seconds old: inside the 30-second window, so this launch fetches
    // nothing and leaves the timestamp exactly as it was.
    let recent = set_last_fetched(&app, fixture.project.id, 5).await;
    launch(&app, fixture.session.id, fixture.fresh()).await;
    assert_eq!(
        reload_project(&app, fixture.project.id)
            .await
            .last_fetched_at,
        Some(recent),
        "a fetch five seconds old was repeated",
    );

    // A minute old: outside it, so the next launch of the project fetches and
    // records a new time.
    let stale = set_last_fetched(&app, fixture.project.id, 60).await;
    let mut second = Fixture::create(&app).await;
    // The second session belongs to the same project as the first.
    let session_id = reparent(&app, &second, &fixture).await;
    launch(&app, session_id, second.fresh()).await;

    let after = reload_project(&app, fixture.project.id)
        .await
        .last_fetched_at
        .expect("a fetch was recorded");
    assert!(after > stale, "a fetch a minute old was not repeated");
}

#[tokio::test]
async fn an_unreachable_upstream_warns_and_the_launch_proceeds() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app).await;

    // The remote the operator deleted, or the network that is down: the mirror
    // is whatever it already was.
    std::fs::remove_dir_all(&fixture.upstream.path).expect("the upstream is removed");

    launch(&app, fixture.session.id, fixture.fresh()).await;

    let session = reload(&app, fixture.session.id).await;
    assert_eq!(
        session.state,
        SessionState::Running,
        "a fetch failure must not fail the launch",
    );

    let recorded = warnings(&app, fixture.session.id).await;
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert!(
        recorded[0].starts_with("mirror fetch failed: "),
        "{}",
        recorded[0],
    );
    // The clone still happened, from the mirror as it is.
    assert_eq!(head_of(&fixture.dirs(&app).work()).await, fixture.branch());
}

#[tokio::test]
async fn a_missing_secret_is_a_warning_and_the_launch_proceeds() {
    let app = TestApp::spawn().await;
    let mut fixture =
        Fixture::with(&app, ProfileKind::Conversational, &["ABSENT_TOKEN"], None).await;

    launch(&app, fixture.session.id, fixture.fresh()).await;

    assert_eq!(
        warnings(&app, fixture.session.id).await,
        vec!["secret ABSENT_TOKEN is not defined at any scope"],
    );
    assert_eq!(
        reload(&app, fixture.session.id).await.state,
        SessionState::Running,
    );
    assert!(
        recorded_spec(&app).secret_env.is_empty(),
        "a missing secret is not injected",
    );
}

#[tokio::test]
async fn an_unresolvable_base_ref_fails_the_session() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app).await;
    sqlx::query("UPDATE sessions SET base_ref = $2 WHERE id = $1")
        .bind(fixture.session.id)
        .bind("refs/heads/nope")
        .execute(&app.pool)
        .await
        .expect("the base ref is set");

    launch(&app, fixture.session.id, fixture.fresh()).await;

    let session = reload(&app, fixture.session.id).await;
    assert_eq!(session.state, SessionState::Failed);
    assert_eq!(
        session.error.as_deref(),
        Some("base ref 'refs/heads/nope' does not resolve"),
    );
    assert!(
        app.engine().specs().is_empty(),
        "no container is created for a launch that failed before the engine",
    );
    assert_eq!(app.session_registry().phase(fixture.session.id), None);
}

#[tokio::test]
async fn both_model_credentials_fail_the_session() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::with(
        &app,
        ProfileKind::Conversational,
        &["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"],
        None,
    )
    .await;
    seed_secret(
        &app,
        ScopeRef::global(),
        "ANTHROPIC_API_KEY",
        "not-a-real-api-key",
    )
    .await;
    seed_secret(
        &app,
        ScopeRef::user(fixture.user.id),
        "CLAUDE_CODE_OAUTH_TOKEN",
        "not-a-real-oauth-token",
    )
    .await;

    launch(&app, fixture.session.id, fixture.fresh()).await;

    let session = reload(&app, fixture.session.id).await;
    assert_eq!(session.state, SessionState::Failed);
    assert_eq!(session.error.as_deref(), Some(BOTH_CREDENTIALS_ERROR));
    assert!(app.engine().specs().is_empty(), "no container was created");
}

#[tokio::test]
async fn a_pull_failure_fails_the_session_with_the_engines_message() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app).await;

    app.engine().set_missing_images([TEST_IMAGE]);
    app.engine().fail_next_pull("manifest unknown");

    launch(&app, fixture.session.id, fixture.fresh()).await;

    let session = reload(&app, fixture.session.id).await;
    assert_eq!(session.state, SessionState::Failed);
    assert_eq!(
        session.error.as_deref(),
        Some("image pull failed: manifest unknown"),
    );
    assert_eq!(session.container_id, None);
    assert!(app.engine().specs().is_empty(), "no container was created");
}

#[tokio::test]
async fn a_failure_after_the_create_removes_the_container_and_clears_the_row() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app).await;

    // A start that refuses is the one failure after the create that the mock
    // has no way to arrange on its own.
    let mut state = app.state.clone();
    state.engine = Arc::new(RefusingStart::new(Arc::clone(&app.state.engine)));

    launch_with(&state, fixture.session.id, fixture.fresh()).await;

    let session = reload(&app, fixture.session.id).await;
    assert_eq!(session.state, SessionState::Failed);
    assert_eq!(
        session.error.as_deref(),
        Some(
            format!(
                "container start failed: the container engine reports a conflict: {}",
                RefusingStart::MESSAGE
            )
            .as_str()
        ),
    );
    assert_eq!(
        session.container_id, None,
        "the container id of a failed launch is cleared",
    );
    // Created, then removed: nothing carrying this session's label is left.
    assert_eq!(app.engine().specs().len(), 1, "the container was created");
    assert_eq!(
        app.engine()
            .container_id_for_session(fixture.session.id)
            .map(|id| id.0),
        None,
        "the container of a failed launch was not removed",
    );
}

#[tokio::test]
async fn a_resume_rotates_the_token_skips_git_and_resumes_the_cli() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let dirs = fixture.dirs(&app);

    // What a first launch left behind: the directories, the config, the clone
    // and a session that has spoken and then parked.
    dirs.ensure().await.expect("the directories are made");
    write_mcp_json(
        &dirs,
        &app.state.config.mcp_url,
        fixture.token.as_ref().expect("the fixture's token"),
    )
    .await
    .expect("the first config is written");
    clone_by_hand(&app, &fixture).await;
    let before_config = std::fs::read_to_string(dirs.mcp_json()).expect("mcp.json is readable");
    let before_hash = reload(&app, fixture.session.id).await.mcp_token_hash;

    set_cli_session_id(&app, fixture.session.id, "fake-cli-session-id").await;
    transition(
        &app,
        fixture.session.id,
        Transition::new(
            SessionState::Creating,
            SessionState::Running,
            "container started and stdin attached",
        ),
    )
    .await;
    transition(
        &app,
        fixture.session.id,
        Transition::new(SessionState::Running, SessionState::Parked, "idle"),
    )
    .await;
    let fetched_before = reload_project(&app, fixture.project.id)
        .await
        .last_fetched_at;
    let head_before = commit_of_head(&dirs.work()).await;

    launch(&app, fixture.session.id, LaunchMode::Resume).await;

    let session = reload(&app, fixture.session.id).await;
    assert_eq!(session.state, SessionState::Running);
    assert_ne!(
        session.mcp_token_hash, before_hash,
        "a resume launches a fresh token (ADR 0029)",
    );
    let after_config = std::fs::read_to_string(dirs.mcp_json()).expect("mcp.json is readable");
    assert_ne!(after_config, before_config, "mcp.json was not rewritten");

    // No git: the mirror was not fetched and the checkout was left alone.
    assert_eq!(
        reload_project(&app, fixture.project.id)
            .await
            .last_fetched_at,
        fetched_before,
        "a resume does not fetch the mirror",
    );
    assert_eq!(commit_of_head(&dirs.work()).await, head_before);

    let spec = recorded_spec(&app);
    assert!(
        spec.cmd
            .windows(2)
            .any(|pair| pair == ["--resume".to_string(), "fake-cli-session-id".to_string()]),
        "{:?}",
        spec.cmd,
    );
}

#[tokio::test]
async fn a_resume_of_a_session_that_never_spoke_relaunches_in_its_checkout() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let dirs = fixture.dirs(&app);

    dirs.ensure().await.expect("the directories are made");
    clone_by_hand(&app, &fixture).await;
    let marker = dirs.work().join("agent-was-here.txt");
    std::fs::write(&marker, "kept\n").expect("the marker is written");

    transition(
        &app,
        fixture.session.id,
        Transition::new(
            SessionState::Creating,
            SessionState::Running,
            "container started and stdin attached",
        ),
    )
    .await;
    transition(
        &app,
        fixture.session.id,
        Transition::new(SessionState::Running, SessionState::Parked, "idle"),
    )
    .await;

    launch(&app, fixture.session.id, LaunchMode::Resume).await;

    let session = reload(&app, fixture.session.id).await;
    assert_eq!(
        session.state,
        SessionState::Running,
        "a missing cli_session_id is not a failure (ADR 0032)",
    );
    assert!(
        marker.is_file(),
        "the existing checkout was replaced rather than reused",
    );
    let spec = recorded_spec(&app);
    assert!(
        !spec.cmd.iter().any(|arg| arg == "--resume"),
        "there is no conversation to resume: {:?}",
        spec.cmd,
    );
}

#[tokio::test]
async fn a_resume_whose_work_tree_is_gone_clones_again() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app).await;
    let dirs = fixture.dirs(&app);

    // The retry of a session that failed during creation: the row is there and
    // the checkout is not.
    dirs.ensure().await.expect("the directories are made");
    transition(
        &app,
        fixture.session.id,
        Transition::new(
            SessionState::Creating,
            SessionState::Failed,
            "launch failed",
        ),
    )
    .await;
    transition(
        &app,
        fixture.session.id,
        Transition::new(SessionState::Failed, SessionState::Parked, "retry"),
    )
    .await;

    launch(&app, fixture.session.id, LaunchMode::Resume).await;

    assert_eq!(
        reload(&app, fixture.session.id).await.state,
        SessionState::Running,
    );
    assert_eq!(
        head_of(&dirs.work()).await,
        fixture.branch(),
        "the fresh git sequence did not run",
    );
}

#[tokio::test]
async fn a_stale_container_of_a_previous_attempt_is_removed_before_the_create() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app).await;

    // What a crash between a create and a remove leaves: a container holding
    // this session's name and label.
    let stale = app
        .engine()
        .create(&stale_spec(fixture.session.id, fixture.project.id))
        .await
        .expect("the stale container is created");

    launch(&app, fixture.session.id, fixture.fresh()).await;

    assert_eq!(
        reload(&app, fixture.session.id).await.state,
        SessionState::Running,
        "the stale name was not cleared before the create",
    );
    assert!(
        app.engine().state_of(&stale).is_none(),
        "the stale container was not removed",
    );
}

#[tokio::test]
async fn a_second_launch_of_the_same_session_is_refused_by_the_guard() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app).await;

    let guard = app
        .session_registry()
        .try_begin_launch(fixture.session.id)
        .expect("the first claim succeeds");

    assert!(
        Launcher::from_state(&app.state)
            .launch(fixture.session.id, fixture.fresh())
            .is_none(),
        "a second launch must not start while one is under way",
    );
    assert_eq!(
        reload(&app, fixture.session.id).await.state,
        SessionState::Creating,
    );

    drop(guard);
}

#[tokio::test]
async fn an_input_that_queues_during_the_launch_is_delivered_when_stdin_attaches() {
    use mars_orchestrator::events::SessionInput;
    use mars_orchestrator::session::{QueuedInput, SubmitResult};

    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app).await;

    // Registered the way the creating route does, so the input has somewhere to
    // wait before the launcher takes over the entry.
    let _rx = app.session_registry().register(
        fixture.session.id,
        ProfileKind::Conversational,
        Phase::Creating,
    );
    let queued = app.session_registry().submit(
        fixture.session.id,
        QueuedInput {
            input: SessionInput::Message {
                text: "hello from the queue".to_string(),
            },
            user_id: Some(fixture.user.id),
            client_id: None,
            accepted_at: chrono::Utc::now(),
        },
    );
    assert_eq!(queued, SubmitResult::Queued);

    launch(&app, fixture.session.id, fixture.fresh()).await;

    assert_eq!(
        app.session_registry().queued(fixture.session.id),
        0,
        "the queue was not drained at the stdin attach",
    );
    assert_eq!(
        reload(&app, fixture.session.id).await.state,
        SessionState::Running,
    );
}

/// A path as the specification spells it.
fn path(of: &std::path::Path) -> String {
    of.display().to_string()
}

/// Assert that no bind precedes a bind it is nested inside.
fn assert_parents_first(spec: &ContainerSpec) {
    let targets: Vec<&str> = spec
        .binds
        .iter()
        .map(|bind| bind.container_target.as_str())
        .collect();

    for (index, target) in targets.iter().enumerate() {
        for (other_index, other) in targets.iter().enumerate() {
            if other_index > index && target.starts_with(&format!("{other}/")) {
                panic!("{other} is mounted after {target}: {targets:?}");
            }
        }
    }
}

/// A `queue` state and one task in it, so a session can be launched for a task.
///
/// Raw statements: what is under test is the environment the launcher builds,
/// and the tracker's own repositories have their own suites.
async fn seed_task(app: &TestApp, project_id: Uuid) -> Uuid {
    let state_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO task_states (id, project_id, name, kind, position)
         VALUES ($1, $2, 'todo', 'queue', 0)",
    )
    .bind(state_id)
    .bind(project_id)
    .execute(&app.pool)
    .await
    .expect("the task state seeds");

    let task_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tasks (id, project_id, number, title, state_id)
         VALUES ($1, $2, 1, 'a fake task', $3)",
    )
    .bind(task_id)
    .bind(project_id)
    .bind(state_id)
    .execute(&app.pool)
    .await
    .expect("the task seeds");

    task_id
}

/// `git rev-parse --abbrev-ref HEAD` in `work`.
async fn head_of(work: &std::path::Path) -> String {
    run_git(work, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .trim()
        .to_string()
}

/// The commit `HEAD` names in `work`.
async fn commit_of_head(work: &std::path::Path) -> String {
    run_git(work, &["rev-parse", "HEAD"])
        .await
        .trim()
        .to_string()
}

/// Clone the session's work tree the way a first launch would have.
async fn clone_by_hand(app: &TestApp, fixture: &Fixture) {
    let paths = DataPaths::from_config(&app.state.config);
    let guard = app.state.git_locks.lock(fixture.project.id).await;
    let base = resolve_base(&guard, &paths, Some(&fixture.session.base_ref), "main")
        .await
        .expect("the base resolves");

    create_work_clone(&guard, &paths, fixture.session.id, &base, &test_identity())
        .await
        .expect("the work clone is created");
}

/// Write one transition the way its owner would.
async fn transition(app: &TestApp, session_id: Uuid, change: Transition<'_>) {
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .transition(&mut tx, session_id, &change)
        .await
        .expect("the session transitions");
    tx.commit().await.expect("the transaction commits");
}

/// Record the CLI's own session id, which an `init` would have.
async fn set_cli_session_id(app: &TestApp, session_id: Uuid, cli_session_id: &str) {
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .set_cli_session_id(&mut tx, session_id, cli_session_id)
        .await
        .expect("the cli session id is recorded");
    tx.commit().await.expect("the transaction commits");
}

/// Backdate `projects.last_fetched_at` by `secs` and answer what was written.
async fn set_last_fetched(
    app: &TestApp,
    project_id: Uuid,
    secs: i64,
) -> chrono::DateTime<chrono::Utc> {
    sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
        "UPDATE projects SET last_fetched_at = now() - make_interval(secs => $2::double precision)
         WHERE id = $1 RETURNING last_fetched_at",
    )
    .bind(project_id)
    .bind(secs as f64)
    .fetch_one(&app.pool)
    .await
    .expect("the fetch time is backdated")
}

/// Move `second`'s session onto `first`'s project and profile, so two launches
/// share one mirror.
async fn reparent(app: &TestApp, second: &Fixture, first: &Fixture) -> Uuid {
    sqlx::query("UPDATE sessions SET project_id = $2, profile_id = $3 WHERE id = $1")
        .bind(second.session.id)
        .bind(first.project.id)
        .bind(first.profile.id)
        .execute(&app.pool)
        .await
        .expect("the session is reparented");

    second.session.id
}

/// The container a crashed launch of `session_id` would have left behind: the
/// same name and the same label, and nothing else that matters.
fn stale_spec(session_id: Uuid, project_id: Uuid) -> ContainerSpec {
    ContainerSpec {
        image: TEST_IMAGE.to_string(),
        name: format!("mars-session-{session_id}"),
        labels: BTreeMap::from([
            (LABEL_SESSION_ID.to_string(), session_id.to_string()),
            (LABEL_PROJECT_ID.to_string(), project_id.to_string()),
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
    }
}

/// `MockEngine` with a `start` that refuses, which is the one failure between a
/// create and a running container the mock cannot arrange on its own.
type EngineResult<T> = std::result::Result<T, EngineError>;

struct RefusingStart {
    inner: Arc<dyn ContainerEngine>,
}

impl RefusingStart {
    /// What the refused start says, and therefore what `sessions.error` ends
    /// with.
    const MESSAGE: &'static str = "start refused";

    fn new(inner: Arc<dyn ContainerEngine>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ContainerEngine for RefusingStart {
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

    async fn start(&self, _id: &ContainerId) -> EngineResult<()> {
        Err(EngineError::Conflict(Self::MESSAGE.to_string()))
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

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
