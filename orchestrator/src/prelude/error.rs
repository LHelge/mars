//! The crate-wide error type, its HTTP mapping and the `Result` alias.
//!
//! `ARCHITECTURE.md`, "Orchestrator internals" (Errors) and `SPEC.md`, "REST
//! API" define the contract implemented here: every failure answers
//! `{ "status": <u16>, "error": "<message>" }` with the same status code on the
//! response, git conflicts additionally carry `conflicts`, and internal
//! failures are logged once and answered with a generic message.

use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::email::EmailError;
use crate::engine::EngineError;
use crate::git::GitError;
use crate::models::{
    ProfileError, ProjectError, SecretError, SessionError, SharedDirError, TaskError, UserError,
};
use crate::prelude::*;
use crate::secrets::SecretsError;

/// The message every 5xx response carries; internal detail never leaves the log.
const INTERNAL_MESSAGE: &str = "internal error";

/// The crate-wide error type.
///
/// Handlers, repositories and services return [`Result`] and let `?` widen
/// their own errors into this enum. `Display` is always the client-visible
/// message for the 4xx variants; 5xx variants keep their detail for the log
/// line only.
///
/// Later epics add their own `#[from]` variants in place rather than wrapping
/// at the call site. Each of those types exposes `fn status(&self) ->
/// StatusCode` and a `Display` that is safe to show a client, so this enum only
/// has to delegate:
///
/// - `ClaimsError` → 401
/// - `UserError`, `ProjectError`, `SessionError`, `TaskError`, `SecretError`,
///   ... → 400 or 409 per the model's own `status()`
/// - `EngineError` → 500, 409 for state conflicts, and 400 for a container
///   specification the builder refused (`EngineError::InvalidSpec`)
/// - `GitError` → 500, 400 for a ref the caller named wrong, 409 for a
///   non-fast-forward push, a dirty work tree or a missing credential, and 422
///   when it carries conflicting paths ([`Error::GitConflict`])
/// - `SecretsError` → 500
/// - `EmailError` → 500
///
/// Axum's own extractor rejections convert into [`Error::BadRequest`], so the
/// [`Json`], [`Path`] and [`Query`] extractor wrappers re-exported from this
/// prelude answer a malformed request body, path segment or query string in
/// the documented shape instead of axum's plain text.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The resource does not exist, or is out of the caller's scope. 404.
    ///
    /// The generic answer, and the one to reach for: which of a route's
    /// lookups came up empty is usually not something a caller is entitled to
    /// learn. [`Error::Missing`] is the exception.
    #[error("not found")]
    NotFound,
    /// A 404 that names what was missing. The string is the client-visible
    /// message.
    ///
    /// For the one shape of 404 where the generic body is not an answer: a
    /// route that looks several things up in a row and whose caller has to act
    /// differently depending on which one was absent. `DELETE
    /// .../dependencies/{dep}` is the case that put it here — an unknown task
    /// and an edge that was never there are the same `not found` otherwise,
    /// and only the second one means "your request already holds" (`SPEC.md`,
    /// "Tasks"). The naming is the whole point, so a call site that has
    /// nothing to add uses [`Error::NotFound`] instead.
    #[error("{0}")]
    Missing(String),
    /// Authenticated, but not permitted. 403. The string is the client-visible
    /// message: the password-change gate answers `password change required`
    /// and an administrator-only route `admin required` (`SPEC.md`,
    /// "Authentication"), other call sites their own.
    #[error("{0}")]
    Forbidden(String),
    /// A state conflict: claim lost, duplicate name, dependency cycle. 409.
    #[error("{0}")]
    Conflict(String),
    /// Validation failure or malformed input. 400.
    #[error("{0}")]
    BadRequest(String),
    /// Missing or invalid credentials. 401. `authentication required` is the
    /// conventional message; the auth epic decides per call site.
    #[error("{0}")]
    Unauthorized(String),
    /// The caller is rate limited. 429. The string is the client-visible
    /// message: the login route says `too many login attempts` (`SPEC.md`,
    /// "Auth (`/api/auth`)"), other call sites their own.
    #[error("{0}")]
    Throttled(String),
    /// An unexpected internal failure. 500; the string is logged, never sent.
    #[error("{0}")]
    Internal(String),
    /// A git operation that failed on conflicting paths. 422, with `conflicts`.
    #[error("git conflict")]
    GitConflict {
        message: String,
        conflicts: Vec<String>,
    },
    /// An access token that did not verify; [`ClaimsError::status`] decides
    /// the code, and it is always 401 `authentication required`. Presenting no
    /// token at all is [`Error::Unauthorized`] instead.
    #[error(transparent)]
    Claims(#[from] ClaimsError),
    /// A container engine failure; [`EngineError::status`] decides the code.
    #[error(transparent)]
    Engine(#[from] EngineError),
    /// An outgoing mail failure; [`EmailError::status`] decides the code.
    #[error(transparent)]
    Email(#[from] EmailError),
    /// A git failure that is not a merge conflict; [`GitError::status`]
    /// decides the code. Conflicts are [`Error::GitConflict`], which carries
    /// the conflicting paths: the hand-written `From<GitError>` below is what
    /// routes them there, so a `?` on any git call produces the documented
    /// 422 body without the call site having to know.
    #[error(transparent)]
    Git(GitError),
    /// A secrets failure; [`SecretsError::status`] decides the code.
    #[error(transparent)]
    Secrets(#[from] SecretsError),
    /// Any database failure. 500, except `RowNotFound`, which is 404 so
    /// repositories can use `fetch_one` and let this conversion do the work.
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    /// A secret model rejected its input. The model decides the status. This
    /// is the caller's mistake; [`Error::Secrets`] is the keyring's.
    #[error(transparent)]
    Secret(#[from] SecretError),
    /// A session model rejected its input. The model decides the status: an
    /// illegal lifecycle transition is 409, everything else 400.
    #[error(transparent)]
    Session(#[from] SessionError),
    /// A tracker model rejected its input. The model decides the status.
    #[error(transparent)]
    Task(#[from] TaskError),
    /// A user model rejected its input. The model decides the status.
    #[error(transparent)]
    User(#[from] UserError),
    /// A project model rejected its input. The model decides the status.
    #[error(transparent)]
    Project(#[from] ProjectError),
    /// A shared-directory model rejected its input. The model decides the
    /// status.
    #[error(transparent)]
    SharedDir(#[from] SharedDirError),
    /// An agent-profile model rejected its input. The model decides the
    /// status.
    #[error(transparent)]
    Profile(#[from] ProfileError),
}

impl Error {
    /// The HTTP status this error maps to.
    ///
    /// Public so the WebSocket, SSE and MCP layers can reuse the mapping
    /// without building an HTTP response.
    pub fn status(&self) -> StatusCode {
        match self {
            Error::BadRequest(_) => StatusCode::BAD_REQUEST,
            Error::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Error::Forbidden(_) => StatusCode::FORBIDDEN,
            Error::NotFound | Error::Missing(_) => StatusCode::NOT_FOUND,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::GitConflict { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Error::Throttled(_) => StatusCode::TOO_MANY_REQUESTS,
            Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Error::Claims(err) => err.status(),
            Error::Engine(err) => err.status(),
            Error::Email(err) => err.status(),
            Error::Git(err) => err.status(),
            Error::Secrets(err) => err.status(),
            Error::Database(sqlx::Error::RowNotFound) => StatusCode::NOT_FOUND,
            Error::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Error::Secret(err) => err.status(),
            Error::Session(err) => err.status(),
            Error::Task(err) => err.status(),
            Error::User(err) => err.status(),
            Error::Project(err) => err.status(),
            Error::SharedDir(err) => err.status(),
            Error::Profile(err) => err.status(),
        }
    }

    /// The message a client is allowed to see.
    ///
    /// Public for the same reason [`Error::status`] is: the response body is
    /// not the only place this message goes. A `git` outcome event records the
    /// failure of a merge, a rebase or a push in its `detail.error`
    /// (`SPEC.md`, "AgentEvent"), and that field is this message — never git's
    /// stderr, which may name a path or a remote, and never an internal
    /// detail (`CLAUDE.md` rule 3). Every 5xx answers the generic
    /// `internal error`; the detail stays in the log line
    /// [`IntoResponse`](Error::into_response) writes.
    pub fn user_message(&self) -> String {
        match self {
            Error::GitConflict { message, .. } => message.clone(),
            // `From<GitError>` routes conflicts to `GitConflict`, so this only
            // catches one constructed by hand. It is here so a 422 can never
            // go out without the message the contract promises.
            Error::Git(GitError::Conflict { .. }) => GIT_CONFLICT_MESSAGE.to_string(),
            Error::Database(sqlx::Error::RowNotFound) => Error::NotFound.to_string(),
            other if other.status().is_server_error() => INTERNAL_MESSAGE.to_string(),
            other => other.to_string(),
        }
    }

    /// Did Postgres abort this transaction to break a deadlock or a
    /// serialisation cycle?
    ///
    /// SQLSTATE class 40: `40P01` `deadlock_detected` and `40001`
    /// `serialization_failure`. Both mean the transaction was rolled back
    /// whole — no row, no event, no notification survived it — so the
    /// operation can simply be run again, which is what
    /// [`retry_on_serialization_failure`](crate::tracker::retry_on_serialization_failure)
    /// does. It is *not* a failure of the caller's request and must never be
    /// answered as one: a client sees the retry's outcome
    /// (`ARCHITECTURE.md`, "Task tracker" → "Lock order").
    pub fn is_serialization_failure(&self) -> bool {
        let Error::Database(err) = self else {
            return false;
        };

        err.as_database_error()
            .and_then(|err| err.code())
            .is_some_and(|code| code == "40P01" || code == "40001")
    }

    /// The conflicting paths this failure carries, for the 422 body's
    /// `conflicts` and the `git` event detail's (`SPEC.md`, "REST API").
    pub fn conflicts(&self) -> Option<Vec<String>> {
        match self {
            Error::GitConflict { conflicts, .. } => Some(conflicts.clone()),
            Error::Git(GitError::Conflict { paths }) => Some(paths.clone()),
            _ => None,
        }
    }
}

/// Not `#[from]`, because one git failure does not belong in
/// [`Error::Git`]: a merge or rebase that stopped on conflicting paths is the
/// documented 422, and the paths have to reach the response body
/// (`SPEC.md`, "REST API"). Routing it here rather than at each call site is
/// what makes `?` on a git operation correct everywhere.
impl From<GitError> for Error {
    fn from(err: GitError) -> Self {
        match err {
            GitError::Conflict { paths } => Error::GitConflict {
                message: GIT_CONFLICT_MESSAGE.to_string(),
                conflicts: paths,
            },
            other => Error::Git(other),
        }
    }
}

/// The message a 422 carries; the conflicting paths are the detail.
const GIT_CONFLICT_MESSAGE: &str = "merge conflict";

/// The only JSON shape an error ever produces (`SPEC.md`, "REST API").
#[derive(Debug, Serialize)]
struct ErrorBody {
    status: u16,
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    conflicts: Option<Vec<String>>,
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status();

        match &self {
            Error::Database(sqlx::Error::RowNotFound) => {}
            Error::Database(err) => error!(error = ?err, "internal error"),
            Error::Internal(detail) => error!(error = %detail, "internal error"),
            // Every other 5xx — an engine, mail, git or secrets failure, and
            // whatever later epics add — is logged once here with its detail,
            // because the response body only ever says "internal error".
            other if status.is_server_error() => error!(error = %other, "internal error"),
            other => debug!(status = status.as_u16(), error = %other, "request failed"),
        }

        // The same two answers a `git` outcome event records, so a failure
        // cannot describe itself one way in the response and another in the
        // session transcript.
        let body = ErrorBody {
            status: status.as_u16(),
            error: self.user_message(),
            conflicts: self.conflicts(),
        };

        (status, axum::Json(body)).into_response()
    }
}

impl From<JsonRejection> for Error {
    fn from(rejection: JsonRejection) -> Self {
        Error::BadRequest(rejection.body_text())
    }
}

impl From<PathRejection> for Error {
    fn from(rejection: PathRejection) -> Self {
        Error::BadRequest(rejection.body_text())
    }
}

impl From<QueryRejection> for Error {
    fn from(rejection: QueryRejection) -> Self {
        Error::BadRequest(rejection.body_text())
    }
}

/// `axum::Json` with [`Error`] as its rejection, so a malformed body answers in
/// the documented `{ status, error }` shape. Handlers use this wrapper for both
/// request extraction and responses; it shadows `axum::Json` in every module
/// that does `use crate::prelude::*;`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    axum::Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = Error;

    async fn from_request(req: Request, state: &S) -> std::result::Result<Self, Self::Rejection> {
        let axum::Json(value) = axum::Json::<T>::from_request(req, state).await?;
        Ok(Self(value))
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// `axum::extract::Path` with [`Error`] as its rejection, so
/// `/secrets/not-a-uuid` answers `400 { "status", "error" }` rather than
/// axum's plain text (`SPEC.md`, "REST API"; `ARCHITECTURE.md`, "Orchestrator
/// internals", Errors).
///
/// Here rather than in `routes::extractors` because it is the same promise
/// [`Json`] makes and there is no reason for the two to live apart: a module
/// that does `use crate::prelude::*;` gets the wrapper without asking, which
/// is what keeps a route from reaching for axum's own by accident.
#[derive(Debug, Clone, Copy)]
pub struct Path<T>(pub T);

impl<T, S> FromRequestParts<S> for Path<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
        let axum::extract::Path(value) =
            axum::extract::Path::<T>::from_request_parts(parts, state).await?;

        Ok(Self(value))
    }
}

/// `axum::extract::Query` with [`Error`] as its rejection, so a `scope` that
/// is not one of the documented values or a `limit` that is not a number
/// answers in the same `{ status, error }` shape as [`Path`] and [`Json`].
#[derive(Debug, Clone, Copy)]
pub struct Query<T>(pub T);

impl<T, S> FromRequestParts<S> for Query<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
        let axum::extract::Query(value) =
            axum::extract::Query::<T>::from_request_parts(parts, state).await?;

        Ok(Self(value))
    }
}

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request as HttpRequest, header::CONTENT_TYPE};
    use serde_json::{Value, json};

    use super::*;

    async fn response_of(error: Error) -> (StatusCode, String) {
        let response = error.into_response();
        let status = response.status();
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    async fn assert_maps_to(error: Error, expected_status: StatusCode, expected_body: Value) {
        let (status, body) = response_of(error).await;
        assert_eq!(status, expected_status);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            expected_body,
            "unexpected body: {body}"
        );
    }

    #[tokio::test]
    async fn bad_request_is_400() {
        assert_maps_to(
            Error::BadRequest("username is too short".into()),
            StatusCode::BAD_REQUEST,
            json!({ "status": 400, "error": "username is too short" }),
        )
        .await;
    }

    #[tokio::test]
    async fn unauthorized_is_401() {
        assert_maps_to(
            Error::Unauthorized("authentication required".into()),
            StatusCode::UNAUTHORIZED,
            json!({ "status": 401, "error": "authentication required" }),
        )
        .await;
    }

    #[tokio::test]
    async fn a_rejected_access_token_is_401_and_says_no_more() {
        for error in [ClaimsError::Invalid, ClaimsError::Expired] {
            // Expired and forged answer identically: which half of a forgery
            // worked is not something a caller gets to learn.
            assert_maps_to(
                Error::from(error),
                StatusCode::UNAUTHORIZED,
                json!({ "status": 401, "error": "authentication required" }),
            )
            .await;
        }
    }

    #[tokio::test]
    async fn forbidden_is_403_with_the_call_site_s_message() {
        for message in ["password change required", "admin required"] {
            assert_maps_to(
                Error::Forbidden(message.into()),
                StatusCode::FORBIDDEN,
                json!({ "status": 403, "error": message }),
            )
            .await;
        }
    }

    #[tokio::test]
    async fn not_found_is_404() {
        assert_maps_to(
            Error::NotFound,
            StatusCode::NOT_FOUND,
            json!({ "status": 404, "error": "not found" }),
        )
        .await;
    }

    #[tokio::test]
    async fn a_named_404_is_404_with_the_call_site_s_message() {
        // The variant exists so that this body is distinguishable from the
        // generic one above; if it ever collapsed back into `not found` the
        // callers that route on it would silently stop working.
        assert_maps_to(
            Error::Missing("dependency not found".into()),
            StatusCode::NOT_FOUND,
            json!({ "status": 404, "error": "dependency not found" }),
        )
        .await;
    }

    #[tokio::test]
    async fn conflict_is_409() {
        assert_maps_to(
            Error::Conflict("duplicate name".into()),
            StatusCode::CONFLICT,
            json!({ "status": 409, "error": "duplicate name" }),
        )
        .await;
    }

    #[tokio::test]
    async fn git_conflict_is_422_with_conflicting_paths() {
        assert_maps_to(
            Error::GitConflict {
                message: "merge failed with conflicts".into(),
                conflicts: vec!["src/main.rs".into(), "README.md".into()],
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "status": 422,
                "error": "merge failed with conflicts",
                "conflicts": ["src/main.rs", "README.md"],
            }),
        )
        .await;
    }

    #[tokio::test]
    async fn throttled_is_429_with_the_call_site_s_message() {
        assert_maps_to(
            Error::Throttled("too many login attempts".into()),
            StatusCode::TOO_MANY_REQUESTS,
            json!({ "status": 429, "error": "too many login attempts" }),
        )
        .await;
    }

    #[tokio::test]
    async fn internal_is_500_and_never_leaks_its_detail() {
        let (status, body) = response_of(Error::Internal("secret detail".into())).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            json!({ "status": 500, "error": "internal error" }),
        );
        assert!(!body.contains("secret detail"), "leaked body: {body}");
    }

    #[tokio::test]
    async fn database_row_not_found_is_404() {
        assert_maps_to(
            Error::Database(sqlx::Error::RowNotFound),
            StatusCode::NOT_FOUND,
            json!({ "status": 404, "error": "not found" }),
        )
        .await;
    }

    #[tokio::test]
    async fn other_database_errors_are_500_without_detail() {
        let (status, body) = response_of(Error::Database(sqlx::Error::Protocol(
            "unexpected packet from server".into(),
        )))
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            json!({ "status": 500, "error": "internal error" }),
        );
        assert!(!body.contains("unexpected packet"), "leaked body: {body}");
    }

    #[tokio::test]
    async fn collaborator_errors_are_500_without_detail() {
        let error = Error::from(EngineError::Connection(
            "connect /run/podman.sock: refused".into(),
        ));
        assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let (status, body) = response_of(error).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            json!({ "status": 500, "error": "internal error" }),
        );
        assert!(!body.contains("podman.sock"), "leaked body: {body}");

        for error in [
            Error::from(EmailError::Transport(
                "connect api.resend.com: refused".into(),
            )),
            Error::from(GitError::Command {
                args: vec!["rev-parse".into(), "--git-dir".into()],
                code: Some(128),
                stderr: "fatal: not a git repository".into(),
            }),
            Error::from(SecretsError::InvalidMasterKey("version 1".into())),
        ] {
            assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let (_, body) = response_of(error).await;
            assert_eq!(
                serde_json::from_str::<Value>(&body).unwrap(),
                json!({ "status": 500, "error": "internal error" }),
            );
        }
    }

    #[tokio::test]
    async fn an_engine_state_conflict_is_409_with_the_engine_s_own_message() {
        // The one engine failure that is not an internal fault: the caller
        // asked for something the container's state does not allow, so the
        // message is the answer rather than something to hide.
        assert_maps_to(
            Error::from(EngineError::Conflict(
                "container mars-session-x is already running".into(),
            )),
            StatusCode::CONFLICT,
            json!({
                "status": 409,
                "error": "the container engine reports a conflict: container mars-session-x is already running",
            }),
        )
        .await;
    }

    #[tokio::test]
    async fn a_git_conflict_becomes_the_422_that_carries_its_paths() {
        // The whole point of the hand-written `From<GitError>`: a `?` on a
        // merge produces the documented body, not a bare 422.
        assert_maps_to(
            Error::from(GitError::Conflict {
                paths: vec!["src/main.rs".into(), "README.md".into()],
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "status": 422,
                "error": "merge conflict",
                "conflicts": ["src/main.rs", "README.md"],
            }),
        )
        .await;
    }

    #[tokio::test]
    async fn a_conflict_left_inside_error_git_still_answers_with_its_paths() {
        assert_maps_to(
            Error::Git(GitError::Conflict {
                paths: vec!["src/lib.rs".into()],
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "status": 422,
                "error": "merge conflict",
                "conflicts": ["src/lib.rs"],
            }),
        )
        .await;
    }

    #[tokio::test]
    async fn a_non_fast_forward_push_is_409_and_names_the_branch() {
        assert_maps_to(
            Error::from(GitError::NonFastForward {
                remote_branch: "main".into(),
            }),
            StatusCode::CONFLICT,
            json!({
                "status": 409,
                "error": "the upstream branch main has moved on; push rejected as non-fast-forward",
            }),
        )
        .await;
    }

    #[tokio::test]
    async fn a_missing_credential_and_a_dirty_work_tree_are_409() {
        for error in [GitError::CredentialUnavailable, GitError::DirtyWorkTree] {
            let expected = error.to_string();
            assert_maps_to(
                Error::from(error),
                StatusCode::CONFLICT,
                json!({ "status": 409, "error": expected }),
            )
            .await;
        }
    }

    #[tokio::test]
    async fn a_ref_the_caller_named_wrong_is_400_with_the_name_in_the_message() {
        for (error, name) in [
            (GitError::InvalidRef("origin/main".into()), "origin/main"),
            (
                GitError::UnknownRef("refs/heads/gone".into()),
                "refs/heads/gone",
            ),
            (
                GitError::NotACommit("v1.0.0^{tree}".into()),
                "v1.0.0^{tree}",
            ),
        ] {
            let error = Error::from(error);
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);

            let (status, body) = response_of(error).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(body.contains(name), "the name is missing from {body}");
        }
    }

    #[tokio::test]
    async fn a_failed_git_command_is_500_and_never_leaks_its_argv_or_stderr() {
        let (status, body) = response_of(Error::from(GitError::Command {
            args: vec!["push".into(), "origin".into()],
            code: Some(128),
            stderr: "fatal: could not read Username for 'https://example.invalid'".into(),
        }))
        .await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            json!({ "status": 500, "error": "internal error" }),
        );
        assert!(!body.contains("example.invalid"), "leaked body: {body}");
    }

    #[tokio::test]
    async fn json_rejection_is_a_bad_request() {
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from("{ not json"))
            .unwrap();

        let error = Json::<Value>::from_request(request, &())
            .await
            .expect_err("malformed JSON must be rejected");

        assert!(matches!(error, Error::BadRequest(_)));
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn json_extractor_accepts_a_valid_body() {
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"name":"mars"}"#))
            .unwrap();

        let Json(value) = Json::<Value>::from_request(request, &()).await.unwrap();
        assert_eq!(value, json!({ "name": "mars" }));
    }
}
