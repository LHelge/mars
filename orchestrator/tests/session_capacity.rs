//! `session::capacity::unattended_capacity` against a real database
//! (`ARCHITECTURE.md`, "Task tracker" → "Unattended launches" → "Capacity" and
//! "Pause"; ADR 0042).
//!
//! The four bounds, one scenario each, plus the two rules that are easy to get
//! wrong and invisible in a unit test: which session states count, and that a
//! session a *person* launched counts against automation exactly as a
//! dispatched one does.
//!
//! Everything is arranged through the repositories rather than raw SQL: a
//! project, one or two profiles and session rows moved into the state the
//! scenario needs with `SessionRepository::transition`, which is the only way
//! a session reaches `parked`, `done` or `failed` in production either. The
//! project never clones anything — capacity is decided before any of that, and
//! from rows alone.
//!
//! The instance cap is the one bound that is not in the database, so a
//! scenario varying it clones the app state with a `Config` of its own; the
//! rest of the state, the pool included, is the same allocation.
//!
//! Every credential-shaped value is an obviously fake stand-in (rule 3).
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use std::sync::Arc;

use common::TestApp;
use mars_orchestrator::models::{
    AgentProfile, MaxConcurrentSessions, NewAgentProfile, NewProject, NewSession, ProfileKind,
    Project, ProjectUpdate, SessionState,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository, Transition};
use mars_orchestrator::session::{CapacityRefusal, LaunchCapacity, unattended_capacity};
use uuid::Uuid;

/// A project and one profile, both freshly inserted, with nothing live yet.
struct Fixture {
    project: Project,
    profile: AgentProfile,
}

impl Fixture {
    /// A project whose profile caps itself at `profile_cap` live sessions.
    async fn create(app: &TestApp, profile_cap: i32) -> Self {
        let name = Uuid::new_v4().simple().to_string()[..8].to_string();
        let projects = ProjectRepository::new(&app.pool);

        let new_project = NewProject::new(
            &format!("project-{name}"),
            "https://git.example.test/mars.git",
        )
        .expect("the project validates");

        let mut tx = app.pool.begin().await.expect("a transaction begins");
        let project = projects
            .insert(&mut tx, &new_project)
            .await
            .expect("the project inserts");

        let mut new_profile = NewAgentProfile::new(
            project.id,
            &format!("profile-{name}"),
            "localhost/mars-session:test",
        )
        .expect("the profile validates");
        new_profile.max_concurrent = profile_cap;
        new_profile.validate().expect("the cap validates");

        let profile = projects
            .insert_profile(&mut tx, &new_profile)
            .await
            .expect("the profile inserts");
        tx.commit().await.expect("the fixture commits");

        Self { project, profile }
    }

    /// Re-read the project, after an update changed one of its columns.
    async fn reload(&mut self, app: &TestApp) {
        self.project = ProjectRepository::new(&app.pool)
            .find(self.project.id)
            .await
            .expect("the project reads")
            .expect("the project is there");
    }

    /// A second profile of the same project, with its own cap.
    async fn second_profile(&self, app: &TestApp, profile_cap: i32) -> AgentProfile {
        let name = Uuid::new_v4().simple().to_string()[..8].to_string();
        let mut new_profile = NewAgentProfile::new(
            self.project.id,
            &format!("other-{name}"),
            "localhost/mars-session:test",
        )
        .expect("the profile validates");
        new_profile.max_concurrent = profile_cap;
        new_profile.validate().expect("the cap validates");

        let mut tx = app.pool.begin().await.expect("a transaction begins");
        let profile = ProjectRepository::new(&app.pool)
            .insert_profile(&mut tx, &new_profile)
            .await
            .expect("the profile inserts");
        tx.commit().await.expect("the profile commits");

        profile
    }
}

/// Insert one session of `profile` and leave it in `state`.
///
/// `creating` is where every session starts, so the other states are reached
/// through the transitions the lifecycle diagram allows, which is how a session
/// reaches them in production as well.
async fn session_in(app: &TestApp, profile: &AgentProfile, state: SessionState) -> Uuid {
    let new_session = NewSession::new(
        profile.project_id,
        profile.id,
        profile.kind,
        "main",
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );

    let sessions = SessionRepository::new(&app.pool);
    let mut tx = app.pool.begin().await.expect("a transaction begins");
    let session = sessions
        .insert(&mut tx, &new_session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the session commits");

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

    for (from, to) in path {
        let mut tx = app.pool.begin().await.expect("a transaction begins");
        sessions
            .transition(&mut tx, session.id, &Transition::new(*from, *to, "fixture"))
            .await
            .expect("the transition applies");
        tx.commit().await.expect("the transition commits");
    }

    session.id
}

/// The same app state with a different `AUTOMATION_MAX_SESSIONS`.
///
/// The only one of the four bounds that is configuration rather than a column,
/// and `Config` is cloneable, so a scenario varies it without a second app.
fn with_instance_cap(app: &TestApp, cap: i64) -> AppState {
    let mut config = (*app.state.config).clone();
    config.automation_max_sessions = cap;

    let mut state = app.state.clone();
    state.config = Arc::new(config);

    state
}

/// Set the project's cap, or clear it with `None`.
async fn set_project_cap(app: &TestApp, project_id: Uuid, cap: Option<i32>) {
    let update = ProjectUpdate {
        max_concurrent_sessions: Some(
            cap.map(|cap| MaxConcurrentSessions::parse(cap).expect("the cap validates")),
        ),
        ..ProjectUpdate::default()
    };

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    ProjectRepository::new(&app.pool)
        .update(&mut tx, project_id, &update)
        .await
        .expect("the project updates")
        .expect("the project is there");
    tx.commit().await.expect("the update commits");
}

/// Set the project's automation pause.
async fn set_paused(app: &TestApp, project_id: Uuid, paused: bool) {
    let update = ProjectUpdate {
        automation_paused: Some(paused),
        ..ProjectUpdate::default()
    };

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    ProjectRepository::new(&app.pool)
        .update(&mut tx, project_id, &update)
        .await
        .expect("the project updates")
        .expect("the project is there");
    tx.commit().await.expect("the update commits");
}

/// A fresh project with nothing running is available under every bound.
#[tokio::test]
async fn an_idle_project_may_launch() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app, 2).await;

    let capacity = unattended_capacity(&app.state, &fixture.project, &fixture.profile)
        .await
        .expect("the capacity reads");

    assert_eq!(capacity, LaunchCapacity::Available);
}

/// The profile's own `max_concurrent` allows one below it and refuses at it.
#[tokio::test]
async fn the_profile_cap_refuses_at_its_limit() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app, 2).await;

    session_in(&app, &fixture.profile, SessionState::Running).await;
    let capacity = unattended_capacity(&app.state, &fixture.project, &fixture.profile)
        .await
        .expect("the capacity reads");
    assert_eq!(
        capacity,
        LaunchCapacity::Available,
        "one live session is below a cap of two",
    );

    session_in(&app, &fixture.profile, SessionState::Creating).await;
    let capacity = unattended_capacity(&app.state, &fixture.project, &fixture.profile)
        .await
        .expect("the capacity reads");
    assert_eq!(
        capacity,
        LaunchCapacity::Refused(CapacityRefusal::ProfileCap { live: 2, cap: 2 }),
    );
}

/// The project's cap counts every profile's live sessions, and refuses a
/// profile that is well under its own cap.
#[tokio::test]
async fn the_project_cap_refuses_at_its_limit() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app, 9).await;
    let other = fixture.second_profile(&app, 9).await;
    set_project_cap(&app, fixture.project.id, Some(2)).await;
    fixture.reload(&app).await;

    session_in(&app, &other, SessionState::Running).await;
    let capacity = unattended_capacity(&app.state, &fixture.project, &fixture.profile)
        .await
        .expect("the capacity reads");
    assert_eq!(capacity, LaunchCapacity::Available);

    session_in(&app, &other, SessionState::Running).await;
    let capacity = unattended_capacity(&app.state, &fixture.project, &fixture.profile)
        .await
        .expect("the capacity reads");
    assert_eq!(
        capacity,
        LaunchCapacity::Refused(CapacityRefusal::ProjectCap { live: 2, cap: 2 }),
        "another profile's sessions count against the project cap",
    );
}

/// A NULL `max_concurrent_sessions` is no project cap at all.
#[tokio::test]
async fn a_null_project_cap_is_no_cap() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app, 9).await;
    let other = fixture.second_profile(&app, 9).await;

    for _ in 0..5 {
        session_in(&app, &other, SessionState::Running).await;
    }
    fixture.reload(&app).await;
    assert_eq!(
        fixture.project.max_concurrent_sessions, None,
        "the column defaults to NULL",
    );

    let state = with_instance_cap(&app, 99);
    let capacity = unattended_capacity(&state, &fixture.project, &fixture.profile)
        .await
        .expect("the capacity reads");
    assert_eq!(capacity, LaunchCapacity::Available);
}

/// `AUTOMATION_MAX_SESSIONS` counts live sessions of every project, and refuses
/// a project that is under both of its own caps.
#[tokio::test]
async fn the_instance_cap_refuses_at_its_limit() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app, 9).await;
    let elsewhere = Fixture::create(&app, 9).await;

    session_in(&app, &elsewhere.profile, SessionState::Running).await;
    let state = with_instance_cap(&app, 2);
    let capacity = unattended_capacity(&state, &fixture.project, &fixture.profile)
        .await
        .expect("the capacity reads");
    assert_eq!(capacity, LaunchCapacity::Available);

    session_in(&app, &elsewhere.profile, SessionState::Running).await;
    let capacity = unattended_capacity(&state, &fixture.project, &fixture.profile)
        .await
        .expect("the capacity reads");
    assert_eq!(
        capacity,
        LaunchCapacity::Refused(CapacityRefusal::InstanceCap { live: 2, cap: 2 }),
        "another project's sessions count against the instance cap",
    );
}

/// The pause refuses whatever the counts are, and does so before any of them.
#[tokio::test]
async fn the_pause_refuses_an_idle_project() {
    let app = TestApp::spawn().await;
    let mut fixture = Fixture::create(&app, 9).await;
    set_paused(&app, fixture.project.id, true).await;
    fixture.reload(&app).await;

    let capacity = unattended_capacity(&app.state, &fixture.project, &fixture.profile)
        .await
        .expect("the capacity reads");
    assert_eq!(
        capacity,
        LaunchCapacity::Refused(CapacityRefusal::AutomationPaused),
        "a paused project refuses although nothing is live",
    );

    set_paused(&app, fixture.project.id, false).await;
    fixture.reload(&app).await;
    let capacity = unattended_capacity(&app.state, &fixture.project, &fixture.profile)
        .await
        .expect("the capacity reads");
    assert_eq!(
        capacity,
        LaunchCapacity::Available,
        "unpausing needs no restart and no other change",
    );
}

/// `parked`, `done` and `failed` are not live: a profile at its cap in those
/// states may launch.
#[tokio::test]
async fn only_creating_and_running_count() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app, 1).await;

    for state in [
        SessionState::Parked,
        SessionState::Done,
        SessionState::Failed,
    ] {
        session_in(&app, &fixture.profile, state).await;
    }

    let state = with_instance_cap(&app, 1);
    let capacity = unattended_capacity(&state, &fixture.project, &fixture.profile)
        .await
        .expect("the capacity reads");
    assert_eq!(
        capacity,
        LaunchCapacity::Available,
        "three finished or parked sessions are not three live ones",
    );
}

/// A session a person launched counts against all three caps, exactly as a
/// dispatched one does.
#[tokio::test]
async fn a_user_launched_session_counts() {
    let app = TestApp::spawn().await;
    let fixture = Fixture::create(&app, 1).await;
    let user = app
        .create_user(
            "capacity-user",
            "capacity-user@example.test",
            "fake-password-capacity",
        )
        .await;

    let mut new_session = NewSession::new(
        fixture.project.id,
        fixture.profile.id,
        ProfileKind::Conversational,
        "main",
        format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
    );
    new_session.created_by = Some(user.user.id);

    let mut tx = app.pool.begin().await.expect("a transaction begins");
    SessionRepository::new(&app.pool)
        .insert(&mut tx, &new_session)
        .await
        .expect("the session inserts");
    tx.commit().await.expect("the session commits");

    let capacity = unattended_capacity(&app.state, &fixture.project, &fixture.profile)
        .await
        .expect("the capacity reads");
    assert_eq!(
        capacity,
        LaunchCapacity::Refused(CapacityRefusal::ProfileCap { live: 1, cap: 1 }),
        "the caps count all live sessions, whoever launched them",
    );
}
