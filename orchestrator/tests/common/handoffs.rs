//! A ready project with a real repository, for the code hand-off suites
//! (`tests/tracker_handoffs.rs`, `tests/handoffs_service.rs`,
//! `tests/handoffs_api.rs`).
//!
//! The arrangement every hand-off case needs and none of it is the thing under
//! test: a real bare upstream, a real project repository under the `TestApp`'s
//! own `DATA_DIR`, the documented default states, a user, a profile, and the
//! session work clones an agent commits in. Git is never mocked (`CLAUDE.md`,
//! "Testing expectations"), so "a session with a commit in it" really is a
//! clone with a commit in it.
//!
//! It may `expect`: a fixture that cannot build its repository has nothing to
//! return, and a panic naming the failed step is what a test author needs to
//! see. Every identity here is obviously fake (rule 3).

use chrono::Utc;
use mars_orchestrator::events::TaskActor;
use mars_orchestrator::git::testutil::{TestUpstream, run_git, test_identity};
use mars_orchestrator::git::{
    DataPaths, GitService, ProjectGitGuard, create_work_clone, init_project_repo, refs,
    resolve_base,
};
use mars_orchestrator::models::{
    BranchName, HandoffCaller, NewAgentProfile, NewProject, NewSession, NewTask, NewTaskComment,
    NewTaskHandoff, ProfileKind, Project, ProjectStatus, RemoteUrl, Task, TaskState, User,
};
use mars_orchestrator::repositories::tasks::test_support::{StateFields, TaskRepositoryTestExt};
use mars_orchestrator::repositories::{ProjectRepository, SessionRepository, TaskRepository};
use mars_orchestrator::tracker::TrackerMutation;
use uuid::Uuid;

use super::{AuthenticatedUser, TestApp};

/// Not a real remote: the fixture value every project test stores (rule 3).
const TEST_REMOTE: &str = "https://git.example.com/fake/repo.git";

/// A ready project with a real repository, default states, a user and a
/// profile to hang sessions off.
pub struct Fixture {
    pub app: TestApp,
    #[allow(dead_code)]
    pub upstream: TestUpstream,
    pub project: Project,
    pub user: User,
    pub profile_id: Uuid,
}

impl Fixture {
    pub async fn create(name: &str) -> Self {
        let app = TestApp::spawn().await;
        let upstream = TestUpstream::create().await;

        let mut new_project = NewProject::new(name, TEST_REMOTE).expect("the project is valid");
        new_project.default_branch =
            Some(BranchName::parse("main").expect("main is a branch name"));

        let projects = ProjectRepository::new(&app.pool);
        let mut tx = app.pool.begin().await.expect("a transaction begins");
        let project = projects
            .insert(&mut tx, &new_project)
            .await
            .expect("the project inserts");
        tx.commit().await.expect("the transaction commits");

        {
            let guard = app.state.git_locks.lock(project.id).await;
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
        let project = projects
            .set_status(&mut tx, project.id, ProjectStatus::Ready, None)
            .await
            .expect("the status is set")
            .expect("the project exists");
        tx.commit().await.expect("the transaction commits");

        let mut mutation = TrackerMutation::begin(&app.pool, project.id, TaskActor::System)
            .await
            .expect("the mutation opens");
        TaskRepository::new(&app.pool)
            .insert_default_states(mutation.conn(), project.id)
            .await
            .expect("the default states insert");
        mutation.commit().await.expect("the mutation commits");

        let user = app
            .insert_user(
                &format!("ada-{}", &Uuid::new_v4().simple().to_string()[..8]),
                &format!("{}@example.test", Uuid::new_v4()),
                false,
                false,
            )
            .await;

        let profile = NewAgentProfile::new(project.id, "default", "localhost/mars-session:test")
            .expect("the profile is valid");
        let mut tx = app.pool.begin().await.expect("a transaction begins");
        let profile = projects
            .insert_profile(&mut tx, &profile)
            .await
            .expect("the profile inserts");
        tx.commit().await.expect("the transaction commits");

        Self {
            app,
            upstream,
            project,
            user,
            profile_id: profile.id,
        }
    }

    pub fn paths(&self) -> DataPaths {
        DataPaths::from_config(&self.app.state.config)
    }

    pub async fn guard(&self) -> ProjectGitGuard {
        self.app.state.git_locks.lock(self.project.id).await
    }

    /// A session row with no work clone: a session still `creating`.
    pub async fn seed_session(&self) -> Uuid {
        self.seed_session_in(self.project.id, self.profile_id).await
    }

    pub async fn seed_session_in(&self, project_id: Uuid, profile_id: Uuid) -> Uuid {
        let mut session = NewSession::new(
            project_id,
            profile_id,
            ProfileKind::Conversational,
            "main",
            // Not a credential: a fake stand-in for the hashed MCP token
            // (rule 3).
            format!("fake-mcp-token-hash-{}", Uuid::new_v4()),
        );
        session.created_by = Some(self.user.id);

        let mut tx = self.app.pool.begin().await.expect("a transaction begins");
        let inserted = SessionRepository::new(&self.app.pool)
            .insert(&mut tx, &session)
            .await
            .expect("the session inserts");
        tx.commit().await.expect("the transaction commits");

        inserted.id
    }

    /// A session with a work clone and one commit in it, *not* fetched back:
    /// exactly what an agent leaves behind when it reports a commit.
    pub async fn session_with_commit(&self, message: &str) -> (Uuid, String) {
        let session_id = self.seed_session().await;
        let paths = self.paths();

        {
            let guard = self.guard().await;
            let base = resolve_base(&guard, &paths, None, "main")
                .await
                .expect("the base resolves");
            create_work_clone(&guard, &paths, session_id, &base, &test_identity())
                .await
                .expect("the work clone is created");
        }

        let work = paths.session_work(session_id);
        std::fs::write(work.join("NOTES.md"), format!("{message}\n")).expect("the file is written");
        run_git(&work, &["add", "--", "NOTES.md"]).await;
        run_git(&work, &["commit", "--quiet", "-m", message]).await;
        let commit = run_git(&work, &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_string();

        (session_id, commit)
    }

    /// One more commit in a session's work clone, the way an agent's next
    /// commit arrives: written, committed, and *not* fetched back.
    pub async fn commit_in_work_clone(
        &self,
        session_id: Uuid,
        file: &str,
        content: &str,
    ) -> String {
        let work = self.paths().session_work(session_id);
        std::fs::write(work.join(file), format!("{content}\n")).expect("the file is written");
        run_git(&work, &["add", "--", file]).await;
        run_git(&work, &["commit", "--quiet", "-m", content]).await;

        run_git(&work, &["rev-parse", "HEAD"])
            .await
            .trim()
            .to_string()
    }

    /// A session of a *different* project, for the cross-project refusals.
    ///
    /// The other project needs no repository: a source session that is not
    /// this project's is refused before any git work happens.
    pub async fn session_in_another_project(&self) -> Uuid {
        let projects = ProjectRepository::new(&self.app.pool);

        let new_project = NewProject::new(
            &format!("other-{}", &Uuid::new_v4().simple().to_string()[..8]),
            TEST_REMOTE,
        )
        .expect("the project is valid");

        let mut tx = self.app.pool.begin().await.expect("a transaction begins");
        let other = projects
            .insert(&mut tx, &new_project)
            .await
            .expect("the project inserts");
        let profile = NewAgentProfile::new(other.id, "default", "localhost/mars-session:test")
            .expect("the profile is valid");
        let profile = projects
            .insert_profile(&mut tx, &profile)
            .await
            .expect("the profile inserts");
        tx.commit().await.expect("the transaction commits");

        self.seed_session_in(other.id, profile.id).await
    }

    /// Fetch a session's branch into the project repository, as the revision
    /// publication that produced the hand-off being forwarded would have.
    pub async fn sync(&self, session_id: Uuid) {
        let guard = self.guard().await;
        GitService::from_state(&self.app.state)
            .sync_session_silent(&guard, session_id)
            .await
            .expect("the session branch syncs");
    }

    /// A task of this project in `state`.
    pub async fn task(&self, title: &str, state: &str) -> Task {
        let mut new = NewTask::new(self.project.id, title).expect("the title parses");
        new.state_id = Some(self.state(state).await.id);

        let mut mutation =
            TrackerMutation::begin(&self.app.pool, self.project.id, TaskActor::System)
                .await
                .expect("the mutation opens");
        let inserted = TaskRepository::new(&self.app.pool)
            .insert_task(mutation.conn(), self.project.id, &new)
            .await
            .expect("the task inserts");
        mutation.commit().await.expect("the mutation commits");

        inserted
    }

    pub async fn state(&self, name: &str) -> TaskState {
        TaskRepository::new(&self.app.pool)
            .find_state_by_name(self.project.id, name)
            .await
            .expect("the state reads")
            .expect("the project has this state")
    }

    /// Put a lease on a task, the way a claim would.
    pub async fn claim(&self, task_id: Uuid, session_id: Uuid) -> Task {
        self.set_fields(
            task_id,
            StateFields {
                lease: Some(Some((session_id, Utc::now()))),
                ..StateFields::default()
            },
        )
        .await
    }

    pub async fn set_fields(&self, task_id: Uuid, fields: StateFields) -> Task {
        let mut mutation =
            TrackerMutation::begin(&self.app.pool, self.project.id, TaskActor::System)
                .await
                .expect("the mutation opens");
        let task = TaskRepository::new(&self.app.pool)
            .set_task_state_fields(mutation.conn(), self.project.id, task_id, &fields)
            .await
            .expect("the fields write");
        mutation.commit().await.expect("the mutation commits");

        task
    }

    /// An existing hand-off on `task`, made current: what a forward forwards.
    pub async fn current_handoff(
        &self,
        task: &Task,
        source_session_id: Option<Uuid>,
        source_branch: &str,
        commit: &str,
    ) -> (Task, Uuid) {
        let mut mutation =
            TrackerMutation::begin(&self.app.pool, self.project.id, TaskActor::System)
                .await
                .expect("the mutation opens");
        let repository = TaskRepository::new(&self.app.pool);

        let comment = NewTaskComment::from_user(task.id, self.user.id, "the first revision");
        let comment = repository
            .insert_comment(mutation.conn(), self.project.id, &comment)
            .await
            .expect("the comment inserts");

        let mut handoff = NewTaskHandoff::new(task.id, source_branch, commit, comment.id);
        handoff.source_session_id = source_session_id;
        handoff.created_by_user_id = Some(self.user.id);
        let handoff = repository
            .insert_handoff(mutation.conn(), self.project.id, &handoff)
            .await
            .expect("the hand-off inserts");
        mutation.commit().await.expect("the mutation commits");

        let task = self
            .set_fields(
                task.id,
                StateFields {
                    current_handoff_id: Some(Some(handoff.id)),
                    ..StateFields::default()
                },
            )
            .await;

        (task, handoff.id)
    }

    /// Every retained hand-off ref in the project repository.
    pub async fn handoff_refs(&self) -> Vec<(Uuid, String)> {
        refs::list_handoffs(&self.paths().project_repo(self.project.id))
            .await
            .expect("the hand-off refs list")
    }

    /// The fixture's user, as the HTTP suites act: the same row with a valid
    /// access token minted from the harness configuration.
    ///
    /// The refresh cookie is empty, as it is for every user this harness
    /// created outside a route that sets one (`TestApp::create_gated_user`);
    /// no hand-off scenario refreshes.
    pub fn signed_in(&self) -> AuthenticatedUser {
        AuthenticatedUser {
            user: self.user.clone(),
            access_token: self.app.token_for(&self.user),
            refresh_cookie: String::new(),
        }
    }

    pub fn user_caller(&self) -> HandoffCaller {
        HandoffCaller::User {
            user_id: self.user.id,
        }
    }
}
