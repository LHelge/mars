//! One agent instance: the `sessions` row, its state machine and the
//! validation a new session has to pass.
//!
//! `docs/data-model.md`, "Sessions and events" is the column contract,
//! `ARCHITECTURE.md`, "Session lifecycle" is the state diagram this module
//! encodes and `SPEC.md`, "Sessions" is the field contract the API exposes.
//! Validation lives here, SQL lives in `repositories/sessions.rs`
//! (`CLAUDE.md`, "Backend conventions").
//!
//! Nothing here starts, stops or resumes a container: the lifecycle belongs to
//! the session epic. What this module guarantees is that an illegal state
//! transition cannot be written, that a session's branch is the one its id
//! implies, and that the accumulated cost counters only ever move forward.

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// The crate convention (`CLAUDE.md`, "Backend conventions"). Models report
// their own error rather than the crate-wide one, so the glob is here for the
// doc links.
#[allow(unused_imports)]
use crate::models::agent_profile::ProfileKind;
// The crate convention (`CLAUDE.md`, "Backend conventions"); see `task.rs`.
#[allow(unused_imports)]
use crate::prelude::*;

/// Where a session is in its lifecycle (`ARCHITECTURE.md`, "Session
/// lifecycle").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "session_state", rename_all = "snake_case")]
pub enum SessionState {
    /// The row exists; clone and container creation are in progress.
    Creating,
    /// The CLI process is alive and a `SessionOwner` is attached.
    Running,
    /// No process, resumable with `--resume`. The rest state of a
    /// conversational session.
    Parked,
    /// Ended by a user or policy, or an ephemeral session whose `result`
    /// arrived. Final.
    Done,
    /// The last launch or run failed; `sessions.error` says why.
    Failed,
}

impl SessionState {
    /// The `session_state` value as it is spelled in the database and in the
    /// `session_state` notification payload.
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionState::Creating => "creating",
            SessionState::Running => "running",
            SessionState::Parked => "parked",
            SessionState::Done => "done",
            SessionState::Failed => "failed",
        }
    }

    /// Whether `self → to` is an edge of the lifecycle diagram
    /// (`ARCHITECTURE.md`, "Session lifecycle").
    ///
    /// The diagram has no self-loops, so re-entering the current state is not
    /// a transition: parking an already parked session is a caller bug, not a
    /// no-op. `done` is final; `failed` leads back to `parked` only, which is
    /// what a conversational retry does before relaunching.
    pub fn can_transition_to(self, to: SessionState) -> bool {
        use SessionState::*;

        matches!(
            (self, to),
            (Creating, Running)
                | (Creating, Failed)
                | (Running, Parked)
                | (Running, Failed)
                | (Running, Done)
                | (Parked, Running)
                | (Parked, Done)
                | (Parked, Failed)
                | (Failed, Parked)
        )
    }
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Every way a session model can reject its input.
///
/// `Display` is the message the API returns in `{ status, error }`, so each
/// variant says what the caller has to change and carries no internal detail.
/// [`SessionError::status`] is the HTTP status the crate-wide [`Error`]
/// delegates to (`ARCHITECTURE.md`, "Orchestrator internals").
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// The lifecycle diagram has no edge from `from` to `to`.
    #[error("a session cannot move from {from} to {to}")]
    InvalidTransition {
        from: SessionState,
        to: SessionState,
    },
    /// The branch was not `session/<id>`.
    #[error("branch must be session/<id>")]
    InvalidBranch,
    /// A title was given but was empty once trimmed.
    #[error("title must not be empty")]
    InvalidTitle,
    /// A cost or token increment was negative, or not a finite number.
    #[error("usage increments must be finite and not negative")]
    InvalidUsage,
}

impl SessionError {
    /// The HTTP status this rejection maps to.
    ///
    /// A transition the diagram does not have is a state conflict rather than
    /// malformed input — the same request would have succeeded a moment
    /// earlier — so it is 409 and everything else is 400.
    pub fn status(&self) -> StatusCode {
        match self {
            SessionError::InvalidTransition { .. } => StatusCode::CONFLICT,
            SessionError::InvalidBranch
            | SessionError::InvalidTitle
            | SessionError::InvalidUsage => StatusCode::BAD_REQUEST,
        }
    }
}

/// The result type the session models return.
pub type SessionResult<T> = std::result::Result<T, SessionError>;

/// The branch a session's work always lives on (`docs/data-model.md`,
/// `sessions.branch`).
///
/// Stored in the column so it is queryable, and derived here so that exactly
/// one place in the crate knows the naming rule.
pub fn session_branch(id: Uuid) -> String {
    format!("session/{id}")
}

/// A validated session title: trimmed and non-empty.
///
/// The title is a label, not an identifier, so nothing is normalised beyond
/// the trim and there is no length bound in the schema; `SPEC.md`, "Sessions"
/// truncates a *derived* title to 80 characters at the point it derives one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionTitle(String);

impl SessionTitle {
    /// Trim `raw` and accept it when anything is left.
    pub fn parse(raw: &str) -> SessionResult<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(SessionError::InvalidTitle);
        }
        Ok(Self(trimmed.to_string()))
    }

    /// The trimmed title, ready to bind to the `TEXT` column.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<SessionTitle> for String {
    fn from(title: SessionTitle) -> Self {
        title.0
    }
}

impl std::fmt::Display for SessionTitle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A `sessions` row, column for column (`docs/data-model.md`, `sessions`).
///
/// `mcp_token_hash` is `#[serde(skip)]` so a row can never be serialised into
/// a response by accident; everything else is exactly the `Session` DTO of
/// `SPEC.md`, "Sessions", in the order that document lists it.
///
/// There is no `Deserialize`: a `Session` only ever comes out of the database.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Session {
    pub id: Uuid,
    pub project_id: Uuid,
    pub profile_id: Uuid,
    pub kind: ProfileKind,
    pub created_by: Option<Uuid>,
    pub title: Option<String>,
    pub task_id: Option<Uuid>,
    pub handoff_id: Option<Uuid>,
    pub state: SessionState,
    pub base_ref: String,
    pub branch: String,
    pub container_id: Option<String>,
    pub cli_session_id: Option<String>,
    #[serde(skip)]
    pub mcp_token_hash: String,
    pub last_seq: i64,
    pub last_activity_at: DateTime<Utc>,
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub parked_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

/// The caller-supplied half of a new session.
///
/// The id is generated up front because three things are derived from it
/// before the row exists: the branch, the session directory and the container
/// labels. The repository fills in `state`, the counters and the timestamps
/// from the column defaults.
///
/// `mcp_token_hash` is the SHA-256 of a freshly generated bearer token; the
/// raw token goes into the session's `mcp.json` and is never recoverable from
/// this hash (ADR 0029). Generating and hashing it belongs to the session
/// epic, so this struct takes the finished hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSession {
    pub id: Uuid,
    pub project_id: Uuid,
    pub profile_id: Uuid,
    pub kind: ProfileKind,
    pub created_by: Option<Uuid>,
    pub title: Option<SessionTitle>,
    pub base_ref: String,
    pub branch: String,
    pub mcp_token_hash: String,
    pub task_id: Option<Uuid>,
    pub handoff_id: Option<Uuid>,
}

impl NewSession {
    /// A session with a fresh id, its derived branch and no task, hand-off or
    /// title.
    ///
    /// The remaining fields are public: callers set what they were given, and
    /// [`NewSession::validate`] — which the repository runs before the insert
    /// — is what keeps a hand-built value honest.
    pub fn new(
        project_id: Uuid,
        profile_id: Uuid,
        kind: ProfileKind,
        base_ref: impl Into<String>,
        mcp_token_hash: impl Into<String>,
    ) -> Self {
        let id = Uuid::new_v4();

        Self {
            id,
            project_id,
            profile_id,
            kind,
            created_by: None,
            title: None,
            base_ref: base_ref.into(),
            branch: session_branch(id),
            mcp_token_hash: mcp_token_hash.into(),
            task_id: None,
            handoff_id: None,
        }
    }

    /// Set the title from raw input, rejecting one that is empty once trimmed.
    ///
    /// `None` leaves the session untitled, which is what `SPEC.md`, "Sessions"
    /// asks for when neither a task, a message nor an explicit title supplies
    /// one.
    pub fn with_title(mut self, title: Option<&str>) -> SessionResult<Self> {
        self.title = title.map(SessionTitle::parse).transpose()?;
        Ok(self)
    }

    /// Reject anything the schema would accept but the contract does not.
    ///
    /// Only the branch needs checking: the title is a [`SessionTitle`], which
    /// cannot be empty, and every other field is an id or an opaque string.
    /// The repository calls this, so the check cannot be skipped by building
    /// the struct field by field.
    pub fn validate(&self) -> SessionResult<()> {
        if self.branch != session_branch(self.id) {
            return Err(SessionError::InvalidBranch);
        }

        Ok(())
    }
}

/// What a state change carries besides the new state.
///
/// A struct rather than a bare argument because the fields a transition sets
/// are per-target and the call site should not have to remember which:
/// `parked_at`, `ended_at` and `error` are decided from the target state by
/// `SessionRepository::set_state`, and this is the caller's half.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateChange {
    /// Why the session failed. Stored in `sessions.error` when — and only
    /// when — the target state is [`SessionState::Failed`]
    /// (`docs/data-model.md`, `sessions.error`).
    pub error: Option<String>,
}

impl StateChange {
    /// A transition that carries nothing, which is every transition except
    /// one into [`SessionState::Failed`].
    pub fn plain() -> Self {
        Self::default()
    }

    /// A transition into [`SessionState::Failed`] with its reason.
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            error: Some(reason.into()),
        }
    }
}

/// The `state_change` event's payload (`SPEC.md`, "AgentEvent").
///
/// Building it here keeps `set_state` and the event that records it spelling
/// the states the same way; the caller appends the event itself, in the same
/// transaction as the state change (`ARCHITECTURE.md`, "Session owner task").
pub fn state_change_payload(from: SessionState, to: SessionState, reason: &str) -> Value {
    serde_json::json!({
        "from": from.as_str(),
        "to": to.as_str(),
        "reason": reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not a credential: an obviously fake stand-in for the SHA-256 of an MCP
    /// bearer token (`CLAUDE.md`, rule 3).
    const FAKE_TOKEN_HASH: &str = "fake-mcp-token-hash";

    const ALL_STATES: [SessionState; 5] = [
        SessionState::Creating,
        SessionState::Running,
        SessionState::Parked,
        SessionState::Done,
        SessionState::Failed,
    ];

    fn new_session() -> NewSession {
        NewSession::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            ProfileKind::Conversational,
            "main",
            FAKE_TOKEN_HASH,
        )
    }

    #[test]
    fn the_transition_table_is_the_diagram() {
        use SessionState::*;

        // Every edge of `ARCHITECTURE.md`, "Session lifecycle", and nothing
        // else. Written out rather than derived from `can_transition_to` so
        // the test fails if the function changes.
        let allowed = [
            (Creating, Running),
            (Creating, Failed),
            (Running, Parked),
            (Running, Failed),
            (Running, Done),
            (Parked, Running),
            (Parked, Done),
            (Parked, Failed),
            (Failed, Parked),
        ];

        for from in ALL_STATES {
            for to in ALL_STATES {
                let expected = allowed.contains(&(from, to));
                assert_eq!(
                    from.can_transition_to(to),
                    expected,
                    "{from} -> {to} should be {}",
                    if expected { "allowed" } else { "refused" },
                );
            }
        }
    }

    #[test]
    fn no_state_transitions_to_itself() {
        for state in ALL_STATES {
            assert!(!state.can_transition_to(state), "{state} self-loops");
        }
    }

    #[test]
    fn done_is_final_and_failed_only_leads_back_to_parked() {
        for state in ALL_STATES {
            assert!(!SessionState::Done.can_transition_to(state));
        }
        for state in ALL_STATES {
            assert_eq!(
                SessionState::Failed.can_transition_to(state),
                state == SessionState::Parked,
            );
        }
    }

    #[test]
    fn a_state_spells_itself_as_the_database_does() {
        assert_eq!(SessionState::Creating.as_str(), "creating");
        assert_eq!(SessionState::Running.to_string(), "running");
        assert_eq!(SessionState::Parked.as_str(), "parked");
        assert_eq!(SessionState::Done.as_str(), "done");
        assert_eq!(SessionState::Failed.as_str(), "failed");

        // And serde agrees, because the WebSocket payloads use it.
        assert_eq!(
            serde_json::to_value(SessionState::Failed).unwrap(),
            serde_json::json!("failed"),
        );
        assert_eq!(
            serde_json::to_value(ProfileKind::Ephemeral).unwrap(),
            serde_json::json!("ephemeral"),
        );
    }

    #[test]
    fn an_invalid_transition_is_a_conflict_and_the_others_are_bad_requests() {
        let transition = SessionError::InvalidTransition {
            from: SessionState::Done,
            to: SessionState::Running,
        };
        assert_eq!(transition.status(), StatusCode::CONFLICT);
        assert_eq!(
            transition.to_string(),
            "a session cannot move from done to running"
        );

        for error in [
            SessionError::InvalidBranch,
            SessionError::InvalidTitle,
            SessionError::InvalidUsage,
        ] {
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            assert!(!error.to_string().is_empty(), "{error:?} has no message");
        }
    }

    #[test]
    fn the_crate_error_delegates_to_the_model() {
        let error = Error::from(SessionError::InvalidTransition {
            from: SessionState::Done,
            to: SessionState::Running,
        });
        assert_eq!(error.status(), StatusCode::CONFLICT);

        let error = Error::from(SessionError::InvalidBranch);
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error.to_string(), SessionError::InvalidBranch.to_string());
    }

    #[test]
    fn a_new_session_derives_its_branch_from_its_id() {
        let session = new_session();
        assert_eq!(session.branch, format!("session/{}", session.id));
        assert_eq!(session.branch, session_branch(session.id));
        assert!(session.validate().is_ok());
        assert!(session.title.is_none());
        assert!(session.task_id.is_none());
        assert!(session.handoff_id.is_none());
    }

    #[test]
    fn a_branch_that_is_not_the_sessions_own_is_rejected() {
        let mut session = new_session();

        session.branch = "main".into();
        assert_eq!(session.validate(), Err(SessionError::InvalidBranch));

        session.branch = session_branch(Uuid::new_v4());
        assert_eq!(session.validate(), Err(SessionError::InvalidBranch));

        // Case and prefix are both part of the rule.
        session.branch = format!("SESSION/{}", session.id);
        assert_eq!(session.validate(), Err(SessionError::InvalidBranch));
        session.branch = format!("refs/heads/session/{}", session.id);
        assert_eq!(session.validate(), Err(SessionError::InvalidBranch));
    }

    #[test]
    fn a_title_is_trimmed_and_may_not_be_empty() {
        let session = new_session()
            .with_title(Some("  fix the parser \n"))
            .unwrap();
        assert_eq!(session.title.as_ref().unwrap().as_str(), "fix the parser");

        assert_eq!(
            new_session().with_title(Some("   ")).unwrap_err(),
            SessionError::InvalidTitle,
        );
        assert_eq!(
            new_session().with_title(Some("")).unwrap_err(),
            SessionError::InvalidTitle,
        );

        assert!(new_session().with_title(None).unwrap().title.is_none());
    }

    #[test]
    fn a_title_renders_as_its_trimmed_self() {
        let title = SessionTitle::parse(" fix\tthe parser ").unwrap();
        assert_eq!(title.to_string(), "fix\tthe parser");
        assert_eq!(String::from(title.clone()), "fix\tthe parser");
        assert_eq!(
            serde_json::to_value(&title).unwrap(),
            serde_json::json!("fix\tthe parser"),
        );
    }

    #[test]
    fn a_state_change_carries_a_reason_only_when_it_failed() {
        assert!(StateChange::plain().error.is_none());
        assert_eq!(
            StateChange::failed("container exited 137").error.as_deref(),
            Some("container exited 137"),
        );
        assert_eq!(StateChange::default(), StateChange::plain());
    }

    #[test]
    fn a_state_change_payload_spells_both_states() {
        assert_eq!(
            state_change_payload(SessionState::Running, SessionState::Parked, "stopped"),
            serde_json::json!({ "from": "running", "to": "parked", "reason": "stopped" }),
        );
    }

    #[test]
    fn a_session_row_never_serialises_its_token_hash() {
        let session = Session {
            id: Uuid::nil(),
            project_id: Uuid::nil(),
            profile_id: Uuid::nil(),
            kind: ProfileKind::Conversational,
            created_by: None,
            title: Some("fix the parser".into()),
            task_id: None,
            handoff_id: None,
            state: SessionState::Running,
            base_ref: "main".into(),
            branch: session_branch(Uuid::nil()),
            container_id: Some("fake-container-id".into()),
            cli_session_id: None,
            mcp_token_hash: FAKE_TOKEN_HASH.into(),
            last_seq: 7,
            last_activity_at: Utc::now(),
            cost_usd: 0.5,
            input_tokens: 10,
            output_tokens: 20,
            error: None,
            created_at: Utc::now(),
            parked_at: None,
            ended_at: None,
        };

        let json = serde_json::to_value(&session).unwrap();
        assert!(json.get("mcp_token_hash").is_none(), "leaked: {json}");
        assert_eq!(json["state"], "running");
        assert_eq!(json["kind"], "conversational");
        assert_eq!(json["last_seq"], 7);
        assert_eq!(json["title"], "fix the parser");
    }
}
