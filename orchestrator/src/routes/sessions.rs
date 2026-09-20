//! `/api/projects/{pid}/sessions` and `/api/sessions` (`SPEC.md`, "Sessions").
//!
//! The two lists, the create, the read, the retitle, the event page, the task
//! list, the five action verbs and the delete: every row of the table.
//!
//! `GET /sessions/{id}/tasks` is the tracker's content under a session's path,
//! and it is here rather than in [`crate::routes::tasks`] because the path is
//! a session's: one implementation, which launch-for-task reuses rather than
//! repeats.
//!
//! **Launching for a task** ([`insert_claiming`]) is the one create that
//! writes outside `sessions`: with a `task_id` the row insert and the claim are
//! one project-locked transaction, so a session that exists holds its task and
//! a claim that loses leaves no session behind (`ARCHITECTURE.md`, "Task
//! tracker" → "Launching a session for a task"). The base a hand-off pins, the
//! generated first message and the 409 for a task that is held, blocked or
//! finished all follow from the row read under that lock.
//!
//! **The resource half** is a row read, a row write or one launch handed to
//! [`crate::session::Launcher`].
//!
//! **The action half** — `input`, `stop`, `end`, `retry`, `sync` and `DELETE` —
//! is [`SessionService`]: every lifecycle rule, every state check and every
//! refusal is that module's, so the same action asked for over the WebSocket
//! behaves identically (`ARCHITECTURE.md`, "Session lifecycle", "Stop
//! semantics"). A handler here decides only what HTTP owns: the shape of the
//! body, the size of the text in it and which status a success answers with.
//!
//! The two 202s are ADR 0020's: `input` and `stop` are accepted by the
//! orchestrator, not delivered to the CLI, and they carry no body at all —
//! a client learns the new state from the WebSocket `session` message or from a
//! `GET` (`SPEC.md`, "REST API"; "WebSocket: session stream", the restart
//! limitation). `end` and `retry` answer the `Session` they produced, and
//! `sync` the `{ref, commit}` of the ref it published.
//!
//! **What `POST` decides**, in this order, because the cheap refusals come
//! first and nothing irreversible happens before the last of them
//! (`SPEC.md`, "Sessions", `POST /projects/{pid}/sessions`;
//! `ARCHITECTURE.md`, "Launch sequence"):
//!
//! 1. the project exists (404) and is `ready` (409) — a project without a
//!    `default_branch` is answered with the same 409, because a session has no
//!    base to start from and a `ready` project always has one;
//! 2. the profile is one of *this* project's (400), which is also what makes
//!    `kind` the profile's kind at launch (`docs/data-model.md`, `sessions`);
//! 3. an ephemeral profile was given a prompt (400), since an ephemeral
//!    session accepts no input after launch (ADR 0003);
//! 4. an explicitly given `title` is 1 to 200 characters (400), and an omitted
//!    one is derived by [`default_title`];
//! 5. the base ref resolves in the project repository (400), checked under the
//!    project git lock and before any transaction, because a git lock is never
//!    taken from inside a database one (ADR 0021). The launcher resolves it
//!    again when it clones — the mirror may have been fetched in between — so
//!    this check exists only to answer 400 now instead of producing a session
//!    that fails a moment later. A base a task's hand-off pins is not checked
//!    here: it is chosen under the tracker lock this request has not taken yet,
//!    and it is a commit the project retains;
//! 6. a fresh MCP token is generated **before** the insert, so the row's hash
//!    and the only copy of the value come into existence together (ADR 0029;
//!    `ARCHITECTURE.md`, "MCP design"): the raw token goes to the launcher,
//!    which writes it into the session's `mcp.json`, and never back to the UI.
//!
//! The row is then inserted and committed, the response is 201 with the
//! `creating` session, and the launch runs on its own task.
//!
//! **How the first message is delivered.** It is not a column: `message` is the
//! first thing said to this session — after the generated task message, when
//! there is a task — and where the two go depends on the kind. An ephemeral
//! session has no stdin to say them on, so they become the `-p` prompt the
//! launcher passes, joined by a blank line ([`LaunchMode::Fresh`]). A
//! conversational session reads its messages from stdin, so each is submitted
//! to the registry as a [`QueuedInput`] — the generated one with no `user_id`,
//! since nobody typed it, the caller's with theirs — and the owner records them
//! as the session's first `user_message`s exactly as it records any other input
//! (`ARCHITECTURE.md`, "Session owner task").
//!
//! That submission has to be race-free against the launch, which is why the
//! *route* registers the session entry: [`SessionRegistry::submit`] rejects an
//! id it has never seen, and the launch task only registers once it starts. So
//! the order here is: commit the row, `register(.., Phase::Creating)`, submit
//! the input, launch. The launch task's own `register` keeps the queue of an id
//! it already knows — that is what makes a message to a parked session survive
//! the resume it triggers — so the queued input is picked up by the owner when
//! stdin attaches, whichever of the two `register` calls ran first. Nothing in
//! the launcher or the registry had to change for that.
//!
//! **The token hash never leaves.** The response type is the
//! [`Session`] model, whose `mcp_token_hash` is `#[serde(skip)]`, so there is
//! no DTO here to forget to strip it from, and the raw token exists only as the
//! value moved into [`LaunchMode::Fresh`] (`CLAUDE.md`, rule 3).

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::events::{MAX_TEXT_BYTES, SessionEvent, SessionInput, TaskActor, validate_client_id};
use crate::git::{DataPaths, resolve_base};
use crate::models::{
    AgentProfile, NewSession, ProjectStatus, Session, SessionKind, SessionState, SessionTitle,
    SyncOutcome, TaskRef, default_title, validate_launch_prompt, validate_title,
};
use crate::prelude::*;
use crate::repositories::{MAX_EVENT_PAGE, ProjectRepository, SessionRepository, TaskRepository};
use crate::routes::tasks::task_ref;
use crate::routes::{CurrentUser, Path, Query};
use crate::session::{
    HandoffContext, LaunchMode, McpToken, Phase, QueuedInput, SessionRegistry, SessionService,
    SubmitResult, generated_task_message,
};
use crate::tracker::{TaskDto, TrackerMutation, claim_for_launch};

/// What a create against a project that has no repository to clone is told
/// (409).
///
/// The same words `routes::projects` answers a fetch or a branch listing with,
/// because it is the same condition: the project is not `ready`.
const NOT_READY: &str = "project is not ready";

/// What a create naming a profile of another project — or of no project — is
/// told (400).
///
/// One message for "no such profile" and "not this project's profile" alike:
/// the lookup is scoped in the `WHERE` clause (`CLAUDE.md`, "Backend
/// conventions"), so which of the two it was is not something a caller learns.
const UNKNOWN_PROFILE: &str = "unknown profile";

/// What a create whose base ref is not in the mirror is told (400).
const UNRESOLVED_BASE: &str = "base_ref does not resolve";

/// What a `?state=` outside the five lifecycle values is told (400).
const UNKNOWN_STATE: &str = "unknown state";

/// What an event page outside `1..=`[`MAX_EVENT_PAGE`] is told (400).
const BAD_LIMIT: &str = "limit must be between 1 and 500";

/// What an event page whose cursor cannot name a sequence is told (400).
const BAD_BEFORE: &str = "before must be 1 or greater";

/// How many events one page carries when the caller names no `limit`
/// (`SPEC.md`, "Sessions").
const DEFAULT_EVENT_PAGE: u32 = 100;

/// How much of an `input` request axum buffers before it refuses to read more.
///
/// Above [`MAX_TEXT_BYTES`] by the room a JSON envelope and its escaping need,
/// so a text just over the cap reaches the handler and is answered with the
/// documented 400 [`SessionInput::validate`] gives rather than with a body-limit
/// rejection; far above it nothing is buffered at all, which is the point of
/// having a limit.
const MAX_INPUT_BODY: usize = 2 * MAX_TEXT_BYTES;

/// The router nested under `/api/sessions`.
///
/// The project-scoped half is [`project_routes`]. `GET /{id}/tasks` is the task
/// tracker's and is added with it.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list))
        .route("/{id}", get(fetch).put(update).delete(remove))
        .route("/{id}/events", get(events))
        .route("/{id}/tasks", get(tasks))
        .route(
            "/{id}/input",
            post(input).layer(DefaultBodyLimit::max(MAX_INPUT_BODY)),
        )
        .route("/{id}/stop", post(stop))
        .route("/{id}/end", post(end))
        .route("/{id}/retry", post(retry))
        .route("/{id}/sync", post(sync))
}

/// The router merged onto `/api/projects`.
///
/// The `{pid}` capture is part of these paths rather than of the `nest`, for
/// the reason `routes::git` gives: axum takes one `nest` per prefix, so every
/// project-scoped router carries its own `{pid}/…` (`routes::mod`).
pub fn project_routes() -> Router<AppState> {
    Router::new().route("/{pid}/sessions", get(list_in_project).post(create))
}

// ---- lists ----

/// `?state=` on both list endpoints (`SPEC.md`, "Sessions").
///
/// A `String` rather than a [`SessionState`], because an unrecognised value is
/// the documented 400 `unknown state` and a typed field would instead answer
/// serde's own message about a variant name.
#[derive(Debug, Deserialize)]
struct StateQuery {
    state: Option<String>,
}

impl StateQuery {
    /// The state to filter by, `None` for "every state", or 400.
    fn resolve(&self) -> Result<Option<SessionState>> {
        let Some(raw) = self.state.as_deref() else {
            return Ok(None);
        };

        let state = [
            SessionState::Creating,
            SessionState::Running,
            SessionState::Parked,
            SessionState::Done,
            SessionState::Failed,
        ]
        .into_iter()
        .find(|state| state.as_str() == raw)
        .ok_or_else(|| Error::BadRequest(UNKNOWN_STATE.to_string()))?;

        Ok(Some(state))
    }
}

/// `GET /projects/{pid}/sessions?state=` → this project's sessions, newest
/// first (404 unknown project, 400 unknown state).
///
/// The project is read first so that an unknown one is a 404 rather than an
/// empty list, which is the difference between "no sessions" and "no project".
async fn list_in_project(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(pid): Path<Uuid>,
    Query(query): Query<StateQuery>,
) -> Result<Json<Vec<Session>>> {
    let filter = query.resolve()?;

    ProjectRepository::new(&state.pool)
        .find(pid)
        .await?
        .ok_or(Error::NotFound)?;

    let sessions = SessionRepository::new(&state.pool)
        .list_by_project(pid, filter)
        .await?;

    Ok(Json(sessions))
}

/// `GET /sessions?state=` → every session of every project, newest first (400
/// unknown state).
///
/// The dashboard's list: sessions are visible to every authenticated user, so
/// there is nothing to scope it by (`SPEC.md`, "Sessions").
async fn list(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Query(query): Query<StateQuery>,
) -> Result<Json<Vec<Session>>> {
    let filter = query.resolve()?;

    let sessions = SessionRepository::new(&state.pool).list_all(filter).await?;

    Ok(Json(sessions))
}

// ---- create ----

/// `POST /projects/{pid}/sessions` (`{ profile_id, base_ref?, title?,
/// message? }`).
///
/// `deny_unknown_fields` for the reason the other route modules give: a client
/// that sends `state`, `branch` or `kind` here has misunderstood the endpoint —
/// they are the server's — and is told so rather than silently ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSessionInput {
    profile_id: Uuid,
    /// Absent means the project's `default_branch` (`SPEC.md`, "Projects":
    /// "the default session base is the integration head named by
    /// `default_branch`"), or — with a `task_id` whose task has a current
    /// hand-off — that hand-off's pinned commit.
    base_ref: Option<String>,
    title: Option<String>,
    /// The first thing said to this session. Not a column: it is delivered as
    /// the launch prompt or as the first queued input, never stored on the row.
    message: Option<String>,
    /// The task this session is launched to work on, claimed in the same
    /// transaction that inserts the row (`SPEC.md`, "Sessions").
    task_id: Option<TaskIdInput>,
}

impl CreateSessionInput {
    /// The message, or `None` when there is nothing to say.
    ///
    /// A message of whitespace is no message: it would derive no title, and
    /// [`validate_launch_prompt`] does not count it as an ephemeral session's
    /// prompt either, so it is not sent as input either.
    fn message(&self) -> Option<&str> {
        self.message
            .as_deref()
            .filter(|message| !message.trim().is_empty())
    }

    /// The task this create names, in the form the tracker resolves, or 404.
    fn task(&self) -> Result<Option<TaskRef>> {
        self.task_id
            .as_ref()
            .map(TaskIdInput::reference)
            .transpose()
    }
}

/// A `task_id` in a create body: a task's UUID, or its per-project number as a
/// string or as a JSON integer (`SPEC.md`, "Sessions").
///
/// The string forms are the ones every other endpoint takes, parsed by the one
/// shared [`task_ref`]. The integer is accepted too because `#12` is a number
/// on the board and a client that sends it as one has not made a mistake; a
/// number outside `INTEGER` addresses no task and is the same 404 a malformed
/// string gets.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TaskIdInput {
    /// A UUID, or a number written as a string.
    Text(String),
    /// A per-project number sent as a JSON integer.
    Number(i64),
}

impl TaskIdInput {
    /// The reference this names, or 404.
    fn reference(&self) -> Result<TaskRef> {
        match self {
            Self::Text(raw) => task_ref(raw),
            Self::Number(number) => i32::try_from(*number)
                .map(TaskRef::Number)
                .map_err(|_| Error::NotFound),
        }
    }
}

/// `POST /projects/{pid}/sessions` → the `creating` session (201; 400, 404 and
/// 409 as the module documentation lists them).
///
/// The launch is spawned after the transaction committed, for the reason every
/// other job in this crate is: the launch task re-reads the row it is about to
/// work on (`ARCHITECTURE.md`, "Launch sequence").
async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(pid): Path<Uuid>,
    Json(body): Json<CreateSessionInput>,
) -> Result<(StatusCode, Json<Session>)> {
    let projects = ProjectRepository::new(&state.pool);
    let project = projects.find(pid).await?.ok_or(Error::NotFound)?;

    // A project with no repository has no base to clone, and one with no
    // `default_branch` has no base to default to. The second cannot happen on a
    // `ready` project — the clone job will not promote one — and is guarded
    // rather than unwrapped.
    if project.status != ProjectStatus::Ready {
        return Err(Error::Conflict(NOT_READY.to_string()));
    }
    let Some(default_branch) = project.default_branch.as_deref() else {
        error!(project_id = %pid, "a ready project has no default branch");
        return Err(Error::Conflict(NOT_READY.to_string()));
    };

    let profile = projects
        .find_profile(pid, body.profile_id)
        .await?
        .ok_or_else(|| Error::BadRequest(UNKNOWN_PROFILE.to_string()))?;

    let message = body.message();

    // Read on the pool, for the 404 and for the title: the row the claim is
    // decided against is the one the mutation locks a few lines down.
    let task = match body.task()? {
        Some(reference) => Some(
            TaskRepository::new(&state.pool)
                .find_task(pid, reference)
                .await?
                .ok_or(Error::NotFound)?,
        ),
        None => None,
    };

    // A task is a prompt: its generated message is what an ephemeral session
    // runs, so one launched for a task needs no `message` (`SPEC.md`).
    validate_launch_prompt(profile.kind, task.as_ref().map(|task| task.id), message)?;

    let title = match body.title.as_deref() {
        Some(raw) => Some(validate_title(raw)?),
        None => default_title(task.as_ref().map(|task| task.title.as_str()), message),
    };

    // A base the caller named is resolved now, under the project git lock and
    // before any database transaction, so that the documented lock order holds
    // and a name that is not in the mirror is a 400 rather than a session that
    // fails a moment later (ADR 0021). The launcher resolves it again when it
    // clones. A hand-off's base is skipped here: it is a commit this project
    // pinned, chosen under a lock this request has not taken yet.
    let explicit_base = body.base_ref.as_deref();
    let starts_from_handoff = explicit_base.is_none()
        && task
            .as_ref()
            .is_some_and(|task| task.current_handoff_id.is_some());
    if !starts_from_handoff {
        let base_ref = explicit_base.unwrap_or(default_branch);
        let guard = state.git_locks.lock(pid).await;
        resolve_base(
            &guard,
            &DataPaths::from_config(&state.config),
            Some(base_ref),
            default_branch,
        )
        .await
        .map_err(|err| {
            debug!(
                project_id = %pid,
                git.base = %base_ref,
                error = %err,
                "a requested session base ref does not resolve",
            );
            Error::BadRequest(UNRESOLVED_BASE.to_string())
        })?;
    }

    // Before the insert, so the hash the row stores and the only copy of the
    // value are created together (ADR 0029). The row records the name the
    // caller gave, not the commit it resolved to.
    let token = McpToken::generate();

    let (session, task_message) = match task {
        Some(task) => {
            insert_claiming(
                &state,
                &profile,
                user.id,
                task.id,
                LaunchBase {
                    explicit: explicit_base,
                    default_branch,
                },
                title.as_deref(),
                &token,
            )
            .await?
        }
        None => {
            let mut new_session = NewSession::new(
                pid,
                profile.id,
                profile.kind,
                explicit_base.unwrap_or(default_branch),
                token.hash(),
            );
            new_session.created_by = Some(user.id);
            let new_session = new_session.with_title(title.as_deref())?;

            let mut tx = state.pool.begin().await?;
            let session = SessionRepository::new(&state.pool)
                .insert(&mut tx, &new_session)
                .await?;
            tx.commit().await?;

            (session, None)
        }
    };

    info!(
        session_id = %session.id,
        project_id = %pid,
        profile_id = %profile.id,
        kind = ?profile.kind,
        task_id = ?session.task_id,
        "session created",
    );

    let prompt = first_message(&state, &session, user.id, task_message.as_deref(), message);
    state
        .launcher()
        .launch(session.id, LaunchMode::Fresh { token, prompt });

    Ok((StatusCode::CREATED, Json(session)))
}

/// What a launch starts from before the task's hand-off has been read.
///
/// The caller's `base_ref` and the project's integration head: which of them
/// the session ends up with — and whether a hand-off commit displaces both —
/// is decided under the lock in [`insert_claiming`].
#[derive(Debug, Clone, Copy)]
struct LaunchBase<'a> {
    /// The `base_ref` the caller named, already resolved in the mirror.
    explicit: Option<&'a str>,
    /// The project's `default_branch`.
    default_branch: &'a str,
}

/// Insert the session row and claim its task in one tracker-locked
/// transaction, and answer with the row and the generated task message.
///
/// The whole of `ARCHITECTURE.md`, "Task tracker" → "Launching a session for a
/// task" that is not the launch itself, in the documented lock order: the
/// project row (the mutation), then the task row, then the session insert
/// (ADR 0021). Nothing here resolves a ref or calls the engine — a git lock is
/// never taken from inside a database one.
///
/// The order within the transaction is forced by the schema:
/// `tasks.lease_holder_session_id` references `sessions`, so the row exists
/// before the claim points at it. Both are the same transaction, so a lost
/// claim takes the session row with it: [`claim_for_launch`] answers 409 `task
/// is not claimable` for a held, blocked or terminal task — the statement's
/// `WHERE` decides all three at once — and `?` drops the mutation, which rolls
/// back.
///
/// The base and the hand-off are selected from the *locked* row, so a session
/// cannot start from a hand-off that was superseded between the read and the
/// claim. An explicit `base_ref` overrides the hand-off commit and leaves
/// `handoff_id` null, and the generated message says so.
async fn insert_claiming(
    state: &AppState,
    profile: &AgentProfile,
    user_id: Uuid,
    task_id: Uuid,
    base: LaunchBase<'_>,
    title: Option<&str>,
    token: &McpToken,
) -> Result<(Session, Option<String>)> {
    let project_id = profile.project_id;
    let tasks = TaskRepository::new(&state.pool);

    let mut mutation =
        TrackerMutation::begin(&state.pool, project_id, TaskActor::User { user_id }).await?;

    let task = tasks
        .find_task_for_update(mutation.conn(), project_id, TaskRef::Id(task_id))
        .await?
        .ok_or(Error::NotFound)?;

    let handoff = match task.current_handoff_id {
        Some(id) => {
            tasks
                .find_handoff_in(mutation.conn(), project_id, id)
                .await?
        }
        None => None,
    };
    let comment = match handoff.as_ref().and_then(|handoff| handoff.comment_id) {
        Some(id) => {
            tasks
                .find_comment_in(mutation.conn(), project_id, id)
                .await?
        }
        None => None,
    };

    // The pinned commit is the default base for a task that has one; an
    // explicit base wins and records no hand-off, because the session is not
    // starting from that revision (`SPEC.md`, "Sessions").
    let (base_ref, handoff_id, base_override) = match (base.explicit, handoff.as_ref()) {
        (Some(explicit), handoff) => (
            explicit.to_string(),
            None,
            handoff.is_some().then_some(explicit),
        ),
        (None, Some(handoff)) => (handoff.commit.clone(), Some(handoff.id), None),
        (None, None) => (base.default_branch.to_string(), None, None),
    };

    let task_message = generated_task_message(
        &task,
        handoff.as_ref().map(|handoff| HandoffContext {
            handoff,
            comment: comment.as_ref().map(|comment| comment.body.as_str()),
        }),
        base_override,
    );

    let mut new_session =
        NewSession::new(project_id, profile.id, profile.kind, base_ref, token.hash());
    new_session.created_by = Some(user_id);
    new_session.task_id = Some(task.id);
    new_session.handoff_id = handoff_id;
    let new_session = new_session.with_title(title)?;

    let session = SessionRepository::new(&state.pool)
        .insert(&mut mutation.conn(), &new_session)
        .await?;

    // The claim, its `claimed` event, its `task_sessions` link and the
    // notification the commit issues are all the tracker's.
    claim_for_launch(&mut mutation, &task, session.id).await?;
    mutation.commit().await?;

    Ok((session, Some(task_message)))
}

/// Deliver what this session is told first, and answer with the ephemeral
/// launch prompt.
///
/// Up to two texts, in the one documented order: the generated task message,
/// when the session was launched for a task, and then the caller's `message`
/// (`SPEC.md`, "Sessions").
///
/// The two kinds differ in *where* they go, not in whether they are delivered:
/// an ephemeral session gets them joined by a blank line as the `-p` prompt the
/// returned value carries, a conversational one gets them as inputs queued for
/// the owner that is about to attach. See the module documentation for why the
/// registry entry is created here.
///
/// The generated message is queued with no `user_id`: nobody typed it, so the
/// `user_message` the owner records for it carries `user_id: null`.
///
/// A refused submission is logged and not returned: the session exists and is
/// launching, and the caller can send the message again over the socket. No
/// text is ever logged (rule 3).
fn first_message(
    state: &AppState,
    session: &Session,
    user_id: Uuid,
    task_message: Option<&str>,
    message: Option<&str>,
) -> Option<String> {
    if session.kind == SessionKind::Ephemeral {
        return match (task_message, message) {
            (Some(task_message), Some(message)) => Some(format!("{task_message}\n\n{message}")),
            (Some(text), None) | (None, Some(text)) => Some(text.to_string()),
            (None, None) => None,
        };
    }

    if task_message.is_none() && message.is_none() {
        return None;
    }

    // The receiver is dropped: the launch task's own `register` installs the
    // channel the owner reads, and keeps the queue these submissions fill.
    let _owner = state
        .session_registry
        .register(session.id, session.kind, Phase::Creating);

    if let Some(task_message) = task_message {
        queue_first(state, session, None, task_message);
    }
    if let Some(message) = message {
        queue_first(state, session, Some(user_id), message);
    }

    None
}

/// Queue one of a new session's first inputs, or log why it was refused.
fn queue_first(state: &AppState, session: &Session, user_id: Option<Uuid>, text: &str) {
    let registry: &SessionRegistry = &state.session_registry;

    match registry.submit(
        session.id,
        QueuedInput {
            input: SessionInput::Message {
                text: text.to_string(),
            },
            user_id,
            client_id: None,
            accepted_at: Utc::now(),
        },
    ) {
        SubmitResult::Rejected(reason) => warn!(
            session_id = %session.id,
            reason = %reason,
            "a first message of a new session was not queued",
        ),
        result => debug!(
            session_id = %session.id,
            result = ?result,
            "a first message of a new session was queued",
        ),
    }
}

// ---- read and retitle ----

/// `GET /sessions/{id}` → the session, or 404.
async fn fetch(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Session>> {
    let session = SessionRepository::new(&state.pool).get(id).await?;

    Ok(Json(session))
}

/// `GET /sessions/{id}/tasks` → the tasks this session touched, most recently
/// touched first (404 unknown session).
///
/// "Touched" is the `task_sessions` link (`docs/data-model.md`): the tasks it
/// claimed, commented on, created or handed back, whether or not it still
/// holds any of them — the session view lists them beside the task it was
/// launched for (`SPEC.md`, "Frontend" → "Task board"). Full `Task` DTOs, so
/// the panel needs no second request per row.
///
/// The session is read first, which is both the 404 and where the project to
/// load the DTOs under comes from: a session belongs to one project and so
/// does every task it can have touched. A task that was deleted took its link
/// with it and is simply not listed.
async fn tasks(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<TaskDto>>> {
    let session = SessionRepository::new(&state.pool).get(id).await?;

    let tasks = TaskRepository::new(&state.pool);
    let rows = tasks.list_tasks_for_session(session.id).await?;

    Ok(Json(tasks.load_task_dtos(session.project_id, &rows).await?))
}

/// `PUT /sessions/{id}` (`{ title }`).
///
/// One required field: the endpoint exists to name a session, so an empty body
/// is a 400 from serde rather than a 200 that changed nothing. Clearing a
/// title back to null is not part of the documented contract.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateSessionInput {
    title: String,
}

/// `PUT /sessions/{id}` → the retitled session (400 an invalid title, 404
/// unknown).
async fn update(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateSessionInput>,
) -> Result<Json<Session>> {
    let title = SessionTitle::parse(&body.title)?;

    let mut tx = state.pool.begin().await?;
    let updated = SessionRepository::new(&state.pool)
        .update_title(&mut tx, id, Some(&title))
        .await?
        .ok_or(Error::NotFound)?;
    tx.commit().await?;

    debug!(session_id = %id, "session retitled");

    Ok(Json(updated))
}

// ---- events ----

/// `?before=<seq>&limit=<n≤500>` (`SPEC.md`, "Sessions").
#[derive(Debug, Deserialize)]
struct EventsQuery {
    before: Option<i64>,
    limit: Option<u32>,
}

impl EventsQuery {
    /// The cursor, or 400.
    ///
    /// Sequences start at 1, so `before=1` is the legal "nothing before the
    /// first event" and `before=0` — or a negative one — names no page at all
    /// and is a caller mistake rather than an empty answer.
    fn before(&self) -> Result<Option<i64>> {
        match self.before {
            Some(before) if before < 1 => Err(Error::BadRequest(BAD_BEFORE.to_string())),
            other => Ok(other),
        }
    }

    /// The page size, or 400.
    ///
    /// Checked rather than clamped: a caller asking for 501 events is told the
    /// limit instead of being handed 500 and left to believe it was all of
    /// them.
    fn limit(&self) -> Result<u32> {
        match self.limit {
            None => Ok(DEFAULT_EVENT_PAGE),
            Some(limit) if (1..=MAX_EVENT_PAGE).contains(&limit) => Ok(limit),
            Some(_) => Err(Error::BadRequest(BAD_LIMIT.to_string())),
        }
    }
}

/// `{ events, has_more }` (`SPEC.md`, "Sessions").
///
/// `events` are [`SessionEvent`]s, which is the shape with the internal
/// `_`-prefixed payload fields already dropped (`SPEC.md`, "AgentEvent"), so
/// `_offset` cannot reach a client through this endpoint.
#[derive(Debug, Serialize)]
struct EventPage {
    events: Vec<SessionEvent>,
    has_more: bool,
}

/// `GET /sessions/{id}/events` → one page, oldest last (400 a bad cursor or
/// limit, 404 unknown session).
///
/// The session is read first, so an unknown id is a 404 rather than an empty
/// page.
async fn events(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<Uuid>,
    Query(query): Query<EventsQuery>,
) -> Result<Json<EventPage>> {
    let before = query.before()?;
    let limit = query.limit()?;

    let sessions = SessionRepository::new(&state.pool);
    sessions.get(id).await?;

    let (events, has_more) = sessions.events_page(id, before, limit).await?;

    Ok(Json(EventPage { events, has_more }))
}

// ---- actions ----

/// The service for this request, built as every other caller builds it.
fn service(state: &AppState) -> SessionService {
    SessionService::new(state)
}

/// The body of `POST /sessions/{id}/input`: a `SessionInput` and, optionally,
/// the caller's own `client_id`.
///
/// Flattened rather than nested, so the body stays `{kind, text}` for every
/// caller that sends no id — the shape this endpoint had before the fallback
/// needed one (`SPEC.md`, "Sessions"). The id is the same echo key the socket
/// carries: a client that sends an input over this route because its socket is
/// closed still gets its `user_message` back with the id it reconciles its
/// optimistic message by.
#[derive(Debug, Deserialize)]
struct InputBody {
    #[serde(flatten)]
    input: SessionInput,
    client_id: Option<String>,
}

/// `POST /sessions/{id}/input` (`{kind, text, client_id?}`) → 202 with no body.
///
/// The acknowledgement is acceptance by the orchestrator and not delivery to
/// the CLI, which is why it is a 202 and why it carries nothing: a parked
/// session is being relaunched as this returns, and the state it ends up in
/// arrives over the socket (ADR 0020; `SPEC.md`, "Sessions"). A `client_id` is
/// not an idempotency key here any more than it is over the socket (ADR 0020);
/// it is only echoed back on the `user_message` this input produces.
///
/// An unknown `kind` and a malformed body are both 400 from the [`Json`]
/// extractor. The text is never logged (rule 3).
async fn input(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<InputBody>,
) -> Result<StatusCode> {
    body.input.validate()?;
    if let Some(client_id) = &body.client_id {
        validate_client_id(client_id)?;
    }

    service(&state)
        .send_input(id, body.input, Some(user.id), body.client_id)
        .await?;

    Ok(StatusCode::ACCEPTED)
}

/// `POST /sessions/{id}/stop` → 202 with no body (409 unless `running`, 404
/// unknown).
///
/// `SIGINT`, then `SIGTERM` after the grace period, then `parked` — all of it
/// after this response (`ARCHITECTURE.md`, "Stop semantics").
async fn stop(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    service(&state).stop(id).await?;

    Ok(StatusCode::ACCEPTED)
}

/// `POST /sessions/{id}/end` → the ended session (409 from `done` and
/// `failed`, 404 unknown).
///
/// Not a 202: unlike a stop, this one waits — for a launch it cancelled to let
/// go of the session or for the run to end, for the fetch-back and for the
/// container to go — and answers the row it produced.
/// That row is `done` for a conversational session; a run that ended badly while
/// the stop was in flight comes back `failed`, because the lifecycle has no
/// `failed → done` edge (`SPEC.md`, "Sessions"; [`SessionService::end`]).
async fn end(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Session>> {
    Ok(Json(service(&state).end(id).await?))
}

/// `POST /sessions/{id}/retry` (`{ message? }`).
///
/// `deny_unknown_fields` for the reason the other bodies in this module carry
/// it: a client that sends `state` or `text` here has misunderstood the
/// endpoint and is told so.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetryInput {
    message: Option<String>,
}

impl RetryInput {
    /// The optional body, from the bytes of the request.
    ///
    /// A retry needs nothing said to it, so no body at all — and an empty one,
    /// which is what a client that posts without a payload sends — is the
    /// documented "absent" case and parses as the default. Anything that is
    /// there is JSON or a 400, as on every other route.
    fn parse(body: &Bytes) -> Result<Self> {
        if body.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self::default());
        }

        serde_json::from_slice(body).map_err(|err| Error::BadRequest(err.to_string()))
    }

    /// The message to relaunch with, or `None` when there is nothing to say.
    ///
    /// Whitespace is no message, exactly as on `POST
    /// /projects/{pid}/sessions`: it would be refused by the backend's encoder
    /// a moment later, and a retry is worth doing without one.
    fn message(self) -> Option<String> {
        self.message.filter(|message| !message.trim().is_empty())
    }
}

/// `POST /sessions/{id}/retry` → the `parked` session (409 for an ephemeral
/// session and for one that is not `failed`, 404 unknown).
///
/// With a `message` the session is relaunched at once and may already be
/// `running` by the time a client reads this body; the row answered here is the
/// `parked` one the transition produced, which is the documented response
/// (`SPEC.md`, "Sessions").
async fn retry(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> Result<Json<Session>> {
    let message = RetryInput::parse(&body)?.message();
    if let Some(text) = &message {
        // The same cap the input route applies: the message goes to
        // `send_input` a moment later and is one for every purpose.
        SessionInput::Message { text: text.clone() }.validate()?;
    }

    let parked = service(&state).retry(id, message, Some(user.id)).await?;

    Ok(Json(parked))
}

/// `POST /sessions/{id}/sync` → `{ ref, commit }` (409 while `creating`, 404
/// unknown, 500 for an internal git failure).
///
/// The lock, the fetch and the `git { op: "sync" }` event — success or failure —
/// are [`crate::git::GitService::sync_session`]'s, which is where the error
/// mapping lives too (`ARCHITECTURE.md`, "Git model", Fetch-back).
async fn sync(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<SyncOutcome>> {
    Ok(Json(service(&state).sync(id).await?))
}

/// `DELETE /sessions/{id}` → 204 (409 unless `done` or `failed`, 404 unknown).
///
/// Named `remove` because `delete` is the routing method it is registered with,
/// as in the other modules. The session directory, the CLI transcript, the row
/// and everything that cascades with it are [`SessionService::delete`]'s
/// (`ARCHITECTURE.md`, "Storage").
async fn remove(
    State(state): State<AppState>,
    CurrentUser(_): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    service(&state).delete(id).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Every documented `state` value parses, and nothing else does.
    #[test]
    fn the_state_filter_takes_the_five_lifecycle_values() {
        for (raw, expected) in [
            ("creating", SessionState::Creating),
            ("running", SessionState::Running),
            ("parked", SessionState::Parked),
            ("done", SessionState::Done),
            ("failed", SessionState::Failed),
        ] {
            let query = StateQuery {
                state: Some(raw.to_string()),
            };

            assert_eq!(query.resolve().expect("{raw} is a state"), Some(expected));
        }

        assert_eq!(
            StateQuery { state: None }
                .resolve()
                .expect("no filter is legal"),
            None,
        );

        for raw in ["", "Running", "stopped", "creating "] {
            let error = StateQuery {
                state: Some(raw.to_string()),
            }
            .resolve()
            .expect_err("{raw} is not a state");

            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{raw}");
            assert_eq!(error.to_string(), UNKNOWN_STATE);
        }
    }

    /// The documented body, with and without its optional halves.
    #[test]
    fn a_create_body_takes_the_documented_shape() {
        let profile_id = Uuid::from_u128(7);

        let minimal: CreateSessionInput = serde_json::from_value(json!({
            "profile_id": profile_id,
        }))
        .expect("the minimal body parses");

        assert_eq!(minimal.profile_id, profile_id);
        assert_eq!(minimal.base_ref, None);
        assert_eq!(minimal.title, None);
        assert_eq!(minimal.message(), None);

        let full: CreateSessionInput = serde_json::from_value(json!({
            "profile_id": profile_id,
            "base_ref": "origin/main",
            "title": "Fix the login form",
            "message": "Fix login\nand the rest",
        }))
        .expect("the full body parses");

        assert_eq!(full.base_ref.as_deref(), Some("origin/main"));
        assert_eq!(full.title.as_deref(), Some("Fix the login form"));
        assert_eq!(full.message(), Some("Fix login\nand the rest"));

        // A message of whitespace is no message at all.
        let blank: CreateSessionInput = serde_json::from_value(json!({
            "profile_id": profile_id,
            "message": "   \n ",
        }))
        .expect("the body parses");
        assert_eq!(blank.message(), None);

        // The fields that are the server's.
        serde_json::from_value::<CreateSessionInput>(json!({
            "profile_id": profile_id,
            "state": "running",
        }))
        .expect_err("a server-owned field is refused");
    }

    /// `limit` defaults to 100 and is bounded by the documented 500; `before`
    /// has to be able to name a sequence.
    #[test]
    fn an_event_page_is_bounded() {
        let page = |before: Option<i64>, limit: Option<u32>| EventsQuery { before, limit };

        assert_eq!(page(None, None).limit().expect("the default"), 100);
        assert_eq!(page(None, Some(1)).limit().expect("one event"), 1);
        assert_eq!(page(None, Some(500)).limit().expect("the maximum"), 500);
        assert_eq!(page(None, None).before().expect("no cursor"), None);
        assert_eq!(page(Some(1), None).before().expect("the first"), Some(1));

        for limit in [0, 501, u32::MAX] {
            let error = page(None, Some(limit))
                .limit()
                .expect_err("the limit is refused");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{limit}");
            assert_eq!(error.to_string(), BAD_LIMIT);
        }

        for before in [0, -1, i64::MIN] {
            let error = page(Some(before), None)
                .before()
                .expect_err("the cursor is refused");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{before}");
            assert_eq!(error.to_string(), BAD_BEFORE);
        }
    }

    /// A retry body is optional in every form a client can leave it out in, and
    /// a message of whitespace is no message.
    #[test]
    fn a_retry_body_is_optional() {
        for raw in ["", "   ", "\n", "{}"] {
            let body = RetryInput::parse(&Bytes::from_static(raw.as_bytes()))
                .unwrap_or_else(|err| panic!("{raw:?} is an accepted retry body: {err}"));
            assert_eq!(body.message(), None, "{raw:?}");
        }

        let body = RetryInput::parse(&Bytes::from_static(br#"{"message":"try again"}"#))
            .expect("a message parses");
        assert_eq!(body.message().as_deref(), Some("try again"));

        let blank = RetryInput::parse(&Bytes::from_static(br#"{"message":"  "}"#))
            .expect("a blank message parses");
        assert_eq!(blank.message(), None);

        for raw in [r#"{"text":"try again"}"#, "not json", "[]"] {
            let error =
                RetryInput::parse(&Bytes::from(raw)).expect_err("{raw} is not a retry body");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{raw}");
        }
    }

    /// `PUT` takes exactly the one documented field.
    #[test]
    fn a_retitle_body_takes_exactly_a_title() {
        let body: UpdateSessionInput =
            serde_json::from_value(json!({ "title": "A better name" })).expect("the body parses");
        assert_eq!(body.title, "A better name");

        serde_json::from_value::<UpdateSessionInput>(json!({})).expect_err("a title is required");
        serde_json::from_value::<UpdateSessionInput>(json!({ "title": "x", "state": "done" }))
            .expect_err("a server-owned field is refused");
    }
}
