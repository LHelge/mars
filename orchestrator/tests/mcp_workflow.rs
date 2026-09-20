//! The epic's acceptance criteria as one suite: a whole multi-agent workflow
//! driven through the in-process `rmcp` server, and the ADR 0030 side-effect
//! rules audited across every tool it touches.
//!
//! The per-tool suites each pin their own contract. This one pins what only
//! the tools *together* can show: that the four profiles of `ARCHITECTURE.md`,
//! "Task tracker" → "State is a queue" — a planner serving `backlog`, an
//! implementer serving `ready`, a reviewer serving `review`, a merger serving
//! `merge` — really do hand an epic and its children from one to the next over
//! MCP alone; that the worked example of `SPEC.md`, "Code hand-offs and
//! review" holds end to end, commit A and all; that a lost claim, a stale
//! hand-off, a tool outside the profile, a rotated token and an ended session
//! each get the documented answer in the middle of that workflow; and that
//! every read and every rejection along the way leaves tracker history, session
//! links and touch timestamps exactly where they were.
//!
//! Git is never mocked (`CLAUDE.md`, "Testing expectations"): the fixture is a
//! real bare upstream, a real project repository and a real work clone the
//! implementer commits in by shelling out to `git`.
//!
//! # Contract walk: `SPEC.md`, "MCP tool contracts"
//!
//! Every normative sentence of that section, with the suite that covers it.
//! A sentence with no test was given one here.
//!
//! **Preamble.**
//! - Served at `/mcp` over Streamable HTTP, bearer-authenticated per session —
//!   `mcp_server.rs`, `mcp_auth.rs`.
//! - The session context supplies `session_id`, `project_id` and the profile;
//!   handlers take no session id — `mcp_lease_tools.rs` (the actor of every
//!   event), `mcp_workflow.rs` (four sessions, four queues, one listener).
//! - Tools are listed to a session only if allowed for its profile; task tools
//!   are always allowed — `mcp_dispatch.rs`; the mid-workflow half is
//!   `the_profile_gate_opens_on_the_next_call_without_reconnecting`.
//! - Successful tracker changes commit their `TaskEvent` rows and the calling
//!   session's link to the directly changed task together — `mcp_lease_tools.rs`,
//!   `mcp_update.rs`, `mcp_create_task.rs`; across the whole run,
//!   `the_workflow_carries_an_epic_from_backlog_to_done`.
//! - `ready` and `get_task` do not write tracker events, create session-task
//!   links, or advance touch timestamps — `mcp_ready_get.rs`;
//!   `every_read_and_every_rejection_leaves_the_tracker_untouched`.
//! - Rejected tracker operations and updates with no effective changes likewise
//!   leave tracker history and links unchanged — `mcp_update.rs`;
//!   `every_read_and_every_rejection_leaves_the_tracker_untouched`.
//! - `list_session_branches` emits no session `git` event — `mcp_git_tools.rs`.
//! - Git operations retain their existing outcome-event contract —
//!   `mcp_git_tools.rs`.
//! - Tool descriptions are reproduced verbatim — `mcp_descriptions.rs`.
//! - Every `task` argument accepts a UUID or a per-project number as a number
//!   or a string such as `"12"` or `"#12"` — `mcp_ready_get.rs`; the `"#n"`
//!   display form reaches `merge` in `mcp_git_tools.rs` and in this suite.
//!
//! **`ready`.** The served states, the ordering by `priority` then `number`,
//! the exclusion of blocked and held tasks, the empty list for a profile that
//! serves nothing, the 1–100 `limit` and the excerpt rules —
//! `mcp_ready_get.rs`. That four profiles each see only their own queue, and
//! that a task leaves one queue as it enters the next, is this suite's
//! `the_workflow_carries_an_epic_from_backlog_to_done`.
//!
//! **`claim`.** `conflict` "task is not claimable" when the atomic update
//! returns zero rows, and "task is not in a state this profile serves"
//! otherwise; the returned task includes its hand-off; claiming resets no
//! checkout — `mcp_lease_tools.rs`. The lost race between two live sessions is
//! `two_sessions_claiming_one_task_leave_one_winner_and_one_conflict`.
//!
//! **`get_task`.** `{ task: TaskDetail }`, same shape as REST —
//! `mcp_ready_get.rs`; that the detail is where a reviewer reads the hand-off
//! it must forward is this suite's workflow scenario.
//!
//! **`update`.** The lease rule and its five-field creator exception, the
//! unknown state name, the state-change effects, the parent rules, the
//! dependency rules and the cycle — `mcp_update.rs`. The hand-off rules of
//! "Code hand-offs and review" — `mcp_update.rs` for each message;
//! the worked example, revision → forward → merge, is
//! `the_workflow_carries_an_epic_from_backlog_to_done`, and the stale
//! forward is `a_stale_handoff_is_refused_by_the_reviewer_and_by_the_merger`.
//!
//! **`release`, `comment`, `needs_human`** — `mcp_lease_tools.rs`.
//!
//! **`create_task`.** The default state, the unknown state, `depends_on`, the
//! parent rules, `created_by_session_id` and every provenance rule —
//! `mcp_create_task.rs`. The planner's own use of it — an epic with two
//! children whose origin *is* their parent, so no `discovered_from` edge is
//! added — is `the_workflow_carries_an_epic_from_backlog_to_done`.
//!
//! **`list_session_branches`, `merge`, `rebase`, `push`** — `mcp_git_tools.rs`
//! for every form, refusal and gate; the merger's task-form merge of an
//! approved hand-off inside a live workflow is this suite's.
//!
//! **Error codes.** The six codes, their JSON-RPC numbers, `data.code` always
//! present, `data.conflicts` only on a stopped merge or rebase, and `internal`
//! carrying nothing else — `src/mcp/error.rs` unit tests, `mcp_dispatch.rs`
//! and `mcp_git_tools.rs`.
//!
//! Two sentences of the section are about the transport rather than a tool and
//! had no scenario in the middle of a workflow: a token replaced by a relaunch
//! (ADR 0029) and a session that has ended are refused by the bearer
//! middleware, underneath JSON-RPC, with the API's own error body. They are
//! `a_rotated_token_cuts_the_old_client_off_mid_workflow` and
//! `an_ended_session_is_forbidden_on_its_next_call`.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use common::mcp::{McpClient, Role, WorkflowFixture, code, refused, task_of};
use mars_orchestrator::events::{TaskActor, TaskEventKind};
use mars_orchestrator::models::{ReviewStatus, SessionState, TaskDependencyKind};
use mars_orchestrator::session::{SessionDirs, rotate_token};
use mars_orchestrator::tracker::TaskDto;
use rmcp::model::ErrorData;
use serde_json::{Value, json};
use tempfile::TempDir;
use uuid::Uuid;

/// The `MCP_URL` a rotation writes into `mcp.json`; the default of
/// `README.md`, "Configuration".
const MCP_URL: &str = "http://orchestrator:7001/mcp";

/// The conflict a claim of a task somebody else just took answers with
/// (`SPEC.md`, `claim`).
const NOT_CLAIMABLE: &str = "task is not claimable";

/// The conflict forwarding anything but the current record answers with
/// (`SPEC.md`, "Code hand-offs and review").
const NOT_CURRENT: &str = "handoff_id is not the task's current hand-off";

// ---- the calls, audited ----

/// A call that must leave the tracker exactly as it was: every read, and every
/// rejection (`SPEC.md`, "MCP tool contracts"; ADR 0030).
///
/// The fingerprint is taken on both sides of the one call, so a scenario says
/// "this changed nothing" in the line that makes the call rather than in a
/// separate assertion somebody could forget to move.
async fn audited_read(
    w: &WorkflowFixture,
    role: &Role,
    tool: &str,
    args: Value,
) -> Result<Value, ErrorData> {
    let before = w.fingerprint().await;
    let outcome = role.call(tool, args).await;
    let after = w.fingerprint().await;

    before.assert_unchanged(&after, &format!("{}'s {tool}", role.name));

    outcome
}

/// A call that must succeed and reach `touched` and nothing else.
///
/// `touched` is a list because the tracker's own rules carry a change outwards
/// — a new child recomputes its parent's `blocked`, a terminal state closes
/// the parent — and those are the tasks a call is *allowed* to move.
async fn audited_change(
    w: &WorkflowFixture,
    role: &Role,
    tool: &str,
    args: Value,
    touched: &[Uuid],
) -> Value {
    let before = w.fingerprint().await;
    let outcome = role
        .call(tool, args)
        .await
        .unwrap_or_else(|err| panic!("{}'s {tool} succeeds: {err:?}", role.name));
    let after = w.fingerprint().await;

    before.assert_changed_only(&after, touched, &format!("{}'s {tool}", role.name));

    outcome
}

// ---- the hand-off bodies an agent writes ----

/// A `revision` hand-off as an MCP caller sends it: no `source_session_id`,
/// because it is derived from the calling session (`SPEC.md`, "Code hand-offs
/// and review").
fn revision(commit: &str, comment: &str) -> Value {
    json!({ "kind": "revision", "commit": commit, "comment": comment })
}

/// A `forward` hand-off carrying a review decision.
fn forward(handoff_id: Uuid, review: &str, comment: &str) -> Value {
    json!({
        "kind": "forward",
        "handoff_id": handoff_id,
        "review": review,
        "comment": comment,
    })
}

/// The current hand-off of a task a tool just answered with.
#[track_caller]
fn handoff_of(task: &TaskDto) -> mars_orchestrator::tracker::HandoffDto {
    task.handoff
        .clone()
        .unwrap_or_else(|| panic!("task #{} carries its hand-off", task.number))
}

// ---- the workflow ----

#[tokio::test]
async fn the_workflow_carries_an_epic_from_backlog_to_done() {
    let w = WorkflowFixture::create("mcp-workflow").await;

    // -- the planner serves `backlog` and hands to `ready` --

    let epic = task_of(
        &w.planner
            .call(
                "create_task",
                json!({ "title": "ship the parser", "state": "backlog" }),
            )
            .await
            .expect("the planner files the epic"),
    );
    assert_eq!(epic.state, "backlog");
    // "with no held tasks, omission creates no provenance edge".
    assert!(epic.depends_on.is_empty(), "{:?}", epic.depends_on);

    let claimed = task_of(
        &audited_change(
            &w,
            &w.planner,
            "claim",
            json!({ "task": epic.number }),
            &[epic.id],
        )
        .await,
    );
    assert_eq!(claimed.lease_holder_session_id, Some(w.planner.session_id));

    // Two children of the epic. The planner holds exactly one task, so the
    // origin is inferred — and because that origin *is* the parent, "the
    // parent link already records provenance and no redundant
    // `discovered_from` edge is added".
    let first = task_of(
        &audited_change(
            &w,
            &w.planner,
            "create_task",
            json!({
                "title": "child one",
                "state": "backlog",
                "priority": 1,
                "parent": epic.number,
            }),
            // The new task is not in the earlier fingerprint; the epic is, and
            // gaining a non-terminal child is what moves it.
            &[epic.id],
        )
        .await,
    );
    let second = task_of(
        &audited_change(
            &w,
            &w.planner,
            "create_task",
            json!({
                "title": "child two",
                "state": "backlog",
                "priority": 2,
                "parent": epic.number,
            }),
            &[epic.id],
        )
        .await,
    );

    for child in [&first, &second] {
        assert_eq!(child.parent_id, Some(epic.id), "#{}", child.number);
        assert!(
            !child
                .depends_on
                .iter()
                .any(|edge| edge.kind == TaskDependencyKind::DiscoveredFrom),
            "the origin is the parent, so no second edge: {:?}",
            child.depends_on,
        );
    }

    // The creator path: five fields on a task nobody holds, without a lease.
    let renamed = task_of(
        &audited_change(
            &w,
            &w.planner,
            "update",
            json!({ "task": first.number, "title": "implement the parser" }),
            &[first.id],
        )
        .await,
    );
    assert_eq!(renamed.title, "implement the parser");
    assert_eq!(renamed.lease_holder_session_id, None);

    // The state is *not* one of those five, so the planner claims each child
    // and hands it to the implementer's queue.
    for child in [&first, &second] {
        audited_change(
            &w,
            &w.planner,
            "claim",
            json!({ "task": child.number }),
            &[child.id],
        )
        .await;

        let handed = task_of(
            &audited_change(
                &w,
                &w.planner,
                "update",
                json!({ "task": child.number, "state": "ready" }),
                &[child.id],
            )
            .await,
        );
        assert_eq!(handed.state, "ready");
        assert_eq!(
            handed.lease_holder_session_id, None,
            "a move to another state ends the hold",
        );
    }

    // "A planner therefore works an epic by claiming it, creating its
    // children, and releasing it wherever it is."
    let released = task_of(
        &audited_change(
            &w,
            &w.planner,
            "release",
            json!({ "task": epic.number, "reason": "the children are planned" }),
            &[epic.id],
        )
        .await,
    );
    assert_eq!(released.state, "backlog");
    assert_eq!(released.lease_holder_session_id, None);

    // -- the implementer serves `ready`: both children, in priority order --

    let queue = w.implementer.ready().await;
    assert_eq!(queue.len(), 2, "{queue:?}");
    assert_eq!(queue[0]["number"], first.number, "{queue:?}");
    assert_eq!(queue[1]["number"], second.number, "{queue:?}");

    // -- the first child, all the way round --

    let first_commit = carry_one_child(&w, &first, "PARSER.md").await;

    // Only the second child is left in the implementer's queue.
    let queue = w.implementer.ready().await;
    assert_eq!(queue.len(), 1, "{queue:?}");
    assert_eq!(queue[0]["number"], second.number, "{queue:?}");

    // The merge took the implementer's commit into the integration head.
    assert_eq!(w.commit_of("main").await, first_commit);
    let files = w.files_in("main").await;
    assert!(files.contains(&"PARSER.md".to_string()), "{files:?}");

    // One child open, so the epic is untouched: "a parent is never reopened
    // automatically" and never closed early either.
    let epic_now = w.task(epic.id).await;
    assert_eq!(epic_now.state, "backlog", "the epic closed too early");
    assert_eq!(epic_now.closed_at, None);

    // -- the second child, the same way, and the epic closes itself --

    let second_commit = carry_one_child(&w, &second, "LEXER.md").await;
    assert_eq!(w.commit_of("main").await, second_commit);

    let epic_now = w.task(epic.id).await;
    // "the same transaction moves the parent to the project's terminal state
    // with the lowest position (`done` by default)".
    assert_eq!(epic_now.state, "done");
    assert!(epic_now.closed_at.is_some());

    let closure = w
        .events_of(epic.id)
        .await
        .into_iter()
        .rfind(|event| event.kind == TaskEventKind::StateChanged)
        .expect("the epic's closure is an event");
    assert_eq!(closure.actor, TaskActor::System);
    assert_eq!(closure.to.as_deref(), Some("done"));
    assert_eq!(closure.from.as_deref(), Some("backlog"));
}

/// One child from `ready` to `done`: implementer, reviewer, merger, in the
/// words of `ARCHITECTURE.md`, "State is a queue" and the worked example of
/// `SPEC.md`, "Code hand-offs and review". Answers the commit that reached
/// `main`.
async fn carry_one_child(w: &WorkflowFixture, task: &TaskDto, file: &str) -> String {
    // The implementer takes it, commits, and publishes that exact revision.
    let claimed = task_of(
        &audited_change(
            w,
            &w.implementer,
            "claim",
            json!({ "task": task.number }),
            &[task.id],
        )
        .await,
    );
    assert_eq!(
        claimed.lease_holder_session_id,
        Some(w.implementer.session_id)
    );

    let commit = w.implementer_commit(file, &format!("work on {file}")).await;

    let published = task_of(
        &audited_change(
            w,
            &w.implementer,
            "update",
            json!({
                "task": task.number,
                "state": "review",
                "handoff": revision(&commit, "implemented; the unit tests pass"),
            }),
            &[task.id],
        )
        .await,
    );
    assert_eq!(published.state, "review");
    let revision_handoff = handoff_of(&published);
    assert_eq!(revision_handoff.commit, commit);
    assert_eq!(revision_handoff.review_status, ReviewStatus::Unreviewed);
    assert_eq!(
        revision_handoff.source_session_id,
        Some(w.implementer.session_id),
        "the source is derived from the calling session",
    );

    // The reviewer takes it out of `review` and reads the hand-off before
    // deciding anything.
    let queue = w.reviewer.ready().await;
    assert!(
        queue.iter().any(|entry| entry["number"] == task.number),
        "{queue:?}",
    );

    audited_change(
        w,
        &w.reviewer,
        "claim",
        json!({ "task": task.number }),
        &[task.id],
    )
    .await;

    let detail = audited_read(w, &w.reviewer, "get_task", json!({ "task": task.number }))
        .await
        .expect("the reviewer reads the task");
    assert_eq!(
        detail["task"]["handoff"]["id"],
        revision_handoff.id.to_string(),
    );
    assert_eq!(detail["task"]["handoff"]["commit"], commit);

    // "The reviewer launches from A and forwards that hand-off to `merge` with
    // `review: "approved"`."
    let approved = task_of(
        &audited_change(
            w,
            &w.reviewer,
            "update",
            json!({
                "task": task.number,
                "state": "merge",
                "handoff": forward(revision_handoff.id, "approved", "reads right"),
            }),
            &[task.id],
        )
        .await,
    );
    assert_eq!(approved.state, "merge");
    let approved_handoff = handoff_of(&approved);
    assert_eq!(
        approved_handoff.commit, commit,
        "forwarding reuses the pinned commit",
    );
    assert_eq!(approved_handoff.review_status, ReviewStatus::Approved);
    assert_eq!(
        approved_handoff.source_session_id,
        Some(w.implementer.session_id),
        "forwarding retains the original source session",
    );
    assert_eq!(
        approved_handoff.reviewed_by_session_id,
        Some(w.reviewer.session_id),
    );

    // The implementer keeps working after the approval: the merge must still
    // take the pinned commit (`SPEC.md`: "even if the implementer's branch has
    // since advanced to B").
    let later = w
        .implementer_commit(&format!("LATER-{file}"), "later work")
        .await;
    assert_ne!(later, commit);

    // The merger serves `merge` and closes.
    audited_change(
        w,
        &w.merger,
        "claim",
        json!({ "task": task.number }),
        &[task.id],
    )
    .await;

    // A git tool: it writes its own session `git` event and no tracker history
    // at all (ADR 0030).
    let merged = audited_read(
        w,
        &w.merger,
        "merge",
        json!({
            "target": "main",
            // The display form an agent copies out of a comment.
            "task_id": format!("#{}", task.number),
            "handoff_id": approved_handoff.id,
        }),
    )
    .await
    .expect("the approved hand-off merges");

    let head = w.commit_of("main").await;
    assert_eq!(merged["commit"], head);
    assert_eq!(
        head, commit,
        "the merge took the pinned commit, not the tip"
    );

    let closed = task_of(
        &audited_change(
            w,
            &w.merger,
            "update",
            json!({ "task": task.number, "state": "done" }),
            // A terminal child recomputes its parent, which may close with it.
            &[task.id, task.parent_id.expect("the children have a parent")],
        )
        .await,
    );
    assert_eq!(closed.state, "done");
    assert!(closed.closed_at.is_some());

    commit
}

// ---- the lost claim ----

#[tokio::test]
async fn two_sessions_claiming_one_task_leave_one_winner_and_one_conflict() {
    let w = WorkflowFixture::create("mcp-workflow-race").await;
    let task = w.git.task("implement it", "ready").await;

    // Two distinct MCP sessions of the implementer profile, each with its own
    // connection: a lease is per session, so one client could never lose this
    // race to itself.
    let (other, other_session) = w.second_session(w.implementer.profile_id).await;
    let args = json!({ "task": task.number });

    let (one, two) = tokio::join!(
        w.implementer.client.call("claim", args.clone()),
        other.call("claim", args.clone()),
    );

    let (winner, loser) = match (one, two) {
        (Ok(won), Err(lost)) => (task_of(&won), lost),
        (Err(lost), Ok(won)) => (task_of(&won), lost),
        (Ok(a), Ok(b)) => panic!("both claims won: {a} and {b}"),
        (Err(a), Err(b)) => panic!("both claims lost: {a:?} and {b:?}"),
    };

    assert_eq!(code(&loser), "conflict");
    assert_eq!(loser.message, NOT_CLAIMABLE);

    let holder = winner
        .lease_holder_session_id
        .expect("the winner holds the task");
    assert!([w.implementer.session_id, other_session].contains(&holder));
    assert_eq!(winner.attempts, 1, "the loser counted no attempt");

    let claims: Vec<_> = w
        .events_of(task.id)
        .await
        .into_iter()
        .filter(|event| event.kind == TaskEventKind::Claimed)
        .collect();
    assert_eq!(claims.len(), 1, "exactly one claim was written: {claims:?}");
    assert_eq!(claims[0].actor, TaskActor::Session { session_id: holder });

    // One winner, one link.
    let links = w.fingerprint().await.links;
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0].1, holder);
}

// ---- the stale hand-off ----

#[tokio::test]
async fn a_stale_handoff_is_refused_by_the_reviewer_and_by_the_merger() {
    let w = WorkflowFixture::create("mcp-workflow-stale").await;
    let task = w.git.task("implement it", "ready").await;

    // Revision A.
    w.implementer
        .call("claim", json!({ "task": task.number }))
        .await
        .expect("the implementer claims");
    let commit_a = w.implementer_commit("A.md", "first attempt").await;
    let published = task_of(
        &w.implementer
            .call(
                "update",
                json!({
                    "task": task.number,
                    "state": "review",
                    "handoff": revision(&commit_a, "first attempt"),
                }),
            )
            .await
            .expect("revision A publishes"),
    );
    let handoff_a = handoff_of(&published);

    // The reviewer sends it back: the hand-off is intact and the decision is
    // recorded on it (`SPEC.md`: "A rejection forwards A with
    // `changes_requested` to the chosen implementation state").
    w.reviewer
        .call("claim", json!({ "task": task.number }))
        .await
        .expect("the reviewer claims");
    let rejected = task_of(
        &w.reviewer
            .call(
                "update",
                json!({
                    "task": task.number,
                    "state": "ready",
                    "handoff": forward(handoff_a.id, "changes_requested", "needs a test"),
                }),
            )
            .await
            .expect("the rejection forwards"),
    );
    assert_eq!(rejected.state, "ready");
    let rejected_handoff = handoff_of(&rejected);
    assert_eq!(rejected_handoff.commit, commit_a);
    assert_eq!(
        rejected_handoff.review_status,
        ReviewStatus::ChangesRequested,
    );

    // Revision B. "A new revision always resets review status to
    // `unreviewed`."
    w.implementer
        .call("claim", json!({ "task": task.number }))
        .await
        .expect("the implementer re-claims");
    let commit_b = w.implementer_commit("B.md", "second attempt").await;
    let republished = task_of(
        &w.implementer
            .call(
                "update",
                json!({
                    "task": task.number,
                    "state": "review",
                    "handoff": revision(&commit_b, "added the test"),
                }),
            )
            .await
            .expect("revision B publishes"),
    );
    let handoff_b = handoff_of(&republished);
    assert_ne!(handoff_b.id, rejected_handoff.id);
    assert_eq!(handoff_b.commit, commit_b);
    assert_eq!(handoff_b.review_status, ReviewStatus::Unreviewed);

    // The reviewer forwarding A's id now forwards something that is no longer
    // current, and the refusal changes nothing.
    w.reviewer
        .call("claim", json!({ "task": task.number }))
        .await
        .expect("the reviewer claims again");

    let err = refused(
        audited_read(
            &w,
            &w.reviewer,
            "update",
            json!({
                "task": task.number,
                "state": "merge",
                "handoff": forward(handoff_a.id, "approved", "approving the wrong one"),
            }),
        )
        .await,
    );
    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message.as_ref(), NOT_CURRENT);
    assert_eq!(
        handoff_of(&w.task(task.id).await).id,
        handoff_b.id,
        "the refused forward moved the current hand-off",
    );

    // The merger naming A's id meets the same rule on the git side.
    let main_before = w.commit_of("main").await;
    let err = refused(
        audited_read(
            &w,
            &w.merger,
            "merge",
            json!({
                "target": "main",
                "task_id": task.number,
                "handoff_id": handoff_a.id,
            }),
        )
        .await,
    );
    assert_eq!(code(&err), "conflict");
    assert_eq!(err.message.as_ref(), NOT_CURRENT);
    assert_eq!(w.commit_of("main").await, main_before);

    // B approved is what merges.
    let approved = task_of(
        &w.reviewer
            .call(
                "update",
                json!({
                    "task": task.number,
                    "state": "merge",
                    "handoff": forward(handoff_b.id, "approved", "the test is there"),
                }),
            )
            .await
            .expect("the reviewer approves B"),
    );
    let approved_handoff = handoff_of(&approved);
    assert_eq!(approved_handoff.commit, commit_b);
    assert_eq!(approved_handoff.review_status, ReviewStatus::Approved);

    let merged = w
        .merger
        .call(
            "merge",
            json!({
                "target": "main",
                "task_id": task.number,
                "handoff_id": approved_handoff.id,
            }),
        )
        .await
        .expect("the approved hand-off merges");
    assert_eq!(merged["commit"], commit_b);
    assert_eq!(w.commit_of("main").await, commit_b);
}

// ---- the profile gate, mid-workflow ----

#[tokio::test]
async fn the_profile_gate_opens_on_the_next_call_without_reconnecting() {
    let w = WorkflowFixture::create("mcp-workflow-gate").await;
    let task = w.git.task("implement it", "ready").await;

    w.implementer
        .call("claim", json!({ "task": task.number }))
        .await
        .expect("the implementer claims");
    let commit = w.implementer_commit("WORK.md", "work").await;
    let main_before = w.commit_of("main").await;

    // The implementer's profile names no git tool at all.
    let err = refused(
        audited_read(
            &w,
            &w.implementer,
            "merge",
            json!({ "target": "main", "source": w.implementer.session_id.to_string() }),
        )
        .await,
    );
    assert_eq!(code(&err), "forbidden");
    assert_eq!(
        err.message.as_ref(),
        "tool merge is not allowed for this profile",
    );
    assert_eq!(
        w.commit_of("main").await,
        main_before,
        "a refused tool call reached the git layer",
    );

    // An operator adds it. The profile is read per request, so the very next
    // call on the *same* connection is allowed.
    w.set_mcp_tools(w.implementer.profile_id, &["merge"]).await;

    let merged = w
        .implementer
        .call(
            "merge",
            json!({ "target": "main", "source": w.implementer.session_id.to_string() }),
        )
        .await
        .expect("the newly allowed tool runs without a reconnection");

    assert_eq!(merged["commit"], commit);
    assert_eq!(w.commit_of("main").await, commit);
}

// ---- the transport, mid-workflow ----

#[tokio::test]
async fn a_rotated_token_cuts_the_old_client_off_mid_workflow() {
    let w = WorkflowFixture::create("mcp-workflow-rotate").await;
    let task = w.git.task("implement it", "ready").await;

    let claimed = task_of(
        &w.implementer
            .call("claim", json!({ "task": task.number }))
            .await
            .expect("the implementer claims"),
    );
    assert_eq!(
        claimed.lease_holder_session_id,
        Some(w.implementer.session_id)
    );

    // The relaunch path: a session is parked before it is relaunched, and the
    // relaunch generates a fresh token and commits its hash (ADR 0029). The
    // lease survives, because "a lease is valid while its holder is alive" and
    // `parked` counts.
    w.set_session_state(w.implementer.session_id, SessionState::Parked)
        .await;

    let data = TempDir::new().expect("a temporary data directory");
    let dirs = SessionDirs::for_session(data.path(), data.path(), w.implementer.session_id);
    dirs.ensure().await.expect("the directories are created");
    let rotated = rotate_token(w.pool(), &dirs, MCP_URL, w.implementer.session_id)
        .await
        .expect("a parked session rotates");

    // The old client is cut off underneath JSON-RPC: nothing is cached, so the
    // very next request fails at the middleware.
    let before = w.fingerprint().await;
    let err = w
        .implementer
        .client
        .try_call("get_task", json!({ "task": task.number }))
        .await
        .expect_err("the replaced token no longer authenticates");
    assert_eq!(
        err.status().map(|status| status.as_u16()),
        Some(401),
        "{err}",
    );
    before.assert_unchanged(&w.fingerprint().await, "a refused request");

    // The relaunched process carries on as the same session, still holding it.
    let relaunched = McpClient::connect(w.app(), rotated.expose())
        .await
        .expect("the replacement token authenticates");

    let detail = relaunched
        .call("get_task", json!({ "task": task.number }))
        .await
        .expect("the new client reads the task");
    assert_eq!(
        detail["task"]["lease_holder_session_id"],
        w.implementer.session_id.to_string(),
        "the rotation dropped the lease",
    );

    let commit = w.implementer_commit("WORK.md", "work").await;
    let published = task_of(
        &relaunched
            .call(
                "update",
                json!({
                    "task": task.number,
                    "state": "review",
                    "handoff": revision(&commit, "finished after the relaunch"),
                }),
            )
            .await
            .expect("the relaunched session hands the work on"),
    );
    assert_eq!(published.state, "review");
    assert_eq!(handoff_of(&published).commit, commit);
}

#[tokio::test]
async fn an_ended_session_is_forbidden_on_its_next_call() {
    let w = WorkflowFixture::create("mcp-workflow-ended").await;
    let task = w.git.task("review it", "review").await;

    // The reviewer is live and working.
    w.reviewer
        .call("claim", json!({ "task": task.number }))
        .await
        .expect("the reviewer claims");

    // Then its session ends.
    w.set_session_state(w.reviewer.session_id, SessionState::Done)
        .await;

    let before = w.fingerprint().await;
    let err = w
        .reviewer
        .client
        .try_call("get_task", json!({ "task": task.number }))
        .await
        .expect_err("a session that has ended is refused");
    assert_eq!(
        err.status().map(|status| status.as_u16()),
        Some(403),
        "{err}",
    );
    before.assert_unchanged(&w.fingerprint().await, "a refused request");
}

// ---- the ADR 0030 audit, over every tool in one run ----

#[tokio::test]
async fn every_read_and_every_rejection_leaves_the_tracker_untouched() {
    let w = WorkflowFixture::create("mcp-workflow-audit").await;
    let task = w.git.task("implement it", "ready").await;

    // A first touch, so the run has a `task_sessions` link whose
    // `first_touched_at` the later calls must preserve.
    audited_change(
        &w,
        &w.implementer,
        "claim",
        json!({ "task": task.number }),
        &[task.id],
    )
    .await;
    let first_touch = w.fingerprint().await;

    // Every read-only tool, from three different profiles.
    audited_read(&w, &w.planner, "ready", json!({}))
        .await
        .expect("ready answers");
    audited_read(&w, &w.implementer, "ready", json!({ "limit": 5 }))
        .await
        .expect("ready answers");
    audited_read(
        &w,
        &w.implementer,
        "get_task",
        json!({ "task": task.number }),
    )
    .await
    .expect("get_task answers");
    audited_read(&w, &w.merger, "list_session_branches", json!({}))
        .await
        .expect("the merger may list branches");

    // Every shape of rejection the tools can produce over a tracker call.
    let rejections: Vec<(&Role, &str, Value, &str)> = vec![
        // `unauthorized` and `forbidden` at the transport are
        // `a_rotated_token_...` and `an_ended_session_...`; this is the
        // profile gate, which is the tool layer's own `forbidden`.
        (
            &w.implementer,
            "merge",
            json!({ "target": "main", "source": "main" }),
            "forbidden",
        ),
        (
            &w.merger,
            "claim",
            json!({ "task": task.number }),
            // The merger serves `merge`, and the task is in `ready`.
            "conflict",
        ),
        (
            &w.reviewer,
            "release",
            json!({ "task": task.number, "reason": "not mine" }),
            "conflict",
        ),
        (
            &w.implementer,
            "update",
            json!({ "task": task.number, "state": "nonexistent" }),
            "invalid_argument",
        ),
        (
            &w.implementer,
            "update",
            json!({ "task": task.number }),
            "invalid_argument",
        ),
        // An `add_depends_on` entry naming no task is the generic `not_found`
        // the REST dependency endpoint gives an unknown `{dep}`, while the
        // same mistake in `create_task`'s `depends_on` is the 400 that
        // endpoint's own body gives. Two endpoints, two answers, and each tool
        // keeps the one of the endpoint it mirrors (`SPEC.md`, "Tasks").
        (
            &w.implementer,
            "update",
            json!({ "task": task.number, "add_depends_on": [4040] }),
            "not_found",
        ),
        (
            &w.implementer,
            "create_task",
            json!({ "title": "the cutover", "depends_on": [4040] }),
            "invalid_argument",
        ),
        (
            &w.implementer,
            "update",
            json!({ "task": task.number, "remove_depends_on": [task.number] }),
            "not_found",
        ),
        (
            &w.implementer,
            "create_task",
            json!({ "title": "", "state": "ready" }),
            "invalid_argument",
        ),
        (
            &w.implementer,
            "create_task",
            json!({ "title": "orphan", "parent": 4040 }),
            "invalid_argument",
        ),
        (
            &w.implementer,
            "get_task",
            json!({ "task": 4040 }),
            "not_found",
        ),
        (
            &w.implementer,
            "claim",
            json!({ "task": 12.5 }),
            "invalid_argument",
        ),
        (
            &w.implementer,
            "ready",
            json!({ "limit": 0 }),
            "invalid_argument",
        ),
    ];

    for (role, tool, args, expected) in rejections {
        let err = refused(audited_read(&w, role, tool, args.clone()).await);
        assert_eq!(code(&err), expected, "{} {tool} {args}", role.name);
    }

    // An update with no effective change is neither a rejection nor a write.
    audited_read(
        &w,
        &w.implementer,
        "update",
        json!({ "task": task.number, "title": task.title }),
    )
    .await
    .expect("setting a field to what it already is succeeds");

    // And a second successful touch by the same session keeps the moment the
    // link was first made.
    let before = w.fingerprint().await;
    w.implementer
        .call(
            "comment",
            json!({ "task": task.number, "body": "still working on it" }),
        )
        .await
        .expect("the comment lands");
    let after = w.fingerprint().await;
    before.assert_changed_only(&after, &[task.id], "a repeat touch");

    let link = after
        .links
        .iter()
        .find(|(_, session, _, _)| *session == w.implementer.session_id)
        .expect("the implementer is linked to the task");
    let original = first_touch
        .links
        .iter()
        .find(|(_, session, _, _)| *session == w.implementer.session_id)
        .expect("the claim linked it");
    assert_eq!(
        link.2, original.2,
        "first_touched_at moved on a repeat touch"
    );
    assert!(link.3 >= original.3, "last_touched_at went backwards");
}
