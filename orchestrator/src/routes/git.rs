//! `/api/projects/{pid}/git` (`SPEC.md`, "Git (`/api/projects/{pid}/git`)").
//!
//! The seven endpoints the UI drives git with, and nothing more: every rule
//! about refs, locking, syncing, credentials and outcome events belongs to
//! [`GitService`], which is the one code path humans and agents share (ADR
//! 0007). A handler here parses HTTP, names the actor and hands the request
//! through, so the same request made over MCP reaches the same code with
//! [`GitActor::Session`] instead of [`GitActor::User`].
//!
//! What this module does decide is only what HTTP owns — the shape of a body
//! or a query string:
//!
//! - a merge takes `source` **or** `task_id` and `handoff_id`, never both and
//!   never neither ([`merge_selection`]);
//! - a diff takes exactly one of `head` and `handoff_id`
//!   ([`DiffQuery::selector`]);
//! - a merge `message` is at most [`MAX_MESSAGE_BYTES`];
//! - `force` defaults to `false`, so a force-push is always something the
//!   caller asked for in as many words (`SPEC.md`, "Git").
//!
//! The revert is the one endpoint that reaches past [`GitService`]: reverting
//! may also reopen tasks, so it goes through
//! [`revert_and_reopen`](crate::tracker::revert_and_reopen), which holds the
//! git lock across the revert and the tracker mutation (ADR 0053).
//!
//! Everything else already answers the documented status: an unknown project
//! is 404 and one that is not `ready` is 409, both decided before any git work
//! happens; a ref of the wrong kind — an upstream-tracking name as a merge
//! target, a rebase `branch` or a push `ref` — is 400, decided before anything
//! is locked or written; a non-fast-forward push is 409 with every local ref
//! preserved; and a merge or rebase conflict is 422 carrying `conflicts`,
//! because `From<GitError>` turns it into [`Error::GitConflict`], whose body
//! is the documented `{ status, error, conflicts }`.
//!
//! **Nothing here logs a body.** The entry lines carry the project id and the
//! operation, and the git layer logs refs; a commit message is user text and a
//! `conflicts` list is a path list, and neither belongs in the orchestrator's
//! own log (`CLAUDE.md`, rule 3 and "Backend conventions").

use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::git::{DiffSelector, GitActor, GitService};
use crate::models::{Diff, HistoryEntry, SessionBranch};
use crate::prelude::*;
use crate::routes::{CurrentUser, Path, Query};
use crate::tracker::{Reopen, RevertRequest, TaskDto, revert_and_reopen};

/// The longest merge commit message a caller may supply.
///
/// Ten kibibytes is far more than a merge message ever needs and far less than
/// an accident or an abuse costs: the string reaches `git commit -m`, and the
/// limit is checked before anything is locked.
const MAX_MESSAGE_BYTES: usize = 10 * 1024;

/// What a merge body that is neither documented alternative is told (400).
///
/// `pub(crate)` because the `merge` MCP tool answers the same sentence for the
/// same mistake (`SPEC.md`, "MCP tool contracts" → `merge`: "`MergeInput`
/// (same as REST)"), and one spelling of it is one fewer string for a caller
/// to have to match twice.
pub(crate) const MERGE_FORM: &str = "merge takes either source or task_id and handoff_id";

/// What a diff request that selects neither or both heads is told (400).
const DIFF_FORM: &str = "diff takes either head or handoff_id";

/// The router nested under `/api/projects`.
///
/// The `{pid}` capture is part of these paths rather than of the `nest`, so
/// the projects epic can merge its own `/projects` router beside this one
/// (`routes::mod`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/{pid}/git/session-branches", get(session_branches))
        .route("/{pid}/git/history", get(history))
        .route("/{pid}/git/diff", get(diff))
        .route("/{pid}/git/merge", post(merge))
        .route("/{pid}/git/rebase", post(rebase))
        .route("/{pid}/git/push", post(push))
        .route("/{pid}/git/revert", post(revert))
}

/// The service for this request, built as every other caller builds it.
fn service(state: &AppState) -> GitService {
    GitService::from_state(state)
}

// ---- session branches ----

/// `GET /projects/{pid}/git/session-branches` → every session branch with its
/// ahead/behind counts.
async fn session_branches(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(pid): Path<Uuid>,
) -> Result<Json<Vec<SessionBranch>>> {
    debug!(project_id = %pid, op = "session-branches", "git request");

    Ok(Json(service(&state).list_session_branches(pid).await?))
}

// ---- history ----

/// The page size a history request without `limit` gets (`SPEC.md`, "Git").
const DEFAULT_HISTORY_LIMIT: u32 = 50;

/// The largest page a history request gets; a larger `limit` is reduced to
/// it rather than refused, as `GET /secrets/{id}/uses` does.
const MAX_HISTORY_LIMIT: u32 = 200;

/// `?branch=&before=&limit=` (`SPEC.md`, "Git").
#[derive(Debug, Deserialize)]
struct HistoryQuery {
    /// An integration head; the project's default branch when absent.
    branch: Option<String>,
    /// The previous page's last commit.
    before: Option<String>,
    /// The page size.
    limit: Option<u32>,
}

/// The page size a `limit` asks for: [`DEFAULT_HISTORY_LIMIT`] when absent,
/// at most [`MAX_HISTORY_LIMIT`], and 400 for zero.
fn history_limit(limit: Option<u32>) -> Result<u32> {
    match limit {
        None => Ok(DEFAULT_HISTORY_LIMIT),
        Some(0) => Err(Error::BadRequest("limit must be at least 1".to_string())),
        Some(limit) => Ok(limit.min(MAX_HISTORY_LIMIT)),
    }
}

/// `GET /projects/{pid}/git/history` → the integration head's first-parent
/// history, newest first, attributed to tasks and sessions.
async fn history(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(pid): Path<Uuid>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<HistoryEntry>>> {
    debug!(project_id = %pid, op = "history", "git request");

    let limit = history_limit(query.limit)?;
    let entries = service(&state)
        .history(pid, query.branch.as_deref(), query.before.as_deref(), limit)
        .await?;

    Ok(Json(entries))
}

// ---- diff ----

/// `GET /projects/{pid}/git/diff?head=&base=` or `?handoff_id=&base=`.
///
/// Both selectors are optional in the type and exactly one of them is required
/// by [`DiffQuery::selector`], which is the only way to say "one of these two"
/// in a query string. `base` is optional everywhere: absent means the
/// project's `default_branch`, which the service supplies.
#[derive(Debug, Deserialize)]
struct DiffQuery {
    head: Option<String>,
    handoff_id: Option<Uuid>,
    base: Option<String>,
}

impl DiffQuery {
    /// The one side the caller selected, or 400 (`SPEC.md`, "Git": "Diff
    /// accepts exactly one of `head` or `handoff_id`").
    fn selector(&self) -> Result<DiffSelector> {
        match (&self.head, self.handoff_id) {
            (Some(head), None) => Ok(DiffSelector::Head(head.clone())),
            (None, Some(handoff_id)) => Ok(DiffSelector::Handoff(handoff_id)),
            _ => Err(Error::BadRequest(DIFF_FORM.to_string())),
        }
    }
}

/// `GET /projects/{pid}/git/diff` → the patch from the merge base to the head.
async fn diff(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(pid): Path<Uuid>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<Diff>> {
    debug!(project_id = %pid, op = "diff", "git request");

    let selector = query.selector()?;
    let diff = service(&state)
        .diff(pid, selector, query.base.as_deref())
        .await?;

    Ok(Json(diff))
}

// ---- merge ----

/// `MergeInput = { target, message? } & ({ source } | { task_id, handoff_id })`
/// (`SPEC.md`, "Git").
///
/// Flat rather than an untagged enum: serde's untagged errors describe none of
/// the alternatives usefully, and the rule is not "one of two shapes" but "one
/// of two *selections*" — which [`merge_selection`] states in one line and
/// answers with the documented message.
///
/// No `deny_unknown_fields`, deliberately: the three selection fields are
/// mutually exclusive rather than unknown, and a client sending all of them is
/// told what the rule is instead of which field was surplus.
#[derive(Debug, Deserialize)]
struct MergeBody {
    /// The integration head to merge into. A session ref or an
    /// upstream-tracking name here is 400, which the service decides.
    target: String,
    /// The merge commit message; the bot's default when absent.
    message: Option<String>,
    /// The branch form: any ref a merge may take its source from.
    source: Option<String>,
    /// The task form, first half. A UUID: a task *number* is MCP-only
    /// (`SPEC.md`, "MCP tool contracts").
    task_id: Option<Uuid>,
    /// The task form, second half: the hand-off whose pinned commit is merged.
    handoff_id: Option<Uuid>,
}

/// Which of the two documented merge forms the body selected.
#[derive(Debug)]
enum MergeSelection {
    /// `{ source }`: an explicit git action that records no task approval
    /// (`ARCHITECTURE.md`, "Git model").
    Branch(String),
    /// `{ task_id, handoff_id }`: merges the hand-off's pinned commit, once it
    /// has been verified current and approved under the git lock.
    Handoff { task_id: Uuid, handoff_id: Uuid },
}

/// The selection, or 400.
///
/// Every mixture is refused, including a half task form: `{ task_id }` alone
/// names no commit and `{ source, task_id }` asks for two different merges.
fn merge_selection(body: &MergeBody) -> Result<MergeSelection> {
    match (&body.source, body.task_id, body.handoff_id) {
        (Some(source), None, None) => Ok(MergeSelection::Branch(source.clone())),
        (None, Some(task_id), Some(handoff_id)) => Ok(MergeSelection::Handoff {
            task_id,
            handoff_id,
        }),
        _ => Err(Error::BadRequest(MERGE_FORM.to_string())),
    }
}

/// A merge commit message that is within the limit (400 above it).
///
/// `pub(crate)` for the same reason as [`MERGE_FORM`]: the `merge` MCP tool
/// takes the same input and so has the same limit, and a second copy of the
/// number is a second thing to keep in step.
pub(crate) fn checked_message(message: Option<&String>) -> Result<Option<&str>> {
    match message {
        Some(message) if message.len() > MAX_MESSAGE_BYTES => Err(Error::BadRequest(format!(
            "message is longer than {MAX_MESSAGE_BYTES} bytes"
        ))),
        other => Ok(other.map(String::as_str)),
    }
}

/// The commit an integration operation left behind: what both `merge` and
/// `rebase` answer (`SPEC.md`, "Git").
#[derive(Debug, Serialize)]
struct CommitResponse {
    commit: String,
}

/// `POST /projects/{pid}/git/merge` → `{ commit }`, 422 with `conflicts`, or
/// 409 for a stale or unapproved task hand-off.
async fn merge(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(pid): Path<Uuid>,
    Json(body): Json<MergeBody>,
) -> Result<Json<CommitResponse>> {
    debug!(project_id = %pid, op = "merge", "git request");

    let selection = merge_selection(&body)?;
    let message = checked_message(body.message.as_ref())?;
    let actor = GitActor::User(user.id);
    let service = service(&state);

    let outcome = match selection {
        MergeSelection::Branch(source) => {
            service
                .merge_branch(pid, &source, &body.target, message, &actor)
                .await?
        }
        MergeSelection::Handoff {
            task_id,
            handoff_id,
        } => {
            service
                .merge_handoff(pid, task_id, handoff_id, &body.target, message, &actor)
                .await?
        }
    };

    Ok(Json(CommitResponse {
        commit: outcome.commit,
    }))
}

// ---- rebase ----

/// `{ branch, onto }` (`SPEC.md`, "Git").
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RebaseBody {
    /// An integration head or a session ref; an upstream-tracking name is 400.
    branch: String,
    /// An integration head or an upstream-tracking name; a session ref is 400.
    onto: String,
}

/// `POST /projects/{pid}/git/rebase` → `{ commit }`, or 422 with `conflicts`.
async fn rebase(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(pid): Path<Uuid>,
    Json(body): Json<RebaseBody>,
) -> Result<Json<CommitResponse>> {
    debug!(project_id = %pid, op = "rebase", "git request");

    let outcome = service(&state)
        .rebase(pid, &body.branch, &body.onto, &GitActor::User(user.id))
        .await?;

    Ok(Json(CommitResponse {
        commit: outcome.commit,
    }))
}

// ---- push ----

/// `{ ref, remote_branch?, force?: boolean = false }` (`SPEC.md`, "Git").
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PushBody {
    /// An integration head or a session ref: the only two namespaces Mars
    /// publishes (ADR 0007). `ref` is a keyword in Rust and a field name on
    /// the wire.
    #[serde(rename = "ref")]
    git_ref: String,
    /// The upstream branch to update; the source's own name when absent.
    remote_branch: Option<String>,
    /// Absent means `false`: "REST force-pushes require explicit `force:
    /// true`" (`SPEC.md`, "Git"), so only the word in the body adds `--force`.
    #[serde(default)]
    force: bool,
}

/// `POST /projects/{pid}/git/push` → `{ remote_branch, commit }`, or 409 when
/// upstream has moved on.
///
/// The compare link the outcome also carries is not part of this body: it
/// travels in the `git` event, which is where the UI reads it from
/// (`SPEC.md`, "Git").
#[derive(Debug, Serialize)]
struct PushResponse {
    remote_branch: String,
    commit: String,
}

/// `POST /projects/{pid}/git/push` → the branch that moved and the commit it
/// now holds.
async fn push(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(pid): Path<Uuid>,
    Json(body): Json<PushBody>,
) -> Result<Json<PushResponse>> {
    debug!(project_id = %pid, op = "push", force = body.force, "git request");

    let outcome = service(&state)
        .push(
            pid,
            &body.git_ref,
            body.remote_branch.as_deref(),
            body.force,
            &GitActor::User(user.id),
        )
        .await?;

    Ok(Json(PushResponse {
        remote_branch: outcome.remote_branch,
        commit: outcome.commit,
    }))
}

// ---- revert ----

/// `{ branch, to, expected_head, reopen?: { state, comment } }` (`SPEC.md`,
/// "Git").
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RevertBody {
    /// The integration head to revert.
    branch: String,
    /// The full id of a strictly older commit on its first-parent line.
    to: String,
    /// The full id of the head the user confirmed against.
    expected_head: String,
    /// Reopen the terminal tasks of the reverted range.
    reopen: Option<ReopenBody>,
}

/// `{ state, comment }`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReopenBody {
    state: String,
    comment: String,
}

/// `{ commit, reverted, reopened }` (`SPEC.md`, "Git").
#[derive(Debug, Serialize)]
struct RevertResponse {
    commit: String,
    reverted: Vec<HistoryEntry>,
    reopened: Vec<TaskDto>,
}

/// `POST /projects/{pid}/git/revert` → the revert commit, the first-parent
/// range it took back and the tasks it reopened.
///
/// 400 for a `branch` that is not an integration head, a `to` that is not a
/// strictly older commit of its first-parent line, a malformed id, an unknown
/// or terminal reopen state and an empty comment; 409 `branch has moved`
/// when `expected_head` is not the head any more.
async fn revert(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(pid): Path<Uuid>,
    Json(body): Json<RevertBody>,
) -> Result<Json<RevertResponse>> {
    debug!(project_id = %pid, op = "revert", "git request");

    let request = RevertRequest {
        branch: body.branch,
        to: body.to,
        expected_head: body.expected_head,
        reopen: body.reopen.map(|reopen| Reopen {
            state: reopen.state,
            comment: reopen.comment,
        }),
    };
    let result = revert_and_reopen(&state, pid, user.id, request).await?;

    Ok(Json(RevertResponse {
        commit: result.outcome.commit,
        reverted: result.outcome.reverted,
        reopened: result.reopened,
    }))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use super::*;

    /// A body's shape rules are the one thing this module decides on its own,
    /// and they need no database to assert.
    #[test]
    fn a_merge_takes_exactly_one_of_the_two_documented_forms() {
        let task_id = Uuid::from_u128(1);
        let handoff_id = Uuid::from_u128(2);

        let branch: MergeBody =
            serde_json::from_value(json!({ "target": "main", "source": "origin/main" }))
                .expect("the branch form parses");
        assert!(matches!(
            merge_selection(&branch).expect("the branch form is accepted"),
            MergeSelection::Branch(source) if source == "origin/main"
        ));

        let task: MergeBody = serde_json::from_value(
            json!({ "target": "main", "task_id": task_id, "handoff_id": handoff_id }),
        )
        .expect("the task form parses");
        assert!(matches!(
            merge_selection(&task).expect("the task form is accepted"),
            MergeSelection::Handoff { task_id: t, handoff_id: h } if t == task_id && h == handoff_id
        ));

        let refused = [
            json!({ "target": "main" }),
            json!({ "target": "main", "source": "feature", "task_id": task_id }),
            json!({ "target": "main", "task_id": task_id }),
            json!({ "target": "main", "handoff_id": handoff_id }),
            json!({
                "target": "main",
                "source": "feature",
                "task_id": task_id,
                "handoff_id": handoff_id,
            }),
        ];

        for body in refused {
            let body: MergeBody = serde_json::from_value(body).expect("the body parses");
            let error = merge_selection(&body).expect_err("the mixture is refused");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            assert_eq!(error.to_string(), MERGE_FORM);
        }
    }

    /// The same rule for the diff's two selectors, which arrive as a query
    /// string rather than a body.
    #[test]
    fn a_diff_takes_exactly_one_selector() {
        let head = DiffQuery {
            head: Some("main".to_string()),
            handoff_id: None,
            base: None,
        };
        assert_eq!(
            head.selector().expect("a head alone is accepted"),
            DiffSelector::Head("main".to_string())
        );

        let handoff_id = Uuid::from_u128(3);
        let handoff = DiffQuery {
            head: None,
            handoff_id: Some(handoff_id),
            base: Some("main".to_string()),
        };
        assert_eq!(
            handoff.selector().expect("a hand-off alone is accepted"),
            DiffSelector::Handoff(handoff_id)
        );

        for query in [
            DiffQuery {
                head: None,
                handoff_id: None,
                base: None,
            },
            DiffQuery {
                head: Some("main".to_string()),
                handoff_id: Some(handoff_id),
                base: None,
            },
        ] {
            let error = query.selector().expect_err("the selection is refused");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            assert_eq!(error.to_string(), DIFF_FORM);
        }
    }

    /// The history page size: the default, the ceiling and the one refusal.
    #[test]
    fn a_history_page_is_fifty_by_default_and_at_most_two_hundred() {
        assert_eq!(history_limit(None).expect("no limit"), 50);
        assert_eq!(history_limit(Some(1)).expect("one"), 1);
        assert_eq!(history_limit(Some(200)).expect("the ceiling"), 200);
        assert_eq!(history_limit(Some(10_000)).expect("reduced"), 200);

        let error = history_limit(Some(0)).expect_err("zero is refused");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    }

    /// `force` is the one field whose default is part of the contract.
    #[test]
    fn force_defaults_to_false() {
        let body: PushBody =
            serde_json::from_value(json!({ "ref": "main" })).expect("the minimal body parses");

        assert_eq!(body.git_ref, "main");
        assert_eq!(body.remote_branch, None);
        assert!(!body.force);
    }

    /// The message limit, at the boundary and one byte past it.
    #[test]
    fn a_message_is_accepted_up_to_ten_kibibytes() {
        let at_limit = "m".repeat(MAX_MESSAGE_BYTES);
        assert_eq!(
            checked_message(Some(&at_limit)).expect("the limit itself is accepted"),
            Some(at_limit.as_str())
        );

        let over = "m".repeat(MAX_MESSAGE_BYTES + 1);
        let error = checked_message(Some(&over)).expect_err("one byte more is refused");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);

        assert_eq!(checked_message(None).expect("no message is fine"), None);
    }
}
