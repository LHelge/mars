//! Tool failures on the wire: [`McpError`], its six documented codes and the
//! conversion into `rmcp`'s [`ErrorData`].
//!
//! `SPEC.md`, "MCP tool contracts" defines the contract implemented here: a
//! tool failure is a JSON-RPC error, not a `CallToolResult` with `is_error`,
//! its `data.code` is one of six strings, its `message` is the user-facing
//! text the tool sections quote, and a git merge or rebase that stopped on
//! conflicting paths additionally carries `data.conflicts`.
//!
//! The mapping from the crate-wide [`Error`] goes through the HTTP status that
//! error already produces ([`Error::status`]), so an MCP tool and the REST
//! endpoint behind the same operation cannot drift apart: whatever answers 409
//! over HTTP answers `conflict` here. The message is always
//! [`Error::user_message`], which is generic for every 5xx, so no internal
//! detail, credential or git stderr can reach the agent (`CLAUDE.md` rule 3;
//! `ARCHITECTURE.md`, "Orchestrator internals" → Errors).

use rmcp::model::{ErrorCode, ErrorData};
use serde_json::{Map, Value, json};

use crate::prelude::*;

/// The message every `internal` answer carries; the detail stays in the log.
const INTERNAL_MESSAGE: &str = "internal error";

/// The six error codes a tool may answer with (`SPEC.md`, "MCP tool
/// contracts").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpErrorCode {
    /// The session bearer token is missing, unknown or no longer current.
    Unauthorized,
    /// Authenticated, but the tool is not in the calling session's profile.
    Forbidden,
    /// The task, dependency or branch named does not exist in this project.
    NotFound,
    /// A state conflict: an unclaimable task, a non-fast-forward push, a merge
    /// that stopped on conflicting paths.
    Conflict,
    /// The arguments were understood but are not valid.
    InvalidArgument,
    /// An unexpected internal failure, logged and answered generically.
    Internal,
}

impl McpErrorCode {
    /// The string that goes into `data.code`.
    pub fn as_str(self) -> &'static str {
        match self {
            McpErrorCode::Unauthorized => "unauthorized",
            McpErrorCode::Forbidden => "forbidden",
            McpErrorCode::NotFound => "not_found",
            McpErrorCode::Conflict => "conflict",
            McpErrorCode::InvalidArgument => "invalid_argument",
            McpErrorCode::Internal => "internal",
        }
    }

    /// The JSON-RPC numeric code (`SPEC.md`, "MCP tool contracts").
    ///
    /// Two are the standard JSON-RPC codes; the other four are in the
    /// implementation-defined server range, because JSON-RPC has nothing to
    /// say about authorisation or state conflicts.
    pub fn json_rpc_code(self) -> i32 {
        match self {
            McpErrorCode::InvalidArgument => -32602,
            McpErrorCode::Internal => -32603,
            McpErrorCode::Unauthorized => -32001,
            McpErrorCode::Forbidden => -32003,
            McpErrorCode::NotFound => -32004,
            McpErrorCode::Conflict => -32009,
        }
    }
}

/// A tool failure, before it becomes an [`ErrorData`] at the dispatch
/// boundary.
///
/// Handlers return [`McpResult`] and let `?` widen an [`Error`] into this;
/// a tool whose message the contract fixes — `claim`'s "task is not
/// claimable", `update`'s list of valid state names — constructs it directly
/// instead, so that the text is the one `SPEC.md` quotes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpError {
    pub code: McpErrorCode,
    pub message: String,
    /// Only ever `Some` for a git merge or rebase that stopped on conflicting
    /// paths; the order is git's and is preserved.
    pub conflicts: Option<Vec<String>>,
}

impl McpError {
    fn new(code: McpErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            conflicts: None,
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(McpErrorCode::Conflict, message)
    }

    /// A merge or rebase that stopped on conflicting paths.
    pub fn conflict_with_paths(message: impl Into<String>, paths: Vec<String>) -> Self {
        Self {
            conflicts: Some(paths),
            ..Self::new(McpErrorCode::Conflict, message)
        }
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(McpErrorCode::InvalidArgument, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(McpErrorCode::Forbidden, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(McpErrorCode::NotFound, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(McpErrorCode::Unauthorized, message)
    }

    /// The only way to build an `internal` failure: the message is fixed, so a
    /// call site cannot put a detail into it by accident.
    pub fn internal() -> Self {
        Self::new(McpErrorCode::Internal, INTERNAL_MESSAGE)
    }
}

/// What every tool handler returns.
pub type McpResult<T> = std::result::Result<T, McpError>;

impl From<McpError> for ErrorData {
    fn from(err: McpError) -> Self {
        let mut data = Map::new();
        data.insert("code".to_string(), json!(err.code.as_str()));
        if let Some(conflicts) = err.conflicts {
            data.insert("conflicts".to_string(), json!(conflicts));
        }

        ErrorData::new(
            ErrorCode(err.code.json_rpc_code()),
            err.message,
            Some(Value::Object(data)),
        )
    }
}

impl From<Error> for McpError {
    fn from(err: Error) -> Self {
        let status = err.status();

        if status.is_server_error() {
            // The one place the detail is allowed to exist, and it goes to the
            // log, never to the agent (`CLAUDE.md` rule 3).
            error!(error = ?err, "internal error");
            return McpError::internal();
        }

        let message = err.user_message();

        match status.as_u16() {
            400 => McpError::invalid_argument(message),
            401 => McpError::unauthorized(message),
            403 => McpError::forbidden(message),
            404 => McpError::not_found(message),
            409 | 422 => match err.conflicts() {
                Some(paths) => McpError::conflict_with_paths(message, paths),
                None => McpError::conflict(message),
            },
            // 429 `Error::Throttled`, and any other 4xx the contract does not
            // list. No MCP tool produces one today — the rate limit sits on
            // the login route — so this is a safety net rather than a
            // contract. `forbidden` is the closest of the six: like a 429 it
            // says the request was understood and the caller identified, and
            // the server still refuses to act on it. `conflict` would invite
            // the agent to treat it as a lost race over a task and pick
            // another one, and `invalid_argument` would send it off rewriting
            // arguments that were never wrong.
            _ => McpError::forbidden(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::email::EmailError;
    use crate::git::GitError;

    use super::*;

    fn value_of(err: impl Into<McpError>) -> Value {
        serde_json::to_value(ErrorData::from(err.into())).unwrap()
    }

    #[test]
    fn the_six_codes_keep_their_strings_and_numbers() {
        for (code, string, number) in [
            (McpErrorCode::Unauthorized, "unauthorized", -32001),
            (McpErrorCode::Forbidden, "forbidden", -32003),
            (McpErrorCode::NotFound, "not_found", -32004),
            (McpErrorCode::Conflict, "conflict", -32009),
            (McpErrorCode::InvalidArgument, "invalid_argument", -32602),
            (McpErrorCode::Internal, "internal", -32603),
        ] {
            assert_eq!(code.as_str(), string);
            assert_eq!(code.json_rpc_code(), number);
        }
    }

    #[test]
    fn a_bad_request_is_invalid_argument_with_its_message() {
        let err = McpError::from(Error::BadRequest("limit must be 1 through 100".into()));
        assert_eq!(err.code, McpErrorCode::InvalidArgument);
        assert_eq!(err.message, "limit must be 1 through 100");
        assert_eq!(err.conflicts, None);
    }

    #[test]
    fn a_model_validation_error_follows_its_400_mapping() {
        let err = McpError::from(Error::from(crate::models::TaskError::InvalidTitle));
        assert_eq!(err.code, McpErrorCode::InvalidArgument);
        assert_eq!(
            err.message,
            crate::models::TaskError::InvalidTitle.to_string()
        );
    }

    #[test]
    fn a_rejected_token_is_unauthorized() {
        let err = McpError::from(Error::Unauthorized("authentication required".into()));
        assert_eq!(err.code, McpErrorCode::Unauthorized);
        assert_eq!(err.message, "authentication required");

        // The same answer whichever way the token failed to verify.
        let err = McpError::from(Error::from(ClaimsError::Expired));
        assert_eq!(err.code, McpErrorCode::Unauthorized);
        assert_eq!(err.message, "authentication required");
    }

    #[test]
    fn a_tool_outside_the_profile_is_forbidden() {
        let err = McpError::from(Error::Forbidden("tool not allowed for this profile".into()));
        assert_eq!(err.code, McpErrorCode::Forbidden);
        assert_eq!(err.message, "tool not allowed for this profile");
    }

    #[test]
    fn a_404_is_not_found_and_keeps_a_named_message() {
        let err = McpError::from(Error::NotFound);
        assert_eq!(err.code, McpErrorCode::NotFound);
        assert_eq!(err.message, "not found");

        // `update`'s `remove_depends_on` needs this text to survive so the
        // agent can tell a missing edge from a task it named wrong.
        let err = McpError::from(Error::Missing("dependency not found".into()));
        assert_eq!(err.code, McpErrorCode::NotFound);
        assert_eq!(err.message, "dependency not found");
    }

    #[test]
    fn a_409_is_conflict_and_its_message_is_passed_through_unchanged() {
        let err = McpError::from(Error::Conflict("task is not claimable".into()));
        assert_eq!(err.code, McpErrorCode::Conflict);
        assert_eq!(err.message, "task is not claimable");
        assert_eq!(err.conflicts, None);
    }

    #[test]
    fn a_non_fast_forward_push_is_conflict_without_paths() {
        let err = McpError::from(Error::from(GitError::NonFastForward {
            remote_branch: "main".into(),
        }));
        assert_eq!(err.code, McpErrorCode::Conflict);
        assert_eq!(
            err.message,
            "the upstream branch main has moved on; push rejected as non-fast-forward"
        );
        assert_eq!(err.conflicts, None);
    }

    #[test]
    fn a_422_carries_its_conflicting_paths_in_order() {
        let paths = vec![
            "src/main.rs".to_string(),
            "README.md".to_string(),
            "Cargo.toml".to_string(),
        ];
        let err = McpError::from(Error::from(GitError::Conflict {
            paths: paths.clone(),
        }));
        assert_eq!(err.code, McpErrorCode::Conflict);
        assert_eq!(err.message, "merge conflict");
        assert_eq!(err.conflicts, Some(paths));
    }

    #[test]
    fn a_422_without_paths_is_a_conflict_without_conflicts() {
        // Should not exist — `From<GitError>` always fills the list — but a
        // hand-built 422 must not produce an empty `conflicts` key either.
        let err = McpError::from(Error::GitConflict {
            message: "merge conflict".into(),
            conflicts: Vec::new(),
        });
        assert_eq!(err.code, McpErrorCode::Conflict);
        assert_eq!(err.conflicts, Some(Vec::new()));
        assert_eq!(
            value_of(Error::GitConflict {
                message: "merge conflict".into(),
                conflicts: Vec::new(),
            })["data"],
            json!({ "code": "conflict", "conflicts": [] }),
        );
    }

    #[test]
    fn a_throttled_caller_is_forbidden() {
        // Documented in the mapping: 429 has no code of its own, and
        // `forbidden` is the closest of the six.
        let err = McpError::from(Error::Throttled("too many login attempts".into()));
        assert_eq!(err.code, McpErrorCode::Forbidden);
        assert_eq!(err.message, "too many login attempts");
    }

    #[test]
    fn an_internal_error_never_exposes_its_source_message() {
        let err = McpError::from(Error::Internal("database exploded".into()));
        assert_eq!(err.code, McpErrorCode::Internal);
        assert_eq!(err.message, "internal error");
        assert_eq!(err.conflicts, None);

        let value = value_of(Error::Internal("database exploded".into()));
        assert_eq!(
            value,
            json!({
                "code": -32603,
                "message": "internal error",
                "data": { "code": "internal" },
            }),
        );
        assert!(
            !value.to_string().contains("database exploded"),
            "leaked: {value}"
        );
    }

    #[test]
    fn every_5xx_collaborator_failure_is_internal() {
        for error in [
            Error::from(EmailError::Transport("connect refused".into())),
            Error::from(GitError::Command {
                args: vec!["push".into(), "origin".into()],
                code: Some(128),
                stderr: "fatal: could not read Username for 'https://example.invalid'".into(),
            }),
            Error::Database(sqlx::Error::Protocol("unexpected packet".into())),
        ] {
            let value = value_of(error);
            assert_eq!(
                value,
                json!({
                    "code": -32603,
                    "message": "internal error",
                    "data": { "code": "internal" },
                }),
            );
        }
    }

    #[test]
    fn a_row_not_found_is_not_found_rather_than_internal() {
        let err = McpError::from(Error::Database(sqlx::Error::RowNotFound));
        assert_eq!(err.code, McpErrorCode::NotFound);
        assert_eq!(err.message, "not found");
    }

    #[test]
    fn the_serialised_shape_is_the_documented_one() {
        assert_eq!(
            value_of(Error::from(GitError::Conflict {
                paths: vec!["src/main.rs".into(), "README.md".into()],
            })),
            json!({
                "code": -32009,
                "message": "merge conflict",
                "data": {
                    "code": "conflict",
                    "conflicts": ["src/main.rs", "README.md"],
                },
            }),
        );
    }

    #[test]
    fn a_hand_built_error_serialises_without_a_conflicts_key() {
        assert_eq!(
            serde_json::to_value(ErrorData::from(McpError::invalid_argument(
                "state must be one of: backlog, ready, doing, review, done"
            )))
            .unwrap(),
            json!({
                "code": -32602,
                "message": "state must be one of: backlog, ready, doing, review, done",
                "data": { "code": "invalid_argument" },
            }),
        );
    }
}
