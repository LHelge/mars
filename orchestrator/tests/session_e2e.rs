//! The session lifecycle end to end, on a real engine and the stub session
//! image (`ARCHITECTURE.md`, "Session image", "Launch sequence", "Stop
//! semantics", "Restart procedure", "Session container specification").
//!
//! Everything else in the suite runs the lifecycle over `MockEngine`, which
//! records the container it would have created. This binary runs it over the
//! engine `DOCKER_HOST` names, with real containers, real bind mounts, a real
//! `mcp.json` and a real CLI process — the stub's credential-free replay of
//! `images/stub/fixtures/default.jsonl` — so what is asserted here is the part
//! no mock can stand in for: that the specification the launcher builds is one
//! the engine accepts, that a `SIGINT` reaches a process that handles it, that
//! the transcript the owner tails is the file the container wrote, and that a
//! restart adopts the container a lost owner left behind.
//!
//! **When it runs.** With `DOCKER_HOST` unset every scenario returns before it
//! touches anything, as `tests/engine.rs` does (`CLAUDE.md`, "Testing
//! expectations"). With it set, the image named by `MARS_STUB_IMAGE` has to be
//! there: an absent image fails the run with the build command rather than
//! skipping quietly, because a silent skip here would mean the lifecycle was
//! never exercised on a container at all.
//!
//! **The uid contract decides whether there is anything to run.** A session
//! container writes into `DATA_DIR` as uid 1000 and the orchestrator reads what
//! it wrote, so these scenarios need the mapping `ARCHITECTURE.md`, "Uid
//! contract" describes: rootless Podman honours it through
//! `keep-id:uid=1000,gid=1000`, and Docker honours it only when the
//! orchestrator is itself uid 1000. On a host where it does not hold the
//! startup probe refuses — which is the documented behaviour, asserted by
//! `tests/engine.rs::bootstrap_engine_end_to_end` — and there is no session to
//! run: this binary says so on stderr and returns.
//!
//! **What is expected of a turn comes from the fixture**, parsed here out of
//! `images/stub/fixtures/default.jsonl`: its tool names, its assistant text
//! and its cumulative costs. Nothing below hard-codes a count or a price, so
//! re-recording the fixture cannot leave this file asserting yesterday's
//! transcript.
//!
//! Every scenario drives the HTTP API through `axum-test`, so the path taken is
//! the user's, and cleans up after itself: the containers of its own sessions
//! and the two networks its app created, whether it passed or panicked. Every
//! credential-shaped value is an obviously fake stand-in (rule 3).

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use bollard::query_parameters::InspectContainerOptions;
use chrono::{Duration as ChronoDuration, Utc};
use common::{AuthenticatedUser, TestApp};
use futures_util::FutureExt;
use mars_orchestrator::cron::{CronService, JobName, JobReport};
use mars_orchestrator::engine::bollard::BollardEngine;
use mars_orchestrator::engine::{
    ContainerEngine, EngineKind, LABEL_PROFILE_ID, LABEL_PROJECT_ID, LABEL_SESSION_ID,
};
use mars_orchestrator::git::DataPaths;
use mars_orchestrator::git::testutil::run_git;
use mars_orchestrator::models::{Session, SessionState};
use mars_orchestrator::projects::clone_job;
use mars_orchestrator::repositories::SessionRepository;
use mars_orchestrator::session::{LAUNCHED_REASON, SessionDirs};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use uuid::Uuid;

// ---- guards ----

/// The image the scenarios run, overridable for CI, where each engine builds
/// its own tag.
///
/// The default is the tag the documented development build produces
/// (`README.md`, "Development"), registry-qualified because Podman resolves an
/// unqualified name against its configured registries and would go looking for
/// this one on a registry.
const DEFAULT_STUB_IMAGE: &str = "localhost/mars-session-stub:dev";

/// How long a clone, a launch, a turn or a stop may take before a scenario
/// gives up with a message instead of hanging.
///
/// Generous: the whole budget is a real engine's create-and-start latency plus
/// a Python process replaying a 150 KiB transcript.
const PATIENCE: Duration = Duration::from_secs(30);

/// How often a wait re-reads what it is waiting for.
const POLL: Duration = Duration::from_millis(250);

/// The uid the session container runs as (`ARCHITECTURE.md`, "Uid contract").
const SESSION_UID: u32 = 1000;

/// What the scheduled profile below is told to do, and so the prompt its run
/// is launched with.
const SCHEDULE_PROMPT: &str = "scan the repository for tech debt and file tasks";

/// `MARS_STUB_IMAGE`, or [`DEFAULT_STUB_IMAGE`].
fn stub_image() -> String {
    std::env::var("MARS_STUB_IMAGE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_STUB_IMAGE.to_string())
}

/// An app over the real engine, or `None` when there is nothing to run here.
///
/// The two reasons to return `None` are different in kind and are reported
/// differently. No `DOCKER_HOST` is an absent engine and the ordinary state of
/// a `cargo test` run on a machine with no container engine. A host that does
/// not honour the uid contract *has* an engine and cannot run a session on it,
/// which is louder: the line names both uids, because the number that is
/// surprising is not the same on both engines.
///
/// An image that is not there is neither: it is the one thing the caller can
/// fix in a command, so it panics naming that command.
async fn spawn_or_skip() -> Option<TestApp> {
    let Some(host) = common::engine::docker_host() else {
        eprintln!("DOCKER_HOST not set; skipping the session end-to-end tests");
        return None;
    };

    let engine = BollardEngine::connect(&host)
        .await
        .expect("the engine named by DOCKER_HOST answers");
    let image = stub_image();
    assert!(
        engine
            .image_exists(&image)
            .await
            .expect("the engine answers whether the stub image is present"),
        "the stub image {image} is not on this engine; build it with \
         `podman build -t {DEFAULT_STUB_IMAGE} images/stub` (or set MARS_STUB_IMAGE)",
    );

    let own_uid = own_uid();
    if engine.kind() == EngineKind::Docker && own_uid != SESSION_UID {
        eprintln!(
            "this host does not honour the uid contract — the engine is Docker and the \
             orchestrator runs as uid {own_uid}, not {SESSION_UID} — so a session container \
             cannot write into DATA_DIR and the startup probe refuses; skipping the session \
             end-to-end tests (tests/engine.rs::bootstrap_engine_end_to_end asserts that \
             refusal)"
        );
        return None;
    }

    Some(TestApp::spawn_with_engine(&image).await)
}

/// The uid this process runs as, read the way the startup probe reads it: off a
/// directory the process created itself.
fn own_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;

    let dir = tempfile::tempdir().expect("a temporary directory");
    std::fs::metadata(dir.path())
        .expect("the directory is there")
        .uid()
}

/// Run a scenario over a real-engine app and clean up after it whatever it did.
///
/// A `Drop` guard cannot await, so the clean-up is a wrapper: the body is
/// caught, this app's containers and networks are removed, and only then is a
/// panic resumed. A clean-up failure is itself a failure — a leftover
/// `mars-session-*` container or `mars-e2e-*` network is a bug in the suite —
/// but only when the body passed, because a body that panicked has the more
/// interesting message.
async fn scenario<F, Fut>(body: F)
where
    F: FnOnce(Arc<TestApp>) -> Fut,
    Fut: Future<Output = ()>,
{
    let Some(app) = spawn_or_skip().await else {
        return;
    };
    let app = Arc::new(app);

    let outcome = AssertUnwindSafe(body(Arc::clone(&app)))
        .catch_unwind()
        .await;
    let failures = app.cleanup_engine().await;

    match outcome {
        Err(panic) => {
            for failure in failures {
                eprintln!("cleanup after a failed scenario: {failure}");
            }
            std::panic::resume_unwind(panic)
        }
        Ok(()) => assert!(failures.is_empty(), "the scenario leaked: {failures:?}"),
    }
}

// ---- the fixture the stub replays ----

/// `images/stub/fixtures/default.jsonl`, split into turns.
///
/// The source of every expectation about a replayed turn. It is read from the
/// repository rather than restated here, so the fixture and the assertions
/// cannot drift: a re-recording changes both at once (`images/stub/fixtures/README.md`).
struct StubFixture {
    /// One entry per turn, in replay order; a turn ends at its `result`.
    turns: Vec<Turn>,
}

/// What one replayed turn contains, in the terms the transcript is asserted in.
struct Turn {
    /// The `name` of every `tool_use` block, in order, subagent frames
    /// included.
    tools: Vec<String>,
    /// The last complete assistant text block, which is the reply a reader
    /// sees.
    last_text: String,
    /// `result.total_cost_usd`, which the pinned CLI reports cumulatively
    /// (`ARCHITECTURE.md`, "Cost accounting").
    cumulative_cost: f64,
}

impl StubFixture {
    /// Parse the shipped fixture.
    fn load() -> Self {
        let path = fixture_path();
        let text = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!("the stub fixture at {} is readable: {err}", path.display())
        });

        let mut turns = Vec::new();
        let mut tools = Vec::new();
        let mut last_text = String::new();

        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let native: Value = serde_json::from_str(line).expect("a fixture line is JSON");
            match native["type"].as_str() {
                // Partial frames carry no block the transcript is asserted on.
                Some("stream_event") => {}
                Some("assistant") => {
                    for block in native["message"]["content"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                    {
                        match block["type"].as_str() {
                            Some("tool_use") => tools.push(
                                block["name"]
                                    .as_str()
                                    .expect("a tool_use block names its tool")
                                    .to_string(),
                            ),
                            Some("text") => {
                                last_text = block["text"]
                                    .as_str()
                                    .expect("a text block carries text")
                                    .to_string();
                            }
                            _ => {}
                        }
                    }
                }
                Some("result") => {
                    turns.push(Turn {
                        tools: std::mem::take(&mut tools),
                        last_text: std::mem::take(&mut last_text),
                        cumulative_cost: native["total_cost_usd"]
                            .as_f64()
                            .expect("a result reports a cumulative cost"),
                    });
                }
                _ => {}
            }
        }

        assert!(
            turns.len() > 1,
            "the stub fixture should hold several turns: {}",
            path.display()
        );
        Self { turns }
    }

    /// The `n`-th turn, counting from one as the fixture's README does.
    fn turn(&self, n: usize) -> &Turn {
        self.turns
            .get(n - 1)
            .unwrap_or_else(|| panic!("the fixture has no turn {n}"))
    }
}

/// `images/stub/fixtures/default.jsonl`, from this crate's manifest directory.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("images")
        .join("stub")
        .join("fixtures")
        .join("default.jsonl")
}

// ---- arrangement ----

/// An obviously fake password of the length `POST /api/test/users` requires
/// (rule 3).
fn password(name: &str) -> String {
    format!("fake-password-{name}")
}

/// Eight hex characters, so parallel scenarios never collide on a name.
fn suffix() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// A signed-in user, a `ready` project over a local bare repository, and the
/// project's seeded default profile — whose image is `SESSION_IMAGE_DEFAULT`,
/// which is the stub.
struct Fixture {
    /// The upstream the project was cloned from, held for the length of the
    /// scenario: a launch may fetch the mirror again, and a remote that has
    /// been removed under it would turn into a `launch_warning` nothing here is
    /// about.
    _upstream: common::git::BareFixture,
    user: AuthenticatedUser,
    project_id: Uuid,
    profile_id: Uuid,
}

impl Fixture {
    async fn create(app: &TestApp) -> Self {
        let name = format!("user-{}", suffix());
        let user = app
            .create_user(&name, &format!("{name}@example.test"), &password(&name))
            .await;

        // A real bare repository in a `tempfile` directory of its own: git is
        // never mocked (`CLAUDE.md`, "Testing expectations").
        let upstream = common::git::BareFixture::new();
        let response = app
            .post_as(&user, "/api/projects")
            .json(&json!({
                "name": format!("project-{}", suffix()),
                "remote_url": format!("file://{}", upstream.path().display()),
                "default_branch": "main",
            }))
            .await;
        response.assert_status(StatusCode::CREATED);
        let project_id = id_of(&response.json::<Value>());

        let cloned = clone_job::wait_for_clone(&app.state, project_id, PATIENCE).await;
        assert_eq!(
            cloned.status,
            mars_orchestrator::models::ProjectStatus::Ready,
            "the fixture clone failed: {:?}",
            cloned.status_message,
        );
        let response = app
            .get_as(&user, &format!("/api/projects/{project_id}/profiles"))
            .await;
        response.assert_status_ok();
        let profiles = response.json::<Vec<Value>>();
        let default = profiles
            .iter()
            .find(|profile| profile["is_default"] == json!(true))
            .expect("a project is seeded with a default profile");
        assert_eq!(
            default["image"],
            json!(app.state.config.session_image_default),
            "the default profile should run the configured session image",
        );

        Self {
            _upstream: upstream,
            user,
            project_id,
            profile_id: id_of(default),
        }
    }

    /// An ephemeral profile of this project, on the same image.
    async fn ephemeral_profile(&self, app: &TestApp) -> Uuid {
        let response = app
            .post_as(
                &self.user,
                &format!("/api/projects/{}/profiles", self.project_id),
            )
            .json(&json!({ "name": format!("implementer-{}", suffix()), "kind": "ephemeral" }))
            .await;
        response.assert_status(StatusCode::CREATED);

        id_of(&response.json::<Value>())
    }

    /// An `auto_launch` ephemeral profile of this project, serving `states`.
    ///
    /// The image is the project's default, which
    /// [`TestApp::spawn_with_engine`] set to the stub, so a session this
    /// profile launches runs the same replay as every other scenario here.
    async fn auto_profile(&self, app: &TestApp, states: &[&str]) -> Uuid {
        let response = app
            .post_as(
                &self.user,
                &format!("/api/projects/{}/profiles", self.project_id),
            )
            .json(&json!({
                "name": format!("auto-{}", suffix()),
                "kind": "ephemeral",
                "auto_launch": true,
                "max_concurrent": 1,
                "serves_states": states,
            }))
            .await;
        response.assert_status(StatusCode::CREATED);

        id_of(&response.json::<Value>())
    }

    /// A scheduled ephemeral profile of this project, on the same image.
    ///
    /// The prompt is what every run of it is asked to do, and the stub image
    /// replays its fixture whatever the prompt says (`images/stub/claude`), so
    /// what it is for here is being the argument the launch is asserted to
    /// carry.
    async fn scheduled_profile(&self, app: &TestApp, cron: &str) -> Uuid {
        let response = app
            .post_as(
                &self.user,
                &format!("/api/projects/{}/profiles", self.project_id),
            )
            .json(&json!({
                "name": format!("scheduled-{}", suffix()),
                "kind": "ephemeral",
                "max_concurrent": 1,
                "schedule_cron": cron,
                "schedule_prompt": SCHEDULE_PROMPT,
            }))
            .await;
        response.assert_status(StatusCode::CREATED);

        id_of(&response.json::<Value>())
    }

    /// Store this project's agent credential, which an `auto_launch` profile
    /// is refused at save without (`SPEC.md`, "Agent profiles"; ADR 0036).
    ///
    /// A user's own credential does not qualify: an unattended launch has no
    /// user to resolve one for. The value authenticates nothing — the stub
    /// image never calls a model (rule 3).
    async fn store_agent_credential(&self, app: &TestApp) {
        let response = app
            .post_as(&self.user, "/api/secrets")
            .json(&json!({
                "scope": "project",
                "scope_id": self.project_id.to_string(),
                "name": "CLAUDE_CODE_OAUTH_TOKEN",
                "value": "fake-value-not-a-credential",
            }))
            .await;
        response.assert_status(StatusCode::CREATED);
    }

    /// `POST /api/projects/{pid}/tasks`, answering the task's number.
    async fn create_task(&self, app: &TestApp, title: &str, state: &str) -> i64 {
        let response = app
            .post_as(
                &self.user,
                &format!("/api/projects/{}/tasks", self.project_id),
            )
            .json(&json!({ "title": title, "state": state }))
            .await;
        response.assert_status(StatusCode::CREATED);

        response.json::<Value>()["number"]
            .as_i64()
            .expect("a task carries a number")
    }

    /// Move a task into another state, as the board's drawer does.
    async fn move_task(&self, app: &TestApp, number: i64, state: &str) {
        let response = app
            .put_as(
                &self.user,
                &format!("/api/projects/{}/tasks/{number}", self.project_id),
            )
            .json(&json!({ "state": state }))
            .await;
        response.assert_status_ok();
    }

    /// `GET /api/projects/{pid}/tasks/{number}`.
    async fn read_task(&self, app: &TestApp, number: i64) -> Value {
        let response = app
            .get_as(
                &self.user,
                &format!("/api/projects/{}/tasks/{number}", self.project_id),
            )
            .await;
        response.assert_status_ok();

        response.json::<Value>()
    }

    /// Every session of this project, as the project page lists them.
    async fn sessions(&self, app: &TestApp) -> Vec<Value> {
        let response = app
            .get_as(
                &self.user,
                &format!("/api/projects/{}/sessions", self.project_id),
            )
            .await;
        response.assert_status_ok();

        response.json::<Vec<Value>>()
    }

    /// `POST /api/projects/{pid}/sessions`, asserting 201.
    async fn create_session(&self, app: &TestApp, body: &Value) -> Uuid {
        let response = app
            .post_as(
                &self.user,
                &format!("/api/projects/{}/sessions", self.project_id),
            )
            .json(body)
            .await;
        response.assert_status(StatusCode::CREATED);

        id_of(&response.json::<Value>())
    }
}

/// The `id` of a resource body, as a [`Uuid`].
fn id_of(body: &Value) -> Uuid {
    body["id"]
        .as_str()
        .expect("a resource carries an id")
        .parse()
        .expect("the id is a uuid")
}

// ---- waits ----

/// Wait until `GET /api/sessions/{id}` reports `state`, reading it every
/// [`POLL`].
///
/// The read is the client's, not the database's, so what the scenario waits for
/// is what a user would see. The failure message carries the state and the
/// error the session actually has, which is what makes a launch failure
/// readable.
async fn wait_for_state(app: &TestApp, fixture: &Fixture, id: Uuid, state: SessionState) -> Value {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    loop {
        let response = app
            .get_as(&fixture.user, &format!("/api/sessions/{id}"))
            .await;
        response.assert_status_ok();
        let body = response.json::<Value>();

        if body["state"] == json!(state.as_str()) {
            return body;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "the session did not reach {state} within {PATIENCE:?}: {} {}",
            body["state"],
            body["error"],
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Wait until the session's state has stopped changing, and answer it.
///
/// For the restart scenario, whose claim is that a state does *not* change: a
/// process the restart had ended would take the owner a moment to notice, so
/// `running` read once straight after recovery proves nothing. "At rest" is one
/// state read twice a [`POLL`] apart, which is several times the interval the
/// owner's own loop and drain run at.
async fn wait_for_settled_state(app: &TestApp, id: Uuid) -> SessionState {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let mut last = reload(app, id).await.state;

    loop {
        tokio::time::sleep(POLL).await;
        let current = reload(app, id).await.state;

        if current == last {
            return current;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session was still changing state after {PATIENCE:?}: {current}",
        );
        last = current;
    }
}

/// Wait until the session has at least `count` events of `kind`, and answer the
/// whole transcript as the API renders it.
async fn wait_for_events(
    app: &TestApp,
    fixture: &Fixture,
    id: Uuid,
    kind: &str,
    count: usize,
) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    loop {
        let events = transcript(app, fixture, id).await;
        if events.iter().filter(|event| event["kind"] == kind).count() >= count {
            return events;
        }

        if tokio::time::Instant::now() >= deadline {
            // The states the session went through and what it says went wrong:
            // without them a missing event is a count and not a diagnosis.
            let row = reload(app, id).await;
            panic!(
                "fewer than {count} {kind} events within {PATIENCE:?}: {:?}\n\
                 the session is {} ({:?}) and its state changes were {:?}",
                kinds(&events),
                row.state,
                row.error,
                of_kind(&events, "state_change"),
            );
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Every event of the session, oldest first, through
/// `GET /api/sessions/{id}/events`.
///
/// One page of 500 is enough for these scenarios: a replayed turn of the stub
/// fixture is a few dozen events. A transcript that outgrows it fails the
/// assertion below rather than silently truncating.
async fn transcript(app: &TestApp, fixture: &Fixture, id: Uuid) -> Vec<Value> {
    let response = app
        .get_as(
            &fixture.user,
            &format!("/api/sessions/{id}/events?limit=500"),
        )
        .await;
    response.assert_status_ok();
    let page = response.json::<Value>();

    assert_eq!(
        page["has_more"],
        json!(false),
        "the scenario's transcript outgrew one page",
    );
    page["events"]
        .as_array()
        .expect("an event page carries events")
        .clone()
}

/// The `kind` of every event, for a failure message.
fn kinds(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| event["kind"].as_str().unwrap_or("?").to_string())
        .collect()
}

/// The events of `kind`, in order.
fn of_kind<'a>(events: &'a [Value], kind: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event["kind"] == kind)
        .collect()
}

/// The index of the first event of `kind` at or after `from`.
fn position_of(events: &[Value], kind: &str, from: usize) -> usize {
    events
        .iter()
        .enumerate()
        .skip(from)
        .find(|(_, event)| event["kind"] == kind)
        .map(|(index, _)| index)
        .unwrap_or_else(|| panic!("no {kind} event after {from}: {:?}", kinds(events)))
}

/// `seq` runs 1, 2, 3 … with no gap and no repeat (`SPEC.md`,
/// "AgentEventBase").
fn assert_contiguous_seq(events: &[Value]) {
    let seqs: Vec<i64> = events
        .iter()
        .map(|event| event["seq"].as_i64().expect("an event carries a seq"))
        .collect();
    let expected: Vec<i64> = (1..=seqs.len() as i64).collect();

    assert_eq!(seqs, expected, "the sequence has a gap or a repeat");
}

/// The session row as it is now, which is where the columns no DTO carries —
/// `mcp_token_hash` above all — are read.
async fn reload(app: &TestApp, id: Uuid) -> Session {
    SessionRepository::new(&app.pool)
        .get(id)
        .await
        .expect("the session is readable")
}

// ---- the container, through a client of the suite's own ----

/// The container `mars-session-<sid>` as the engine describes it.
///
/// [`ContainerEngine`] deliberately exposes only what Mars performs, and the
/// command and the binds are not among them, so the two assertions that are
/// about the *specification* go through a raw client, as
/// `tests/common/engine.rs` does for the operations Mars never performs.
///
/// Asked again for a short while when the answer cannot be decoded: the Podman
/// 4 series reports a container between running and exited as `stopped`, which
/// the typed response refuses, and an ephemeral session's container is in that
/// window for a moment (the adapter's own listing retries for the same reason).
async fn inspect(id: Uuid) -> bollard::models::ContainerInspectResponse {
    let docker = common::engine::raw_docker();
    let name = format!("mars-session-{id}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    loop {
        match docker
            .inspect_container(&name, None::<InspectContainerOptions>)
            .await
        {
            Ok(container) => return container,
            Err(bollard::errors::Error::JsonDataError { .. })
                if tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(err) => panic!("the session container of {id} is inspectable: {err}"),
        }
    }
}

/// Whether the engine still has a container for this session.
async fn container_exists(app: &TestApp, id: Uuid) -> bool {
    let label = id.to_string();

    app.state
        .engine
        .list_by_label(LABEL_SESSION_ID)
        .await
        .expect("the engine lists its session containers")
        .iter()
        .any(|summary| summary.labels.get(LABEL_SESSION_ID) == Some(&label))
}

/// Wait until the session's row carries a container id.
///
/// What a scenario that wants to look at the container itself waits for.
/// Waiting for `running` instead is a race for a session that can *finish*:
/// an ephemeral run on the stub image is over in well under a poll interval,
/// so the state read after it can be `done` and `running` never comes. The
/// column is set at launch and cleared only when the container is discarded,
/// well after `done` (see [`wait_for_discarded_container`]), so it is the wide
/// window the narrow one was standing in for.
async fn wait_for_container(app: &TestApp, id: Uuid) {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    loop {
        let session = reload(app, id).await;
        if session.container_id.is_some() {
            return;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "the session had no container after {PATIENCE:?}: {} {:?}",
            session.state,
            session.error,
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Wait until the session has no container left on the engine and none on its
/// row (`docs/data-model.md`, `sessions.container_id`).
///
/// A wait rather than a read: the owner and the session service both remove the
/// container and clear the column *after* the state change a scenario waited
/// for, so a row read the moment a session goes `done` can still carry one.
async fn wait_for_discarded_container(app: &TestApp, id: Uuid) {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    loop {
        let recorded = reload(app, id).await.container_id;
        if recorded.is_none() && !container_exists(app, id).await {
            return;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "the container of {id} was still there after {PATIENCE:?}: {recorded:?}",
        );
        tokio::time::sleep(POLL).await;
    }
}

/// The labels of an inspect response.
fn labels(container: &bollard::models::ContainerInspectResponse) -> BTreeMap<String, String> {
    container
        .config
        .as_ref()
        .and_then(|config| config.labels.clone())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// The `Cmd` of an inspect response: the launch command the backend built.
fn cmd(container: &bollard::models::ContainerInspectResponse) -> Vec<String> {
    container
        .config
        .as_ref()
        .and_then(|config| config.cmd.clone())
        .unwrap_or_default()
}

/// The `Binds` of an inspect response.
fn binds(container: &bollard::models::ContainerInspectResponse) -> Vec<String> {
    container
        .host_config
        .as_ref()
        .and_then(|host| host.binds.clone())
        .unwrap_or_default()
}

/// The `Authorization` header `mcp.json` carries, which is the session's MCP
/// bearer token.
///
/// Read rather than compared as bytes, so a rotation assertion says the token
/// changed and not merely that the file did.
fn mcp_authorization(app: &TestApp, id: Uuid) -> String {
    let path = SessionDirs::from_config(&app.state.config, id).mcp_json();
    let document: Value = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|err| panic!("{} is readable: {err}", path.display())),
    )
    .expect("mcp.json is JSON");

    document["mcpServers"]["mars-orchestrator"]["headers"]["Authorization"]
        .as_str()
        .expect("mcp.json carries the Authorization header")
        .to_string()
}

/// The commit `refs/sessions/<sid>` points at in the project mirror, or `None`.
async fn mirror_session_ref(app: &TestApp, fixture: &Fixture, id: Uuid) -> Option<String> {
    let repo = DataPaths::from_config(&app.state.config).project_repo(fixture.project_id);
    let listed = run_git(
        &repo,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            &format!("refs/sessions/{id}"),
        ],
    )
    .await;

    let commit = listed.trim().to_string();
    (!commit.is_empty()).then_some(commit)
}

// ---- scenarios ----

/// The whole conversational lifecycle on one container per launch: running on
/// the stdin attach, a replayed turn, a stop that parks it, a resume that
/// rotates its token and delivers the queued message, and an end that fetches
/// the branch back.
///
/// One test rather than six, because every step of it is the state the next one
/// starts from and a real container per step would be six launches of the same
/// session (`ARCHITECTURE.md`, "Session lifecycle").
#[tokio::test]
async fn a_conversational_session_runs_replays_parks_resumes_and_ends() {
    scenario(|app| async move {
        let fixture = StubFixture::load();
        let project = Fixture::create(&app).await;
        let id = project
            .create_session(&app, &json!({ "profile_id": project.profile_id }))
            .await;

        // ---- running on the stdin attach (ADR 0032) ----
        let body = wait_for_state(&app, &project, id, SessionState::Running).await;
        assert_eq!(
            body["cli_session_id"],
            Value::Null,
            "the pinned CLI writes nothing until its first stdin line, so there is no id yet",
        );
        let row = reload(&app, id).await;
        let container_id = row
            .container_id
            .clone()
            .expect("a running session records its container");

        let launched = transcript(&app, &project, id).await;
        let state_changes = of_kind(&launched, "state_change");
        let first = state_changes
            .first()
            .expect("the launch wrote a state_change");
        assert_eq!(first["from"], json!("creating"));
        assert_eq!(first["to"], json!("running"));
        assert_eq!(first["reason"], json!(LAUNCHED_REASON));
        assert!(
            of_kind(&launched, "init").is_empty(),
            "a conversational launch produces no init before its first message: {:?}",
            kinds(&launched),
        );

        // The specification table, as the engine took it
        // (`ARCHITECTURE.md`, "Session container specification").
        let container = inspect(id).await;
        assert_eq!(
            container.id.as_deref(),
            Some(container_id.as_str()),
            "the container named mars-session-<sid> is the one on the row",
        );
        let labels = labels(&container);
        assert_eq!(labels.get(LABEL_SESSION_ID), Some(&id.to_string()));
        assert_eq!(
            labels.get(LABEL_PROJECT_ID),
            Some(&project.project_id.to_string())
        );
        assert_eq!(
            labels.get(LABEL_PROFILE_ID),
            Some(&project.profile_id.to_string())
        );
        let mcp_json_host = SessionDirs::from_config(&app.state.config, id).mcp_json_host();
        // `<source>:/session/mcp.json:ro`, possibly with the mount options the
        // engine adds of its own accord after the `ro` (Podman appends
        // `rprivate,nosuid,nodev,rbind`).
        assert!(
            binds(&container).iter().any(|bind| {
                bind.starts_with(&format!("{}:/session/mcp.json:ro", mcp_json_host.display()))
            }),
            "the read-only mcp.json bind is missing: {:?}",
            binds(&container),
        );
        assert!(
            cmd(&container)
                .windows(2)
                .any(|pair| pair == ["--input-format", "stream-json"]),
            "a conversational launch reads its messages from stdin: {:?}",
            cmd(&container),
        );

        // ---- the message and the turn it replays ----
        let response = app
            .post_as(&project.user, &format!("/api/sessions/{id}/input"))
            .json(&json!({ "kind": "message", "text": "please read the files" }))
            .await;
        response.assert_status(StatusCode::ACCEPTED);

        let events = wait_for_events(&app, &project, id, "result", 1).await;
        assert_contiguous_seq(&events);

        let init = of_kind(&events, "init");
        let init = init.first().expect("the first turn opens with an init");
        assert_eq!(init["resumed"], json!(false));
        let cli_session_id = init["cli_session_id"]
            .as_str()
            .expect("init names the CLI session")
            .to_string();
        assert_eq!(
            reload(&app, id).await.cli_session_id.as_deref(),
            Some(cli_session_id.as_str()),
            "the owner records the CLI session id from init",
        );

        let turn = fixture.turn(1);
        assert_eq!(
            of_kind(&events, "tool_call")
                .iter()
                .map(|event| event["name"].as_str().unwrap_or("?").to_string())
                .collect::<Vec<_>>(),
            turn.tools,
            "the replayed turn is the fixture's first turn",
        );
        assert_eq!(
            of_kind(&events, "tool_result").len(),
            turn.tools.len(),
            "every tool call of the turn was answered: {:?}",
            kinds(&events),
        );
        assert_eq!(
            of_kind(&events, "text")
                .last()
                .map(|event| event["text"].clone()),
            Some(json!(turn.last_text)),
            "the turn's closing text is the fixture's",
        );

        let result = of_kind(&events, "result");
        let result = result.first().expect("the turn ended with a result");
        assert_eq!(result["cost_usd"], json!(turn.cumulative_cost));
        let after_turn = reload(&app, id).await;
        assert_eq!(
            after_turn.cost_usd, turn.cumulative_cost,
            "the session's cost is the turn's, accumulated from a zero baseline",
        );
        assert!(after_turn.cost_usd > 0.0);
        assert!(after_turn.input_tokens > 0 && after_turn.output_tokens > 0);

        // ---- the stop, which parks a conversational session ----
        let token_before = mcp_authorization(&app, id);
        let hash_before = after_turn.mcp_token_hash.clone();

        let response = app
            .post_as(&project.user, &format!("/api/sessions/{id}/stop"))
            .await;
        response.assert_status(StatusCode::ACCEPTED);

        wait_for_state(&app, &project, id, SessionState::Parked).await;
        let parked = transcript(&app, &project, id).await;
        let into_parked = of_kind(&parked, "state_change")
            .into_iter()
            .find(|event| event["to"] == json!("parked"))
            .expect("the stop wrote a state_change into parked");
        assert_eq!(into_parked["from"], json!("running"));
        assert_eq!(
            into_parked["signal"],
            json!("SIGINT"),
            "the stub exits on the SIGINT, so no SIGTERM is ever sent",
        );

        // A parked session has no container, on the engine or on its row.
        wait_for_discarded_container(&app, id).await;

        // ---- the resume, with a rotated token and the queued message ----
        let response = app
            .post_as(&project.user, &format!("/api/sessions/{id}/input"))
            .json(&json!({ "kind": "message", "text": "and now the second turn" }))
            .await;
        response.assert_status(StatusCode::ACCEPTED);

        wait_for_state(&app, &project, id, SessionState::Running).await;
        let resumed_row = reload(&app, id).await;
        assert_ne!(
            resumed_row.mcp_token_hash, hash_before,
            "a resume rotates the session's MCP token (ADR 0029)",
        );
        assert_ne!(
            mcp_authorization(&app, id),
            token_before,
            "the rotated token reached mcp.json",
        );
        assert!(
            cmd(&inspect(id).await)
                .windows(2)
                .any(|pair| pair == ["--resume", cli_session_id.as_str()]),
            "a resume continues the recorded CLI conversation",
        );

        let events = wait_for_events(&app, &project, id, "result", 2).await;
        assert_contiguous_seq(&events);

        let inits = of_kind(&events, "init");
        assert_eq!(inits.len(), 2, "each launch produces one init");
        assert_eq!(
            inits[1]["resumed"],
            json!(true),
            "the second launch resumed the conversation",
        );

        // The queued message was delivered, and delivered before the turn it
        // triggered: the `user_message` is recorded before anything is written
        // to stdin (ADR 0020).
        let message_at = events
            .iter()
            .position(|event| {
                event["kind"] == json!("user_message")
                    && event["text"] == json!("and now the second turn")
            })
            .unwrap_or_else(|| {
                panic!("the queued message was not delivered: {:?}", kinds(&events))
            });
        assert!(
            message_at < position_of(&events, "result", message_at),
            "the queued message should precede the turn it replayed",
        );

        // ---- the end, which fetches the branch back ----
        let response = app
            .post_as(&project.user, &format!("/api/sessions/{id}/end"))
            .await;
        response.assert_status_ok();
        let body = response.json::<Value>();
        assert_eq!(body["state"], json!("done"), "{body}");
        assert_eq!(body["container_id"], Value::Null);

        let row = reload(&app, id).await;
        assert_eq!(row.state, SessionState::Done);
        wait_for_discarded_container(&app, id).await;

        let published = mirror_session_ref(&app, &project, id)
            .await
            .expect("the mirror has the session ref");
        let ended = transcript(&app, &project, id).await;
        let sync = of_kind(&ended, "git")
            .into_iter()
            .find(|event| event["op"] == json!("sync"))
            .expect("the end recorded its fetch-back");
        assert_eq!(sync["ok"], json!(true), "{sync}");
        assert_eq!(sync["detail"]["ref"], json!(format!("refs/sessions/{id}")));
        assert_eq!(sync["detail"]["commit"], json!(published));
    })
    .await;
}

/// An ephemeral session: one `-p` run of the whole fixture, no input accepted,
/// `done` with its cost accumulated and no container left (ADR 0003).
#[tokio::test]
async fn an_ephemeral_session_replays_its_prompt_and_finishes_done() {
    scenario(|app| async move {
        let fixture = StubFixture::load();
        let project = Fixture::create(&app).await;
        let profile_id = project.ephemeral_profile(&app).await;

        let id = project
            .create_session(
                &app,
                &json!({ "profile_id": profile_id, "message": "implement the thing" }),
            )
            .await;

        // The argv, read while the container is still there: a one-shot launch
        // takes its prompt as an argument and never attaches an input format
        // (`ARCHITECTURE.md`, "Claude Code invocation").
        wait_for_container(&app, id).await;
        let cmd = cmd(&inspect(id).await);
        assert!(
            cmd.iter().any(|part| part == "-p"),
            "an ephemeral launch is a one-shot run: {cmd:?}",
        );
        assert!(
            cmd.iter().any(|part| part == "implement the thing"),
            "the prompt is an argument of the command: {cmd:?}",
        );
        assert!(
            !cmd.iter().any(|part| part == "--input-format"),
            "an ephemeral session has no stdin to read: {cmd:?}",
        );

        // An ephemeral session takes no input in any state (ADR 0003).
        let response = app
            .post_as(&project.user, &format!("/api/sessions/{id}/input"))
            .json(&json!({ "kind": "message", "text": "one more thing" }))
            .await;
        response.assert_status(StatusCode::CONFLICT);
        response.assert_json(&json!({
            "status": 409,
            "error": "ephemeral sessions accept no input",
        }));

        wait_for_state(&app, &project, id, SessionState::Done).await;
        let events = transcript(&app, &project, id).await;
        assert_contiguous_seq(&events);
        // One turn, not the fixture's three: an ephemeral session's first
        // `result` ends the run outright, whatever the CLI would have gone on
        // to write (`ARCHITECTURE.md`, "Claude Code invocation"; ADR 0003).
        assert_eq!(
            of_kind(&events, "result").len(),
            1,
            "the first result of a one-shot run ends it: {:?}",
            kinds(&events),
        );
        assert!(
            of_kind(&events, "user_message").is_empty(),
            "an ephemeral session's prompt is an argument, not an input: {:?}",
            kinds(&events),
        );
        assert_eq!(
            of_kind(&events, "tool_call")
                .iter()
                .map(|event| event["name"].as_str().unwrap_or("?").to_string())
                .collect::<Vec<_>>(),
            fixture.turn(1).tools,
            "the turn that ran is the fixture's first",
        );

        let row = reload(&app, id).await;
        assert_eq!(
            row.cost_usd,
            fixture.turn(1).cumulative_cost,
            "the cost of the turn that ran is the session's",
        );
        assert!(row.input_tokens > 0 && row.output_tokens > 0);
        wait_for_discarded_container(&app, id).await;
    })
    .await;
}

/// The dispatcher's own loop and its waker, started as `CronService::start`
/// starts them and stopped the way the drain does (`tests/cron_dispatcher.rs`,
/// "the wake-up").
///
/// Only the dispatcher: a scenario that wants a job running on its timer wants
/// that job and not the other six, and the idle reaper in particular would
/// park the very session under test.
struct RunningDispatcher {
    shutdown: watch::Sender<bool>,
    handles: Vec<JoinHandle<()>>,
}

impl RunningDispatcher {
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

/// The whole dispatcher loop on real containers: a task moved into a state an
/// `auto_launch` profile serves runs the stub image to `done` with nobody
/// asking, and gives the lease back on the way out (`ARCHITECTURE.md`, "Task
/// tracker" → "Unattended launches" and "Dispatcher"; ADR 0042).
///
/// `tests/cron_dispatcher.rs` is the job's own suite and covers which profile,
/// which task, whether now and what a refusal does — over `MockEngine`, which
/// records the container it *would* have created. What is left for here is the
/// part no mock can stand in for, and it is the whole point of the feature:
/// that the specification an unattended launch builds is one a real engine
/// accepts, that the CLI it names really runs, and that the session ends and
/// releases its task without a person anywhere in the loop.
///
/// **Nothing is launched by hand.** The only calls this scenario makes are a
/// profile, a task and a state change — the three things a user does on the
/// board. `POST /api/projects/{pid}/sessions` is never reached, which is
/// asserted through `launch_source` and `created_by` on the row that appears.
///
/// **What the stub cannot do.** It replays a fixture and calls no MCP tool at
/// all (`images/stub/claude`), so the dispatched agent never hands the task
/// on, never comments and never moves it: the task comes back to the state it
/// was claimed from. That is what is asserted — one attempt, the lease gone,
/// the state unchanged — and it is the release path a finished ephemeral
/// session takes whatever the agent did (`ARCHITECTURE.md`, "Task tracker" →
/// "Liveness comes from the session, not from tool calls"). A hand-off would
/// need an agent that speaks MCP, which is `tests/mcp_workflow.rs`'s subject.
///
/// **The job is stopped the moment its session exists**, before that session
/// finishes. Not tidiness: the released task lands back in the served state
/// and a running dispatcher would launch it again, and again, until the
/// project's `max_attempts` escalated it. One launch is what is under test, so
/// the loop is stopped after it.
#[tokio::test]
async fn a_task_moved_into_a_served_state_is_dispatched_run_and_released() {
    scenario(|app| async move {
        let fixture = StubFixture::load();
        let project = Fixture::create(&app).await;
        project.store_agent_credential(&app).await;
        let profile_id = project.auto_profile(&app, &["ready"]).await;

        // Out of reach to begin with: `backlog` is not served, so the job's
        // startup tick has nothing to do and the launch below is provably the
        // move's doing.
        let number = project
            .create_task(&app, "Fix the login form", "backlog")
            .await;

        let dispatcher = RunningDispatcher::start(&app);

        project.move_task(&app, number, "ready").await;

        // The `task_events` notice wakes the job within its debounce
        // (`cron::dispatcher::WAKE_DEBOUNCE`, 250 ms); the suite's interval is
        // 45 seconds, so nothing that appears inside [`PATIENCE`] is a tick.
        let launched = wait_for_dispatched_session(&app, &project).await;
        let id = id_of(&launched);
        dispatcher.stop().await;

        // Nobody asked for it, so there is nobody to attribute it to, and
        // `launch_source` is the record of which job did (`SPEC.md`,
        // "Sessions").
        assert_eq!(launched["created_by"], Value::Null);
        assert_eq!(launched["profile_id"], json!(profile_id.to_string()));
        assert_eq!(launched["kind"], json!("ephemeral"));
        // The generated task message is the session's title and the head of
        // its `-p` prompt (`SPEC.md`, "Sessions").
        assert_eq!(launched["title"], json!("Fix the login form"));

        // It really is a one-shot run of the stub, on this engine: the prompt
        // is an argument and no stdin format is attached.
        wait_for_container(&app, id).await;
        let cmd = cmd(&inspect(id).await);
        assert!(
            cmd.windows(2)
                .any(|pair| pair[0] == "-p" && pair[1].contains("Fix the login form")),
            "the generated task message is not the prompt: {cmd:?}",
        );
        assert!(
            !cmd.iter().any(|part| part == "--input-format"),
            "an unattended launch is ephemeral and has no stdin: {cmd:?}",
        );

        // ---- and it runs to the end on its own ----
        wait_for_state(&app, &project, id, SessionState::Done).await;
        let events = transcript(&app, &project, id).await;
        assert_contiguous_seq(&events);
        assert_eq!(
            of_kind(&events, "tool_call")
                .iter()
                .map(|event| event["name"].as_str().unwrap_or("?").to_string())
                .collect::<Vec<_>>(),
            fixture.turn(1).tools,
            "the turn that ran is the fixture's first",
        );
        assert_eq!(
            reload(&app, id).await.cost_usd,
            fixture.turn(1).cumulative_cost,
        );
        wait_for_discarded_container(&app, id).await;

        // ---- the lease goes back through the v1 paths ----
        // A finished ephemeral session ends in `AppState::session_ended`, and
        // the tracker hook installed there releases what it held. Nothing in
        // this scenario asked for that either.
        let released = wait_for_released_task(&app, &project, number).await;
        assert_eq!(
            released["attempts"],
            json!(1),
            "the claim the launch made is the one attempt on the record",
        );
        assert_eq!(
            released["state"],
            json!("ready"),
            "the stub hands nothing off, so the task comes back where it was",
        );
        assert!(
            released["sessions"]
                .as_array()
                .expect("a task lists the sessions that touched it")
                .iter()
                .any(|touch| touch["session_id"] == json!(id.to_string())),
            "the dispatched session is not on the task's record: {}",
            released["sessions"],
        );

        // One launch, and no second one after the release.
        let all = project.sessions(&app).await;
        assert_eq!(all.len(), 1, "something else was launched: {all:?}");
    })
    .await;
}

/// A profile's cron expression coming due runs a real container to `done`
/// (`ARCHITECTURE.md`, "Task tracker" → "Scheduled agents"; `SPEC.md`,
/// "User-facing features" → "Scheduled agents"; ADR 0043).
///
/// `tests/cron_scheduler.rs` is the job's own suite and covers every rule about
/// *when* a tick is due, over `MockEngine`. What is left for here is the same
/// part the dispatcher scenario above covers for its own job, and it is the
/// whole point of the feature: that the specification a scheduled launch
/// builds is one a real engine accepts, that the CLI it names really runs with
/// the schedule's prompt as its argument, and that the session ends by itself
/// with no person and no task anywhere in the loop.
///
/// **Nothing is launched by hand.** The only calls are a profile and one run of
/// the job. `POST /api/projects/{pid}/sessions` is never reached, which is
/// asserted through `launch_source` and `created_by` on the row that appears.
///
/// **The two instants are the scenario's.** The job takes its `now` from the
/// caller and its process-start floor from [`CronService::with_started_at`], so
/// a window two minutes wide makes `* * * * *` due at once and nothing here
/// waits for a minute boundary. The same two instants a second time are what
/// shows the tick was spent by the first run.
///
/// No job loop is started, so the one run below is the only run there is: a
/// timer would fire the profile again every minute for as long as the scenario
/// lasted.
#[tokio::test]
async fn a_due_schedule_runs_an_ephemeral_session_on_the_stub_image() {
    scenario(|app| async move {
        let fixture = StubFixture::load();
        let project = Fixture::create(&app).await;
        project.store_agent_credential(&app).await;
        let profile_id = project.scheduled_profile(&app, "* * * * *").await;

        // A window that ends now and opens two minutes ago: every minute has
        // an occurrence in it, so the tick is due on the spot.
        let now = Utc::now();
        let service =
            CronService::with_started_at(app.state.clone(), now - ChronoDuration::minutes(2));
        let report = service
            .run_once(JobName::Scheduler, now)
            .await
            .expect("the sweep runs");
        assert_eq!(
            report,
            JobReport {
                items: 1,
                ..JobReport::default()
            },
            "the due tick did not launch",
        );

        let sessions = project.sessions(&app).await;
        let [launched] = sessions.as_slice() else {
            panic!("one session was launched, not {}", sessions.len());
        };
        // Nobody asked for it, and it holds nothing: a scheduled run is a
        // task-less ephemeral launch (`SPEC.md`, "Sessions").
        assert_eq!(launched["launch_source"], json!("schedule"));
        assert_eq!(launched["created_by"], Value::Null);
        assert_eq!(launched["task_id"], Value::Null);
        assert_eq!(launched["kind"], json!("ephemeral"));
        assert_eq!(launched["profile_id"], json!(profile_id.to_string()));
        let id = id_of(launched);

        // It really is a one-shot run of the stub on this engine, and what it
        // was given is the schedule's prompt.
        wait_for_container(&app, id).await;
        let cmd = cmd(&inspect(id).await);
        assert!(
            cmd.windows(2)
                .any(|pair| pair[0] == "-p" && pair[1] == SCHEDULE_PROMPT),
            "the schedule prompt is not the argument of the run: {cmd:?}",
        );
        assert!(
            !cmd.iter().any(|part| part == "--input-format"),
            "a scheduled launch is ephemeral and has no stdin: {cmd:?}",
        );

        // ---- and it runs to the end on its own ----
        wait_for_state(&app, &project, id, SessionState::Done).await;
        let events = transcript(&app, &project, id).await;
        assert_contiguous_seq(&events);
        assert_eq!(
            of_kind(&events, "tool_call")
                .iter()
                .map(|event| event["name"].as_str().unwrap_or("?").to_string())
                .collect::<Vec<_>>(),
            fixture.turn(1).tools,
            "the turn that ran is the fixture's first",
        );
        assert_eq!(
            reload(&app, id).await.cost_usd,
            fixture.turn(1).cumulative_cost,
        );
        wait_for_discarded_container(&app, id).await;

        // ---- and the tick it fired is spent ----
        // The claim is committed before the launch, so the same window a
        // second time — which a restart inside the tick's own minute is —
        // finds nothing to do.
        assert_eq!(
            service
                .run_once(JobName::Scheduler, now)
                .await
                .expect("the second sweep runs"),
            JobReport::default(),
        );
        let all = project.sessions(&app).await;
        assert_eq!(all.len(), 1, "the tick fired twice: {all:?}");
    })
    .await;
}

/// Wait until the project has a session the dispatcher launched, and answer it.
///
/// `launch_source` is what it waits on rather than "any session": it is the
/// one field that says a job and not a person created the row, and a scenario
/// that asserted on a count could not tell the two apart (`SPEC.md`,
/// "Sessions").
async fn wait_for_dispatched_session(app: &TestApp, fixture: &Fixture) -> Value {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    loop {
        let sessions = fixture.sessions(app).await;
        if let Some(session) = sessions
            .iter()
            .find(|session| session["launch_source"] == json!("dispatcher"))
        {
            return session.clone();
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "nothing was dispatched within {PATIENCE:?}: {sessions:?}",
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Wait until nobody holds the task any more, and answer it.
///
/// The hook runs after the session's own transaction has committed, so a task
/// read the moment a session goes `done` can still be held.
async fn wait_for_released_task(app: &TestApp, fixture: &Fixture, number: i64) -> Value {
    let deadline = tokio::time::Instant::now() + PATIENCE;

    loop {
        let task = fixture.read_task(app, number).await;
        if task["lease_holder_session_id"] == Value::Null {
            return task;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "the finished session still held its task after {PATIENCE:?}: {task}",
        );
        tokio::time::sleep(POLL).await;
    }
}

/// A restart adopts the session's container, keeps its token and goes on
/// delivering input to the *same process*, with no duplicated events
/// (`ARCHITECTURE.md`, "Restart procedure", "Durability and recovery").
///
/// Losing the owners closes the connection the last process wrote stdin
/// through. That connection is an exec relaying into a FIFO the CLI holds open
/// itself (ADR 0034), so its end is not an EOF for the CLI on either engine —
/// on a rootless Podman a closed *container* attach was, and the CLI exited on
/// it within about 100 ms (Bears gg4ch). So the session is still `running`
/// after recovery, in the container it was launched in, and the next message
/// reaches the process that answered the first: the fixture's second turn is
/// what replays, no second launch is recorded and no second `init` appears.
#[tokio::test]
async fn a_restart_adopts_the_session_container_and_keeps_its_token() {
    scenario(|app| async move {
        let fixture = StubFixture::load();
        let project = Fixture::create(&app).await;
        let id = project
            .create_session(
                &app,
                &json!({ "profile_id": project.profile_id, "message": "the first turn" }),
            )
            .await;

        // A turn has to have been committed before the restart, or there would
        // be nothing for the adopting owner to reconstruct its state from.
        wait_for_state(&app, &project, id, SessionState::Running).await;
        let before = wait_for_events(&app, &project, id, "result", 1).await;
        let row_before = reload(&app, id).await;
        let container_before = row_before
            .container_id
            .clone()
            .expect("a running session records its container");

        // The restart: the owners are lost, the containers are not.
        app.simulate_restart().await;
        let report = app.recover().await;
        assert_eq!(
            report.adopted, 1,
            "the running container should have been adopted: {report:?}",
        );
        assert_eq!(report.parked, 0);
        assert_eq!(report.failed, 0);

        let adopted = reload(&app, id).await;
        assert_eq!(
            adopted.mcp_token_hash, row_before.mcp_token_hash,
            "adoption must not rotate the token the live process is holding (ADR 0029)",
        );
        assert_eq!(
            adopted.container_id,
            Some(container_before),
            "the adopted session keeps the container it was launched with",
        );

        // The adopted process is still reading stdin: had the restart reached
        // it as EOF it would have exited and the owner would have parked the
        // session by the time its state came to rest.
        let settled = wait_for_settled_state(&app, id).await;
        assert_eq!(
            settled,
            SessionState::Running,
            "the restart ended the adopted process",
        );
        let container = inspect(id).await;
        assert_eq!(
            container.state.as_ref().and_then(|state| state.running),
            Some(true),
            "the adopted container is no longer running",
        );

        let response = app
            .post_as(&project.user, &format!("/api/sessions/{id}/input"))
            .json(&json!({ "kind": "message", "text": "the second turn" }))
            .await;
        response.assert_status(StatusCode::ACCEPTED);

        let after = wait_for_events(&app, &project, id, "result", 2).await;
        assert!(
            after.len() > before.len(),
            "nothing was committed after the restart",
        );
        assert_contiguous_seq(&after);
        let replied = of_kind(&after, "result")
            .last()
            .and_then(|event| event["cost_usd"].as_f64())
            .expect("the second turn ended with a result");
        assert_eq!(
            // The live process carries on into the fixture's second turn; a
            // relaunched one would have started the fixture again at its first.
            replied,
            fixture.turn(2).cumulative_cost,
            "the second message was not answered by the process that took the first",
        );
        assert_eq!(
            of_kind(&after, "init").len(),
            1,
            "a second `init` means a second process: {:?}",
            kinds(&after),
        );
        let launches = of_kind(&after, "state_change")
            .into_iter()
            .filter(|event| event["to"] == "running")
            .count();
        assert_eq!(
            launches,
            1,
            "the session was relaunched: {:?}",
            kinds(&after)
        );
        assert_eq!(
            reload(&app, id).await.container_id,
            adopted.container_id,
            "the second turn ran in another container",
        );

        // No gap and no duplicate, in the table's own terms: one row per
        // sequence, and one committed offset per transcript line. Only the last
        // event of a line carries `_offset` (`SessionRepository::append_native_line`),
        // so the offsets are counted among the rows that have one and a repeat
        // there is a line committed twice.
        let (count, max_seq, offsets, distinct): (i64, Option<i64>, i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), MAX(seq), COUNT(payload->>'_offset'), \
                    COUNT(DISTINCT payload->>'_offset') \
             FROM events WHERE session_id = $1",
        )
        .bind(id)
        .fetch_one(&app.pool)
        .await
        .expect("the events are countable");
        assert_eq!(Some(count), max_seq, "the sequence has a gap or a repeat");
        assert!(offsets > 0, "no transcript line was committed at all");
        assert_eq!(
            distinct, offsets,
            "two events share a transcript offset, so a line was committed twice",
        );

        // Ending it leaves nothing behind, adopted or not.
        let response = app
            .post_as(&project.user, &format!("/api/sessions/{id}/end"))
            .await;
        response.assert_status_ok();
        wait_for_discarded_container(&app, id).await;
    })
    .await;
}
