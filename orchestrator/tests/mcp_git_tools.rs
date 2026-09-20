//! The four git MCP tools over a real `rmcp` client and real repositories
//! (`SPEC.md`, "MCP tool contracts" → `list_session_branches`, `merge`,
//! `rebase`, `push`).
//!
//! The git operations themselves belong to `GitService` and are asserted
//! against it in `tests/git_service.rs`, and over HTTP in `tests/git_routes.rs`
//! and `tests/handoffs_merge.rs`. What is asserted here is the half these four
//! modules own: that each tool reaches the same service with
//! `GitActor::Session(<the caller>)`, that the arguments an agent may write —
//! the two merge forms, a task *number*, an absent `force` — are read the way
//! the document says, that every failure arrives as the documented
//! `data.code` with `data.conflicts` where the contract has it, and that the
//! outcome events on the calling session are exactly the service's, with
//! nothing added and nothing written by the read-only tool.
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"):
//! `common::handoffs::Fixture` builds a real bare upstream, a real project
//! repository and real session work clones, and the caller is one of those
//! sessions, holding a real bearer token for a profile whose `mcp_tools` name
//! all four tools.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use common::AuthenticatedUser;
use common::handoffs::Fixture;
use common::mcp::McpClient;
use mars_orchestrator::git::testutil::{run_git, test_identity};
use mars_orchestrator::git::{GitActor, GitRef, GitService, create_work_clone, refs, resolve_base};
use mars_orchestrator::models::{SessionState, Task};
use mars_orchestrator::repositories::SessionRepository;
use mars_orchestrator::tracker::TaskDto;
use rmcp::model::ErrorData;
use serde_json::{Value, json};
use uuid::Uuid;

/// The four tools, as a profile that may use all of them names them.
const ALL: &[&str] = &["list_session_branches", "merge", "rebase", "push"];

// ---- the fixture ----

/// A ready project with real repositories, a calling session with a work clone
/// and a bearer token, and an `rmcp` client connected as it.
struct Tools {
    fixture: Fixture,
    client: McpClient,
    /// The session the client authenticates as.
    caller: Uuid,
    user: AuthenticatedUser,
}

impl Tools {
    /// A caller whose profile names `mcp_tools`, with an empty work clone
    /// already made from `main`.
    async fn create(name: &str, mcp_tools: &[&str]) -> Tools {
        let fixture = Fixture::create(name).await;
        let profile_id = fixture
            .app
            .seed_mcp_profile(fixture.project.id, mcp_tools)
            .await;
        let seeded = fixture
            .app
            .seed_mcp_session(fixture.project.id, profile_id, SessionState::Running)
            .await;

        work_clone(&fixture, seeded.session_id).await;

        let client = McpClient::connect(&fixture.app, &seeded.token)
            .await
            .expect("a running session's token authenticates");
        let user = fixture.signed_in();

        Tools {
            fixture,
            client,
            caller: seeded.session_id,
            user,
        }
    }

    /// The usual caller: every tool allowed.
    async fn all(name: &str) -> Tools {
        Self::create(name, ALL).await
    }

    /// `tools/call`, as the agent would make it.
    async fn call(&self, tool: &str, args: Value) -> Result<Value, ErrorData> {
        self.client.call(tool, args).await
    }

    /// One commit in the caller's own work clone, not fetched back.
    async fn commit(&self, file: &str, content: &str) -> String {
        self.fixture
            .commit_in_work_clone(self.caller, file, content)
            .await
    }

    /// A second session of this project with a work clone and one commit in
    /// it: the other line of work a merge or a conflict needs.
    async fn other_session(&self, file: &str, content: &str) -> Uuid {
        let session_id = self.fixture.seed_session().await;
        work_clone(&self.fixture, session_id).await;
        self.fixture
            .commit_in_work_clone(session_id, file, content)
            .await;

        session_id
    }

    /// Merge a session's branch into `main` as a *user*, so that `main` can be
    /// advanced without writing anything on the calling session.
    async fn advance_main(&self, session_id: Uuid) {
        GitService::from_state(&self.fixture.app.state)
            .merge_branch(
                self.fixture.project.id,
                &session_id.to_string(),
                "main",
                None,
                &GitActor::User(self.user.user.id),
            )
            .await
            .expect("the merge into main succeeds");
    }

    /// The commit a ref in the project repository points at, by API name.
    async fn commit_of(&self, name: &str) -> String {
        let git_ref = GitRef::parse(name).expect("a parsable ref name");

        refs::resolve(
            &self.fixture.paths().project_repo(self.fixture.project.id),
            &git_ref,
        )
        .await
        .unwrap_or_else(|err| panic!("{name} resolves: {err}"))
        .commit
    }

    /// The commit a ref in the temporary upstream points at.
    async fn upstream_commit(&self, name: &str) -> String {
        run_git(&self.fixture.upstream.path, &["rev-parse", name])
            .await
            .trim()
            .to_string()
    }

    /// Every path in a ref's tree: how "the merge took A and not B" is
    /// asserted by content rather than by commit id.
    async fn files_in(&self, name: &str) -> Vec<String> {
        run_git(
            &self.fixture.paths().project_repo(self.fixture.project.id),
            &["ls-tree", "--name-only", "-r", name],
        )
        .await
        .lines()
        .map(str::to_string)
        .collect()
    }

    /// The `git` event payloads a session was told about, oldest first.
    async fn git_events(&self, session_id: Uuid) -> Vec<Value> {
        SessionRepository::new(&self.fixture.app.pool)
            .list_events(session_id, None, 100)
            .await
            .expect("the events read")
            .0
            .into_iter()
            .filter(|row| row.kind == "git")
            .map(|row| row.payload)
            .collect()
    }
}

/// A work clone made from `main`, as a launching session's would be.
async fn work_clone(fixture: &Fixture, session_id: Uuid) {
    let guard = fixture.guard().await;
    let paths = fixture.paths();
    let base = resolve_base(&guard, &paths, None, "main")
        .await
        .expect("the base resolves");
    create_work_clone(&guard, &paths, session_id, &base, &test_identity())
        .await
        .expect("the work clone is created");
}

/// The `data.code` every tool failure carries (`SPEC.md`, "MCP tool
/// contracts").
#[track_caller]
fn code(err: &ErrorData) -> String {
    err.data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("every tool error carries data.code: {err:?}"))
        .to_string()
}

/// The `data.conflicts` a merge or rebase that stopped on paths carries.
#[track_caller]
fn conflicts(err: &ErrorData) -> Vec<String> {
    err.data
        .as_ref()
        .and_then(|data| data.get("conflicts"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a stopped merge carries data.conflicts: {err:?}"))
        .iter()
        .map(|path| {
            path.as_str()
                .expect("a conflicting path is a string")
                .to_string()
        })
        .collect()
}

/// A `revision` hand-off body, as `PUT /projects/{pid}/tasks/{id}` takes it.
fn revision(source_session_id: Uuid, commit: &str) -> Value {
    json!({
        "kind": "revision",
        "source_session_id": source_session_id,
        "commit": commit,
        "comment": "ready for review",
    })
}

/// A `forward` hand-off body carrying a review decision.
fn forward(handoff_id: Uuid, review: &str) -> Value {
    json!({
        "kind": "forward",
        "handoff_id": handoff_id,
        "review": review,
        "comment": "reviewed",
    })
}

/// Publish a hand-off through the endpoint that owns it, and answer the
/// hand-off it made current.
async fn publish(tools: &Tools, task: &Task, state: &str, handoff: Value) -> Uuid {
    let response = tools
        .fixture
        .app
        .put_as(
            &tools.user,
            &format!(
                "/api/projects/{}/tasks/{}",
                tools.fixture.project.id, task.id
            ),
        )
        .json(&json!({ "state": state, "handoff": handoff }))
        .await;

    response.assert_status_ok();
    response
        .json::<TaskDto>()
        .handoff
        .expect("the hand-off is published")
        .id
}

// ---- list_session_branches ----

#[tokio::test]
async fn list_session_branches_answers_the_callers_branch_and_writes_no_event() {
    let tools = Tools::all("mcp-branches").await;
    tools.commit("NOTES.md", "work").await;
    tools.fixture.sync(tools.caller).await;

    let output = tools
        .call("list_session_branches", json!({}))
        .await
        .expect("the listing succeeds");

    let branches = output["branches"]
        .as_array()
        .expect("the output is { branches }");
    assert_eq!(branches.len(), 1, "one session, one branch: {branches:?}");
    assert_eq!(branches[0]["session_id"], tools.caller.to_string());
    assert_eq!(
        branches[0]["ref"],
        format!("refs/sessions/{}", tools.caller)
    );
    assert_eq!(branches[0]["ahead"], 1, "{branches:?}");
    assert_eq!(branches[0]["behind"], 0, "{branches:?}");

    // `main` moves on under the caller by exactly one commit, fast-forwarded
    // from another session's work: the counts follow it.
    let other = tools.other_session("OTHER.md", "other").await;
    tools.advance_main(other).await;

    let output = tools
        .call("list_session_branches", json!({}))
        .await
        .expect("the listing succeeds");
    let caller = output["branches"]
        .as_array()
        .expect("the output is { branches }")
        .iter()
        .find(|branch| branch["session_id"] == tools.caller.to_string())
        .expect("the caller's branch is listed")
        .clone();
    assert_eq!(caller["ahead"], 1, "{caller}");
    assert_eq!(caller["behind"], 1, "{caller}");

    // The point of the read (`SPEC.md`: "`list_session_branches` emits no
    // session `git` event"): nothing this tool did is in the transcript.
    assert!(
        tools.git_events(tools.caller).await.is_empty(),
        "the read wrote a git event",
    );
}

// ---- merge, branch form ----

#[tokio::test]
async fn the_branch_form_merges_the_callers_session_and_records_one_event() {
    let tools = Tools::all("mcp-merge").await;
    let commit = tools.commit("NOTES.md", "work").await;

    let output = tools
        .call(
            "merge",
            json!({ "target": "main", "source": tools.caller.to_string() }),
        )
        .await
        .expect("the merge succeeds");

    assert_eq!(output["commit"], commit, "the merge fast-forwards main");
    assert_eq!(tools.commit_of("main").await, commit);

    let events = tools.git_events(tools.caller).await;
    assert_eq!(events.len(), 1, "exactly one outcome event: {events:?}");
    assert_eq!(events[0]["op"], "merge");
    assert_eq!(events[0]["ok"], true);
    assert_eq!(events[0]["detail"]["target"], "main");
    assert_eq!(
        events[0]["detail"]["requested_by"],
        format!("session:{}", tools.caller),
    );
}

#[tokio::test]
async fn a_conflicting_merge_answers_conflict_with_the_paths_and_moves_nothing() {
    let tools = Tools::all("mcp-merge-conflict").await;
    tools.commit("SHARED.md", "mine").await;

    let other = tools.other_session("SHARED.md", "theirs").await;
    tools.advance_main(other).await;
    let main_before = tools.commit_of("main").await;

    let err = tools
        .call(
            "merge",
            json!({ "target": "main", "source": tools.caller.to_string() }),
        )
        .await
        .expect_err("the two lines of work conflict");

    assert_eq!(code(&err), "conflict");
    assert_eq!(conflicts(&err), ["SHARED.md"]);
    assert_eq!(
        tools.commit_of("main").await,
        main_before,
        "a conflicting merge moved the integration head",
    );
}

#[tokio::test]
async fn a_merge_takes_exactly_one_of_the_two_forms() {
    let tools = Tools::all("mcp-merge-form").await;
    let commit = tools.commit("NOTES.md", "work").await;
    let handoff_id = Uuid::new_v4();

    for args in [
        json!({ "target": "main" }),
        json!({ "target": "main", "task_id": 1 }),
        json!({ "target": "main", "handoff_id": handoff_id }),
        json!({
            "target": "main",
            "source": "origin/main",
            "task_id": 1,
            "handoff_id": handoff_id,
        }),
    ] {
        let err = tools
            .call("merge", args.clone())
            .await
            .expect_err("the selection is refused");

        assert_eq!(code(&err), "invalid_argument", "{args}");
        assert_eq!(
            err.message, "merge takes either source or task_id and handoff_id",
            "{args}",
        );
    }

    // Nothing was locked, synced or written on the way to those refusals.
    assert_ne!(tools.commit_of("main").await, commit);
    assert!(tools.git_events(tools.caller).await.is_empty());
}

#[tokio::test]
async fn a_merge_refuses_a_target_that_is_not_an_integration_head() {
    let tools = Tools::all("mcp-merge-refs").await;
    tools.commit("NOTES.md", "work").await;
    let main_before = tools.commit_of("main").await;

    let err = tools
        .call(
            "merge",
            json!({ "target": "origin/main", "source": tools.caller.to_string() }),
        )
        .await
        .expect_err("an upstream-tracking ref is no merge target");

    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(tools.commit_of("main").await, main_before);
}

// ---- merge, task form ----

#[tokio::test]
async fn the_task_form_merges_the_pinned_commit_of_an_approved_handoff() {
    let tools = Tools::all("mcp-merge-task").await;
    let pinned = tools.commit("NOTES.md", "the work").await;
    let task = tools.fixture.task("implement it", "ready").await;

    let handoff_id = publish(&tools, &task, "review", revision(tools.caller, &pinned)).await;
    let approved = publish(&tools, &task, "merge", forward(handoff_id, "approved")).await;

    // The agent keeps working after the hand-off was pinned, and the branch is
    // synced: the merge must still take the pinned commit (ADR 0018).
    tools.commit("LATER.md", "later").await;
    tools.fixture.sync(tools.caller).await;

    // Named by its per-project number in the display syntax an agent copies
    // out of a comment (`SPEC.md`, "MCP tool contracts").
    let output = tools
        .call(
            "merge",
            json!({
                "target": "main",
                "task_id": format!("#{}", task.number),
                "handoff_id": approved,
            }),
        )
        .await
        .expect("the approved hand-off merges");

    assert_eq!(output["commit"], tools.commit_of("main").await);

    let files = tools.files_in("main").await;
    assert!(files.contains(&"NOTES.md".to_string()), "{files:?}");
    assert!(
        !files.contains(&"LATER.md".to_string()),
        "the newer session tip was merged: {files:?}",
    );
}

#[tokio::test]
async fn the_task_form_refuses_an_unapproved_handoff() {
    let tools = Tools::all("mcp-merge-unapproved").await;
    let pinned = tools.commit("NOTES.md", "the work").await;
    let task = tools.fixture.task("implement it", "ready").await;

    let handoff_id = publish(&tools, &task, "review", revision(tools.caller, &pinned)).await;
    let main_before = tools.commit_of("main").await;

    let err = tools
        .call(
            "merge",
            json!({
                "target": "main",
                "task_id": task.id.to_string(),
                "handoff_id": handoff_id,
            }),
        )
        .await
        .expect_err("an unreviewed hand-off may not be merged");

    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message.as_ref(), "hand-off is not approved");
    assert_eq!(tools.commit_of("main").await, main_before);
}

#[tokio::test]
async fn the_task_form_refuses_a_superseded_handoff() {
    let tools = Tools::all("mcp-merge-stale").await;
    let pinned = tools.commit("NOTES.md", "the work").await;
    let task = tools.fixture.task("implement it", "ready").await;

    let handoff_id = publish(&tools, &task, "review", revision(tools.caller, &pinned)).await;
    let approved = publish(&tools, &task, "merge", forward(handoff_id, "approved")).await;

    // A second revision arrives before anyone pressed merge: the approval
    // stands on a record that is no longer current.
    let next = tools.commit("MORE.md", "more").await;
    publish(&tools, &task, "review", revision(tools.caller, &next)).await;

    let main_before = tools.commit_of("main").await;
    let err = tools
        .call(
            "merge",
            json!({
                "target": "main",
                "task_id": task.id.to_string(),
                "handoff_id": approved,
            }),
        )
        .await
        .expect_err("a superseded hand-off may not be merged");

    assert_eq!(code(&err), "conflict");
    assert_eq!(
        err.message.as_ref(),
        "handoff_id is not the task's current hand-off",
    );
    assert_eq!(tools.commit_of("main").await, main_before);
}

#[tokio::test]
async fn the_task_form_answers_not_found_for_a_task_this_project_does_not_have() {
    let tools = Tools::all("mcp-merge-no-task").await;
    tools.commit("NOTES.md", "the work").await;

    let err = tools
        .call(
            "merge",
            json!({
                "target": "main",
                "task_id": 404,
                "handoff_id": Uuid::new_v4(),
            }),
        )
        .await
        .expect_err("a task number nothing carries is not found");

    assert_eq!(code(&err), "not_found");
    assert_eq!(err.message.as_ref(), "task not found");
}

// ---- rebase ----

#[tokio::test]
async fn a_rebase_of_the_callers_branch_updates_its_clean_work_tree() {
    let tools = Tools::all("mcp-rebase").await;
    tools.commit("MINE.md", "mine").await;

    let other = tools.other_session("OTHER.md", "other").await;
    tools.advance_main(other).await;

    let output = tools
        .call(
            "rebase",
            json!({ "branch": tools.caller.to_string(), "onto": "main" }),
        )
        .await
        .expect("the rebase succeeds");

    let commit = output["commit"]
        .as_str()
        .expect("the output is { commit }")
        .to_string();
    assert_eq!(commit, tools.commit_of(&tools.caller.to_string()).await);

    // The clean checkout really moved: the agent's next command sees the
    // rewritten history.
    let work = tools.fixture.paths().session_work(tools.caller);
    assert_eq!(
        run_git(&work, &["rev-parse", "HEAD"]).await.trim(),
        commit,
        "the work tree was left behind",
    );

    let events = tools.git_events(tools.caller).await;
    assert_eq!(events.len(), 1, "exactly one outcome event: {events:?}");
    assert_eq!(events[0]["op"], "rebase");
    assert_eq!(events[0]["ok"], true);
    assert_eq!(events[0]["detail"]["work_tree"], "updated");
}

#[tokio::test]
async fn a_dirty_work_tree_is_reported_as_needing_reconciliation_and_the_rebase_still_answers() {
    let tools = Tools::all("mcp-rebase-dirty").await;
    tools.commit("MINE.md", "mine").await;

    let other = tools.other_session("OTHER.md", "other").await;
    tools.advance_main(other).await;

    // Uncommitted work in the checkout: the mirror's ref is rewritten anyway
    // and somebody has to reconcile the clone.
    let work = tools.fixture.paths().session_work(tools.caller);
    std::fs::write(work.join("DIRTY.md"), "in progress\n").expect("the file is written");
    run_git(&work, &["add", "--", "DIRTY.md"]).await;

    let output = tools
        .call(
            "rebase",
            json!({ "branch": tools.caller.to_string(), "onto": "main" }),
        )
        .await
        .expect("a dirty checkout is not a failed rebase");

    assert_eq!(
        output["commit"],
        tools.commit_of(&tools.caller.to_string()).await,
        "the tool output stays {{ commit }}",
    );

    let events = tools.git_events(tools.caller).await;
    assert_eq!(events.len(), 1, "exactly one outcome event: {events:?}");
    assert_eq!(events[0]["detail"]["work_tree"], "reconciliation_required");
}

#[tokio::test]
async fn a_rebase_refuses_an_upstream_ref_as_the_branch_it_rewrites() {
    let tools = Tools::all("mcp-rebase-refs").await;
    tools.commit("MINE.md", "mine").await;
    let main_before = tools.commit_of("main").await;

    let err = tools
        .call("rebase", json!({ "branch": "origin/main", "onto": "main" }))
        .await
        .expect_err("Mars never rewrites an upstream ref");

    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(tools.commit_of("main").await, main_before);
    assert!(
        tools.git_events(tools.caller).await.is_empty(),
        "a refused request recorded an outcome",
    );
}

#[tokio::test]
async fn a_conflicting_rebase_answers_conflict_with_the_paths() {
    let tools = Tools::all("mcp-rebase-conflict").await;
    tools.commit("SHARED.md", "mine").await;

    let other = tools.other_session("SHARED.md", "theirs").await;
    tools.advance_main(other).await;

    let err = tools
        .call(
            "rebase",
            json!({ "branch": tools.caller.to_string(), "onto": "main" }),
        )
        .await
        .expect_err("the replay stops on the shared path");

    assert_eq!(code(&err), "conflict");
    assert_eq!(conflicts(&err), ["SHARED.md"]);
}

// ---- push ----

#[tokio::test]
async fn a_push_of_an_integration_head_publishes_it_upstream() {
    let tools = Tools::all("mcp-push").await;
    let commit = tools.commit("PUSHED.md", "pushed").await;
    tools.advance_main(tools.caller).await;

    let output = tools
        .call("push", json!({ "ref": "main" }))
        .await
        .expect("the push succeeds");

    assert_eq!(output["remote_branch"], "main");
    assert_eq!(output["commit"], commit);
    assert_eq!(
        tools.upstream_commit("refs/heads/main").await,
        commit,
        "the commit reached the upstream repository",
    );

    let events = tools.git_events(tools.caller).await;
    let push = events
        .iter()
        .find(|payload| payload["op"] == "push")
        .expect("the calling session is told about the push");
    assert_eq!(push["ok"], true);
    assert_eq!(push["detail"]["force"], false);
}

#[tokio::test]
async fn a_session_ref_with_no_remote_branch_lands_on_its_default_name() {
    let tools = Tools::all("mcp-push-session").await;
    let commit = tools.commit("PUSHED.md", "pushed").await;

    let output = tools
        .call("push", json!({ "ref": tools.caller.to_string() }))
        .await
        .expect("the push succeeds");

    assert_eq!(output["remote_branch"], format!("session/{}", tools.caller));
    assert_eq!(output["commit"], commit);
    assert_eq!(
        tools
            .upstream_commit(&format!("refs/heads/session/{}", tools.caller))
            .await,
        commit,
    );
}

#[tokio::test]
async fn a_diverged_upstream_is_a_conflict_that_only_an_explicit_force_gets_past() {
    let tools = Tools::all("mcp-push-rejected").await;
    tools.commit("LOCAL.md", "local").await;
    tools.advance_main(tools.caller).await;
    let main_before = tools.commit_of("main").await;

    // Upstream `main` moves past everything the mirror knows, so sending the
    // integration head to it cannot be a fast-forward.
    let upstream_before = tools
        .fixture
        .upstream
        .commit_file("main", "UPSTREAM.md", "upstream\n", "docs: upstream")
        .await;

    let err = tools
        .call("push", json!({ "ref": "main" }))
        .await
        .expect_err("a diverged upstream rejects the push");

    assert_eq!(code(&err), "conflict");
    assert_eq!(
        tools.commit_of("main").await,
        main_before,
        "a rejected push retains every local ref",
    );
    assert_eq!(
        tools.upstream_commit("refs/heads/main").await,
        upstream_before,
        "a rejected push changed upstream",
    );

    let forced = tools
        .call("push", json!({ "ref": "main", "force": true }))
        .await
        .expect("force: true publishes it");

    assert_eq!(forced["remote_branch"], "main");
    assert_eq!(forced["commit"], main_before);
    assert_eq!(tools.upstream_commit("refs/heads/main").await, main_before);
}

#[tokio::test]
async fn a_push_refuses_a_ref_mars_does_not_publish() {
    let tools = Tools::all("mcp-push-refs").await;
    tools.commit("LOCAL.md", "local").await;
    let upstream_before = tools.upstream_commit("refs/heads/main").await;

    let err = tools
        .call("push", json!({ "ref": "origin/main" }))
        .await
        .expect_err("an upstream-tracking ref is not a push source");

    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(
        tools.upstream_commit("refs/heads/main").await,
        upstream_before,
        "a refused push sent nothing",
    );
    assert!(tools.git_events(tools.caller).await.is_empty());
}

// ---- the profile gate ----

#[tokio::test]
async fn a_profile_without_push_is_refused_the_push_tool() {
    let tools = Tools::create("mcp-push-gated", &["merge"]).await;
    tools.commit("LOCAL.md", "local").await;
    let upstream_before = tools.upstream_commit("refs/heads/main").await;

    let err = tools
        .call("push", json!({ "ref": "main" }))
        .await
        .expect_err("the tool is not in this profile");

    assert_eq!(code(&err), "forbidden");
    assert_eq!(
        err.message.as_ref(),
        "tool push is not allowed for this profile",
    );
    assert_eq!(
        tools.upstream_commit("refs/heads/main").await,
        upstream_before,
        "a refused tool call reached the git layer",
    );
}
