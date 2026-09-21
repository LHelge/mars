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
//! **Launching** is [`crate::session::create_session`]'s, all of it: the
//! project, profile, prompt, title and base checks, the task claim that commits
//! with the session row and the first message. `POST` here parses the body,
//! calls it with [`LaunchActor::User`] and answers 201, so that the dispatcher
//! and the scheduled agents planned in `ARCHITECTURE.md`, "Unattended
//! launches", launch through the same path without an HTTP request
//! and without a user. What that path decides, and in which order, is
//! `session::create`'s documentation.
//!
//! **The resource half** is a row read, a row write or one launch handed to
//! [`crate::session::create_session`].
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
//! **The token hash never leaves.** The response type is the
//! [`Session`] model, whose `mcp_token_hash` is `#[serde(skip)]`, so there is
//! no DTO here to forget to strip it from (`CLAUDE.md`, rule 3).

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::events::{MAX_TEXT_BYTES, SessionEvent, SessionInput, validate_client_id};
use crate::models::{Session, SessionState, SessionTitle, SyncOutcome, TaskRef};
use crate::prelude::*;
use crate::repositories::{MAX_EVENT_PAGE, ProjectRepository, SessionRepository, TaskRepository};
use crate::routes::tasks::task_ref;
use crate::routes::{CurrentUser, Path, Query};
use crate::session::{LaunchActor, LaunchRequest, SessionService, create_session};
use crate::tracker::TaskDto;

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
    /// The launch this body asks for, or 404 for a `task_id` that names no
    /// task at all.
    ///
    /// The one place the DTO becomes the domain request: every rule about what
    /// the fields mean is [`create_session`]'s, and a user launch never
    /// enforces the profile's served states (`ARCHITECTURE.md`, "Task tracker"
    /// → "Launching a session for a task").
    fn into_request(self) -> Result<LaunchRequest> {
        let task = self
            .task_id
            .as_ref()
            .map(TaskIdInput::reference)
            .transpose()?;

        Ok(LaunchRequest {
            profile_id: self.profile_id,
            task,
            base_ref: self.base_ref,
            title: self.title,
            message: self.message,
            require_served_state: false,
        })
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
/// Everything this endpoint decides beyond the shape of the body is
/// [`create_session`]'s, which is what the dispatcher and the scheduler will
/// call too; what is HTTP's is the parse, the launching user and the 201.
async fn create(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(pid): Path<Uuid>,
    Json(body): Json<CreateSessionInput>,
) -> Result<(StatusCode, Json<Session>)> {
    let session = create_session(
        &state,
        pid,
        body.into_request()?,
        LaunchActor::User { user_id: user.id },
    )
    .await?;

    Ok((StatusCode::CREATED, Json(session)))
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

        let minimal = minimal
            .into_request()
            .expect("the minimal body is a launch");
        assert_eq!(minimal.profile_id, profile_id);
        assert_eq!(minimal.base_ref, None);
        assert_eq!(minimal.title, None);
        assert_eq!(minimal.message, None);
        assert!(minimal.task.is_none());

        let full: CreateSessionInput = serde_json::from_value(json!({
            "profile_id": profile_id,
            "base_ref": "origin/main",
            "title": "Fix the login form",
            "message": "Fix login\nand the rest",
            "task_id": 12,
        }))
        .expect("the full body parses");

        let full = full.into_request().expect("the full body is a launch");
        assert_eq!(full.base_ref.as_deref(), Some("origin/main"));
        assert_eq!(full.title.as_deref(), Some("Fix the login form"));
        assert_eq!(full.message.as_deref(), Some("Fix login\nand the rest"));
        assert_eq!(full.task, Some(TaskRef::Number(12)));

        // A user launch never asks for the profile's served states
        // (`ARCHITECTURE.md`, "Task tracker" → "Launching a session for a
        // task").
        assert!(!full.require_served_state);

        // A reference that names no task at all is the documented 404, before
        // anything is launched.
        let bad: CreateSessionInput = serde_json::from_value(json!({
            "profile_id": profile_id,
            "task_id": "not-a-task",
        }))
        .expect("the body parses");
        assert_eq!(
            bad.into_request()
                .expect_err("a malformed reference names no task")
                .status(),
            StatusCode::NOT_FOUND,
        );

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
