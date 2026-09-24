//! The orphan cleanup job's container, `/data/tmp`, hand-off ref and session ref sweeps
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
//! The hand-off sweep is asserted on real repositories (`CLAUDE.md`, "Testing
//! expectations": git is never mocked) and on one `CronService` value held
//! across several runs, because the sightings that make the two-sighting rule
//! work live on the service: a fresh `app.cron()` per run would forget them
//! and nothing would ever be removed.
//!
//! Needs a container engine; see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use common::TestApp;
use common::handoffs::Fixture as HandoffFixture;
use mars_orchestrator::cron::JobReport;
use mars_orchestrator::engine::{
    ContainerEngine, ContainerId, ContainerSpec, LABEL_PROJECT_ID, LABEL_SESSION_ID,
};
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::git::testutil::run_git;
use mars_orchestrator::git::{init_project_repo, refs};
use mars_orchestrator::models::{
    BranchName, NewProject, NewSession, NewTask, NewTaskComment, NewTaskHandoff, ProfileKind,
    Project, RemoteUrl, SessionState, StateChange,
};
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository, TaskRepository};
use mars_orchestrator::session::{OwnerRx, Phase};
use mars_orchestrator::tracker::TrackerMutation;
use tokio::time::timeout;
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
    // It parked in this process, which keeps its registry entry for the input
    // queue: an entry alone must not protect its leftover.
    drop(
        app.session_registry()
            .register(parked, ProfileKind::Conversational, Phase::Running),
    );
    app.session_registry().mark_parked(parked);

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

// --- the hand-off ref sweep -----------------------------------------------

/// A second project with a repository of its own, beside the hand-off
/// fixture's, so that the sweep is asserted over more than one git lock and
/// the per-project scope of the row query is visible.
///
/// It shares the fixture's upstream: what matters here is that
/// `DATA_DIR/projects/<id>/repo.git` is a real bare repository with a commit
/// in it, not where that commit came from.
async fn second_project(fixture: &HandoffFixture) -> Project {
    let mut new_project = NewProject::new("orphans-other", "https://git.example.invalid/other.git")
        .expect("the project is valid");
    new_project.default_branch = Some(BranchName::parse("main").expect("main is a branch name"));

    let projects = ProjectRepository::new(&fixture.app.pool);
    let mut tx = fixture
        .app
        .pool
        .begin()
        .await
        .expect("a transaction begins");
    let project = projects
        .insert(&mut tx, &new_project)
        .await
        .expect("the project inserts");
    tx.commit().await.expect("the transaction commits");

    {
        let guard = fixture.app.state.git_locks.lock(project.id).await;
        init_project_repo(
            &guard,
            &fixture.paths(),
            &RemoteUrl::local_for_tests(&fixture.upstream.path),
            Some("main"),
            None,
        )
        .await
        .expect("the second project's repository is initialised");
    }

    let mut mutation = TrackerMutation::begin(&fixture.app.pool, project.id, TaskActor::System)
        .await
        .expect("the mutation opens");
    TaskRepository::new(&fixture.app.pool)
        .insert_default_states(mutation.conn(), project.id)
        .await
        .expect("the default states insert");
    mutation.commit().await.expect("the mutation commits");

    project
}

/// A task with a comment and a hand-off row in `project_id`, as a publication
/// would leave behind, and the hand-off's id.
///
/// The fixture's own `task`/`current_handoff` pair only knows its own project,
/// so the second project's rows are made here through the same repositories.
async fn handoff_row(fixture: &HandoffFixture, project_id: Uuid, commit: &str) -> Uuid {
    let repository = TaskRepository::new(&fixture.app.pool);
    let mut mutation = TrackerMutation::begin(&fixture.app.pool, project_id, TaskActor::System)
        .await
        .expect("the mutation opens");

    let new_task = NewTask::new(project_id, "hand the work over").expect("the title parses");
    let task = repository
        .insert_task(mutation.conn(), project_id, &new_task)
        .await
        .expect("the task inserts");

    let comment = NewTaskComment::from_user(task.id, fixture.user.id, "the published revision");
    let comment = repository
        .insert_comment(mutation.conn(), project_id, &comment)
        .await
        .expect("the comment inserts");

    let mut handoff = NewTaskHandoff::new(task.id, "session/fake", commit, comment.id);
    handoff.created_by_user_id = Some(fixture.user.id);
    let handoff = repository
        .insert_handoff(mutation.conn(), project_id, &handoff)
        .await
        .expect("the hand-off inserts");
    mutation.commit().await.expect("the mutation commits");

    handoff.id
}

/// The commit `refs/heads/main` names in a project's repository: what every
/// hand-off ref in these scenarios is pinned at.
async fn main_commit(mirror: &std::path::Path) -> String {
    run_git(mirror, &["rev-parse", "refs/heads/main"])
        .await
        .trim()
        .to_string()
}

/// Pin `refs/handoffs/<id>` at `commit`, the way publication does under the
/// project git lock.
async fn pin(fixture: &HandoffFixture, project_id: Uuid, id: Uuid, commit: &str) {
    let _guard = fixture.app.state.git_locks.lock(project_id).await;
    refs::retain_handoff(&fixture.paths().project_repo(project_id), id, commit)
        .await
        .expect("the hand-off ref is pinned");
}

/// The hand-off ids a project's repository currently retains, sorted.
async fn pinned(fixture: &HandoffFixture, project_id: Uuid) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = refs::list_handoffs(&fixture.paths().project_repo(project_id))
        .await
        .expect("the hand-off refs list")
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn an_orphan_handoff_ref_is_removed_only_on_a_second_sighting_an_hour_later() {
    let fixture = HandoffFixture::create("orphan-handoffs").await;
    let other = second_project(&fixture).await;

    let mirror_a = fixture.paths().project_repo(fixture.project.id);
    let mirror_b = fixture.paths().project_repo(other.id);
    let commit_a = main_commit(&mirror_a).await;
    let commit_b = main_commit(&mirror_b).await;

    // (a) a published hand-off: a row and its ref, which must survive.
    let task = fixture.task("implement it", "ready").await;
    let (_task, id_a) = fixture
        .current_handoff(&task, None, "session/fake", &commit_a)
        .await;
    pin(&fixture, fixture.project.id, id_a, &commit_a).await;

    // (b) a ref whose row was deleted with its task: an interrupted deletion.
    let id_b = handoff_row(&fixture, fixture.project.id, &commit_a).await;
    pin(&fixture, fixture.project.id, id_b, &commit_a).await;
    // The row-level fact no interface answers: a `task_handoffs` row that is
    // gone while its ref is not, which is what an interrupted deletion leaves.
    sqlx::query("DELETE FROM task_handoffs WHERE id = $1")
        .bind(id_b)
        .execute(&fixture.app.pool)
        .await
        .expect("the hand-off row is deleted");

    // (c) a ref for an id that has no row at all yet: a publication that
    // pinned its ref and has not committed its tracker transaction.
    let id_c = Uuid::new_v4();
    pin(&fixture, fixture.project.id, id_c, &commit_a).await;

    // The second project: one published hand-off of its own, and a copy of the
    // first project's ref, whose row belongs to another project and is
    // therefore an orphan here.
    let id_d = handoff_row(&fixture, other.id, &commit_b).await;
    pin(&fixture, other.id, id_d, &commit_b).await;
    pin(&fixture, other.id, id_a, &commit_b).await;

    // One service across the runs: the sightings live on it.
    let cron = fixture.app.cron();
    let now = Utc::now();

    let first = cron.orphan_cleanup(now).await.expect("the first run");
    assert_eq!(
        first,
        JobReport {
            items: 0,
            skipped: 3,
            failures: 0
        },
        "the first sighting must remove nothing",
    );

    let second = cron
        .orphan_cleanup(now + TimeDelta::minutes(30))
        .await
        .expect("the second run");
    assert_eq!(
        second,
        JobReport {
            items: 0,
            skipped: 3,
            failures: 0
        },
        "a second sighting inside the hour must remove nothing",
    );

    // The publication behind (c) finally commits its row, under an id the
    // sweep has already sighted twice.
    let mut mutation =
        TrackerMutation::begin(&fixture.app.pool, fixture.project.id, TaskActor::System)
            .await
            .expect("the mutation opens");
    let late_task = TaskRepository::new(&fixture.app.pool)
        .insert_task(
            mutation.conn(),
            fixture.project.id,
            &NewTask::new(fixture.project.id, "the late publication").expect("the title parses"),
        )
        .await
        .expect("the task inserts");
    mutation.commit().await.expect("the mutation commits");
    // Raw SQL for the one thing no interface offers: a hand-off row with a
    // chosen id, so the row lands under the ref the sweep already remembers.
    sqlx::query(
        r#"INSERT INTO task_handoffs (id, task_id, source_branch, "commit")
           VALUES ($1, $2, 'session/fake', $3)"#,
    )
    .bind(id_c)
    .bind(late_task.id)
    .bind(&commit_a)
    .execute(&fixture.app.pool)
    .await
    .expect("the late hand-off row inserts");

    let third = cron
        .orphan_cleanup(now + TimeDelta::minutes(61))
        .await
        .expect("the third run");
    assert_eq!(
        third,
        JobReport {
            items: 2,
            skipped: 0,
            failures: 0
        },
        "exactly the two refs orphaned for an hour are removed",
    );

    let mut remaining_a = vec![id_a, id_c];
    remaining_a.sort();
    assert_eq!(
        pinned(&fixture, fixture.project.id).await,
        remaining_a,
        "a published ref, or one whose row arrived late, was removed",
    );
    assert_eq!(
        pinned(&fixture, other.id).await,
        vec![id_d],
        "the second project's own hand-off ref was removed, or another project's was kept",
    );

    let fourth = cron
        .orphan_cleanup(now + TimeDelta::minutes(122))
        .await
        .expect("the fourth run");
    assert_eq!(
        fourth,
        JobReport::default(),
        "a run with nothing orphaned did work",
    );
}

#[tokio::test]
async fn the_sweep_waits_for_the_project_git_lock() {
    let fixture = HandoffFixture::create("orphan-handoff-lock").await;
    let cron = fixture.app.cron();

    let guard = fixture.guard().await;

    let blocked = timeout(Duration::from_millis(200), cron.orphan_cleanup(Utc::now())).await;
    assert!(
        blocked.is_err(),
        "the sweep reached a project's refs while its git lock was held",
    );

    drop(guard);

    let report = timeout(Duration::from_secs(10), cron.orphan_cleanup(Utc::now()))
        .await
        .expect("the sweep completes once the git lock is released")
        .expect("orphan cleanup runs");
    assert_eq!(report, JobReport::default(), "an empty run did work");
}

#[tokio::test]
async fn an_unreadable_repository_costs_one_failure_and_the_other_projects_are_swept() {
    let fixture = HandoffFixture::create("orphan-handoff-failure").await;
    let commit = main_commit(&fixture.paths().project_repo(fixture.project.id)).await;

    // An orphan in the healthy project, so its sweep is visible in the report.
    let id = Uuid::new_v4();
    pin(&fixture, fixture.project.id, id, &commit).await;

    // A project whose `repo.git` is there but is not a repository: what an
    // interrupted deletion, or a half-written clone, leaves behind. A project
    // with no `repo.git` at all is skipped rather than counted, which is what
    // the container and `/data/tmp` scenarios above rely on.
    let broken = second_project(&fixture).await;
    let broken_repo = fixture.paths().project_repo(broken.id);
    fs::remove_dir_all(&broken_repo).expect("the second repository is removed");
    fs::create_dir_all(&broken_repo).expect("an empty directory takes its place");

    let report = fixture
        .app
        .cron()
        .orphan_cleanup(Utc::now())
        .await
        .expect("orphan cleanup runs even when one project fails");

    assert_eq!(
        report,
        JobReport {
            items: 0,
            skipped: 1,
            failures: 1
        },
        "the broken project cost more than one failure, or stopped the sweep",
    );
    assert_eq!(
        pinned(&fixture, fixture.project.id).await,
        vec![id],
        "a first-sighted orphan was removed",
    );
}

// --- the session ref sweep ------------------------------------------------

/// The session ids a project's repository has a `refs/sessions/<sid>` for,
/// sorted.
async fn session_refs(fixture: &HandoffFixture, project_id: Uuid) -> Vec<Uuid> {
    let mut ids = refs::list_sessions(&fixture.paths().project_repo(project_id))
        .await
        .expect("the session refs list");
    ids.sort();
    ids
}

#[tokio::test]
async fn a_session_ref_with_no_session_row_is_removed_and_a_live_sessions_ref_is_kept() {
    let fixture = HandoffFixture::create("orphan-sessions").await;
    let mirror = fixture.paths().project_repo(fixture.project.id);

    // A live session whose branch has been fetched back.
    let (live, _commit) = fixture.session_with_commit("feat: live work").await;
    fixture.sync(live).await;

    // A ref named by a session that no longer exists: what a session deleted
    // before deletion removed its ref leaves behind (ADR 0049).
    let orphan = Uuid::new_v4();
    let commit = main_commit(&mirror).await;
    {
        let _guard = fixture.guard().await;
        refs::update(&mirror, &refs::session_ref(orphan), &commit, None)
            .await
            .expect("the orphan session ref is written");
    }

    let mut both = vec![live, orphan];
    both.sort();
    assert_eq!(session_refs(&fixture, fixture.project.id).await, both);

    let report = fixture
        .app
        .cron()
        .orphan_cleanup(Utc::now())
        .await
        .expect("orphan cleanup runs");

    assert_eq!(
        report,
        JobReport {
            items: 1,
            skipped: 0,
            failures: 0
        },
        "an orphan session ref is removed on its first sighting, and only it",
    );
    assert_eq!(
        session_refs(&fixture, fixture.project.id).await,
        vec![live],
        "the live session's ref was removed, or the orphan was kept",
    );
}
