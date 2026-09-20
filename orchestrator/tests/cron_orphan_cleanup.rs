//! The orphan cleanup job's container and `/data/tmp` sweeps
//! (`ARCHITECTURE.md`, "Background jobs"; `CLAUDE.md`, "Testing
//! expectations").
//!
//! The job is called directly with the `now` the scenario wants, which is what
//! makes both age guards testable without touching a clock: a container is
//! aged by moving its creation time on the mock engine. A `/data/tmp` entry is
//! aged by moving its own mtime instead, because one run has to judge a stale
//! entry and a fresh one at once and a `now` far enough ahead to age the first
//! ages the second with it.
//!
//! What the assertions are about: that exactly the containers whose session
//! has nothing running are removed, that the two guards — a live registry
//! entry and a container younger than five minutes — keep a launch's container
//! whatever the row says, that `sessions.container_id` is cleared after the
//! removal, and that a sweep failing as a whole does not cost the sweeps after
//! it.
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use chrono::{TimeDelta, Utc};
use common::TestApp;
use mars_orchestrator::cron::JobReport;
use mars_orchestrator::engine::{
    ContainerEngine, ContainerId, ContainerSpec, LABEL_PROJECT_ID, LABEL_SESSION_ID,
};
use mars_orchestrator::models::{NewSession, ProfileKind, SessionState, StateChange};
use mars_orchestrator::repositories::SessionRepository;
use mars_orchestrator::session::{OwnerRx, Phase};
use uuid::Uuid;

/// Not a credential: an obviously fake stand-in for the seeded user's Argon2id
/// PHC string (`CLAUDE.md`, rule 3).
const FAKE_PASSWORD_HASH: &str = "$argon2id$fake$hash";

/// A project and a profile every session in one scenario hangs off.
struct Fixture {
    project_id: Uuid,
    profile_id: Uuid,
}

/// Seed a user, a project and a conversational profile.
///
/// Seeded with unchecked statements for the foreign keys only, as the other
/// repository suites do.
async fn fixture(app: &TestApp) -> Fixture {
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
    .bind(ProfileKind::Conversational)
    .execute(&app.pool)
    .await
    .expect("the profile seeds");

    Fixture {
        project_id,
        profile_id,
    }
}

/// One session in `state`, reached through the lifecycle edges the diagram has
/// (`ARCHITECTURE.md`, "Session lifecycle").
async fn session_in(app: &TestApp, fixture: &Fixture, state: SessionState) -> Uuid {
    let new = NewSession::new(
        fixture.project_id,
        fixture.profile_id,
        ProfileKind::Conversational,
        "main",
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );

    let repository = SessionRepository::new(&app.pool);
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let session = repository
        .insert(&mut tx, &new)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the transaction commits");

    // `creating` is where every session starts, so it needs no edge at all.
    let path: &[SessionState] = match state {
        SessionState::Creating => &[],
        SessionState::Running => &[SessionState::Running],
        SessionState::Parked => &[SessionState::Running, SessionState::Parked],
        SessionState::Done => &[SessionState::Running, SessionState::Done],
        SessionState::Failed => &[SessionState::Failed],
    };

    for to in path {
        let change = match to {
            SessionState::Failed => StateChange::failed("a test put the session here"),
            _ => StateChange::plain(),
        };

        let mut tx = app.pool.begin().await.expect("a transaction begins");
        repository
            .set_state(&mut tx, session.id, *to, &change)
            .await
            .unwrap_or_else(|err| panic!("the session moves to {to}: {err}"));
        tx.commit().await.expect("the transaction commits");
    }

    session.id
}

/// Create a container labelled with `label`, the way the launcher does, and
/// age it to `minutes` before `now`.
async fn container_for(
    app: &TestApp,
    fixture: &Fixture,
    label: &str,
    minutes: i64,
    now: chrono::DateTime<Utc>,
) -> ContainerId {
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
        secret_env: Vec::new(),
        binds: Vec::new(),
        network: "mars-sessions".to_string(),
        extra_hosts: Vec::new(),
        runtime: None,
    };

    let id = app
        .engine()
        .create(&spec)
        .await
        .expect("the container is created");
    assert!(
        app.engine()
            .set_created(&id, now - TimeDelta::minutes(minutes)),
        "the container's creation time is moved",
    );

    id
}

/// Record `sessions.container_id`, the way a launch does.
async fn record_container(app: &TestApp, session_id: Uuid, container_id: &ContainerId) {
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .set_container_id(&mut tx, session_id, Some(container_id.0.as_str()))
        .await
        .expect("the container id is recorded");
    tx.commit().await.expect("the transaction commits");
}

/// The `container_id` on the row, read back.
async fn container_id_of(app: &TestApp, session_id: Uuid) -> Option<String> {
    SessionRepository::new(&app.pool)
        .get(session_id)
        .await
        .expect("the session is readable")
        .container_id
}

/// `DATA_DIR/tmp`, the directory the second sweep empties.
fn tmp_dir(app: &TestApp) -> PathBuf {
    app.state.config.data_dir.join("tmp")
}

#[tokio::test]
async fn only_containers_of_sessions_with_nothing_running_are_removed() {
    let app = TestApp::spawn().await;
    let fixture = fixture(&app).await;
    let now = Utc::now();

    // (a) a container whose label names no session at all.
    let missing = container_for(&app, &fixture, &Uuid::new_v4().to_string(), 10, now).await;

    // (b)–(d) the three ended states, each with the removal recorded on its
    // row so the clearing of `container_id` can be asserted.
    let parked = session_in(&app, &fixture, SessionState::Parked).await;
    let parked_container = container_for(&app, &fixture, &parked.to_string(), 10, now).await;
    record_container(&app, parked, &parked_container).await;

    let done = session_in(&app, &fixture, SessionState::Done).await;
    let done_container = container_for(&app, &fixture, &done.to_string(), 10, now).await;
    record_container(&app, done, &done_container).await;

    let failed = session_in(&app, &fixture, SessionState::Failed).await;
    let failed_container = container_for(&app, &fixture, &failed.to_string(), 10, now).await;
    record_container(&app, failed, &failed_container).await;

    // (e), (f) the two states that own their container.
    let running = session_in(&app, &fixture, SessionState::Running).await;
    let running_container = container_for(&app, &fixture, &running.to_string(), 10, now).await;
    record_container(&app, running, &running_container).await;

    let creating = session_in(&app, &fixture, SessionState::Creating).await;
    let creating_container = container_for(&app, &fixture, &creating.to_string(), 10, now).await;

    // (g) parked, but this process is relaunching it: the owner is registered
    // before the container is created.
    let relaunching = session_in(&app, &fixture, SessionState::Parked).await;
    let relaunching_container =
        container_for(&app, &fixture, &relaunching.to_string(), 10, now).await;
    let _owner: OwnerRx =
        app.session_registry()
            .register(relaunching, ProfileKind::Conversational, Phase::Creating);

    // (h) parked, container created two minutes ago: inside the launch window.
    let young = session_in(&app, &fixture, SessionState::Parked).await;
    let young_container = container_for(&app, &fixture, &young.to_string(), 2, now).await;

    // (i) a label that is not a uuid.
    let unparseable = container_for(&app, &fixture, "not-a-session-id", 10, now).await;

    let report = app
        .cron()
        .orphan_cleanup(now)
        .await
        .expect("orphan cleanup runs");

    assert_eq!(report.items, 4, "removed the wrong number of containers");
    assert_eq!(report.failures, 0, "a sweep reported a failure: {report:?}");

    for (id, what) in [
        (&missing, "a container whose session does not exist"),
        (&parked_container, "a parked session's container"),
        (&done_container, "a done session's container"),
        (&failed_container, "a failed session's container"),
    ] {
        assert!(
            app.engine().state_of(id).is_none(),
            "{what} was not removed",
        );
    }

    for (id, what) in [
        (&running_container, "a running session's container"),
        (&creating_container, "a creating session's container"),
        (&relaunching_container, "a relaunching session's container"),
        (&young_container, "a container two minutes old"),
        (&unparseable, "a container with an unparseable label"),
    ] {
        assert!(app.engine().state_of(id).is_some(), "{what} was removed");
    }

    for (session_id, what) in [(parked, "parked"), (done, "done"), (failed, "failed")] {
        assert_eq!(
            container_id_of(&app, session_id).await,
            None,
            "the {what} session still points at its removed container",
        );
    }

    // The running session's row is untouched, which is the other half of "a
    // running session is never this job's business".
    assert_eq!(
        container_id_of(&app, running).await,
        Some(running_container.0.clone()),
    );
}

#[tokio::test]
async fn stale_tmp_entries_are_removed_and_fresh_ones_are_left() {
    let app = TestApp::spawn().await;
    let tmp = tmp_dir(&app);
    fs::create_dir_all(&tmp).expect("the temporary directory is made");

    let old_clone = tmp.join("old-clone");
    let stale_file = tmp.join("stale.file");
    fs::create_dir(&old_clone).expect("the old clone is made");
    fs::write(old_clone.join("HEAD"), b"ref: refs/heads/main\n").expect("the old clone has a file");
    fs::write(&stale_file, b"a leftover credential config\n").expect("the stale file is made");
    age(&old_clone);
    age(&stale_file);

    // Younger than an hour: the merge or rebase that made it may still be
    // using it under its project's git lock.
    let fresh_clone = tmp.join("fresh-clone");
    fs::create_dir(&fresh_clone).expect("the fresh clone is made");

    let report = app
        .cron()
        .orphan_cleanup(Utc::now())
        .await
        .expect("orphan cleanup runs");

    assert_eq!(report.items, 2, "removed the wrong entries: {report:?}");
    assert_eq!(report.failures, 0, "a sweep reported a failure: {report:?}");
    assert!(!old_clone.exists(), "the old clone was not removed");
    assert!(!stale_file.exists(), "the stale file was not removed");
    assert!(fresh_clone.exists(), "the fresh clone was removed");
}

#[tokio::test]
async fn a_missing_tmp_directory_is_nothing_to_do() {
    let app = TestApp::spawn().await;
    assert!(
        !tmp_dir(&app).exists(),
        "a freshly spawned app already has a temporary directory",
    );

    let report = app
        .cron()
        .orphan_cleanup(Utc::now())
        .await
        .expect("orphan cleanup runs");

    assert_eq!(report, JobReport::default(), "an empty run did work");
}

#[tokio::test]
async fn an_unreachable_engine_costs_one_failure_and_the_tmp_sweep_still_runs() {
    let app = TestApp::spawn().await;
    let tmp = tmp_dir(&app);
    fs::create_dir_all(&tmp).expect("the temporary directory is made");
    let old_clone = tmp.join("old-clone");
    fs::create_dir(&old_clone).expect("the old clone is made");
    age(&old_clone);

    app.engine()
        .fail_next_list("the mock engine was made unreachable");

    let report = app
        .cron()
        .orphan_cleanup(Utc::now())
        .await
        .expect("orphan cleanup runs even when a sweep fails");

    assert_eq!(report.failures, 1, "the failed sweep was not counted once");
    assert_eq!(report.items, 1, "the tmp sweep did not run: {report:?}");
    assert!(!old_clone.exists(), "the old clone was not removed");
}

/// Move an entry's modification time two hours back, which is what makes it a
/// leftover the sweep collects.
///
/// The mtime is rewritten rather than the job's `now` moved forward, because
/// one `now` has to judge a stale entry and a fresh one in the same run:
/// a `now` far enough in the future to age the stale entry ages the fresh one
/// with it. `File::set_modified` works on a directory as well as on a file,
/// which is what both leftovers are.
fn age(path: &std::path::Path) {
    let file = fs::File::open(path).expect("the entry opens");
    file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600))
        .expect("the entry's modification time is moved back");
}
