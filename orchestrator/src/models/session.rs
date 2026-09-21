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

use crate::events::StopSignal;
use crate::models::agent_profile::ProfileKind;
// The crate convention (`CLAUDE.md`, "Backend conventions"); see `task.rs`.
#[allow(unused_imports)]
use crate::prelude::*;

/// What a session was launched as (`docs/data-model.md`, "Enums",
/// `profile_kind`).
///
/// The same enum as [`ProfileKind`] rather than a copy of it: `sessions.kind`
/// is the `profile_kind` column, the session records the profile's kind at
/// launch (`SPEC.md`, "Sessions") and two enums with the same two values would
/// only invite a mismatched conversion. The alias is the name the session code
/// reads better under, and `SessionKind::from(profile.kind)` is the identity.
pub type SessionKind = ProfileKind;

/// The longest title a caller may set on `POST /projects/{pid}/sessions` or
/// `PUT /sessions/{id}`, in characters after trimming (`SPEC.md`, "Sessions").
pub const MAX_SESSION_TITLE_CHARS: usize = 200;

/// The longest title derived from a first message, in characters (`SPEC.md`,
/// "Sessions").
pub const MAX_DERIVED_TITLE_CHARS: usize = 80;

/// Who launched a session (`docs/data-model.md`, "Enums",
/// `session_launch_source`).
///
/// The record of the launch actor, kept because `created_by` cannot be it:
/// deleting a user nulls that column on every session that user launched, so
/// the same NULL would mean both "the machine started this" and "a person who
/// no longer has an account started this" (`ARCHITECTURE.md`, "Task tracker" →
/// "Unattended launches"; ADR 0042).
///
/// Written once, at insert, from the launch actor
/// ([`crate::session::LaunchActor`]) and never updated: a retry or a resume is
/// the same session, launched by whoever launched it first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "session_launch_source", rename_all = "snake_case")]
pub enum SessionLaunchSource {
    /// A person, through `POST /projects/{pid}/sessions`.
    User,
    /// The dispatcher, putting a profile on a claimable task by itself.
    Dispatcher,
    /// A scheduled agent, at a tick of its own cron expression.
    Schedule,
}

impl SessionLaunchSource {
    /// The value as it is spelled in the database and in the API.
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionLaunchSource::User => "user",
            SessionLaunchSource::Dispatcher => "dispatcher",
            SessionLaunchSource::Schedule => "schedule",
        }
    }
}

impl std::fmt::Display for SessionLaunchSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

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
    ///
    /// `creating → done` is the user who ends a session they have just
    /// launched: the launch is cancelled, whatever container it had got as far
    /// as creating is removed, and the session closes without ever having run
    /// (`ARCHITECTURE.md`, "Session lifecycle"; task `qhyhw`).
    pub fn can_transition_to(self, to: SessionState) -> bool {
        use SessionState::*;

        matches!(
            (self, to),
            (Creating, Running)
                | (Creating, Done)
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
    /// A title was given but was empty or too long once trimmed.
    #[error("title must be 1 to 200 characters")]
    InvalidTitle,
    /// A cost or token increment was negative, or not a finite number.
    #[error("usage increments must be finite and not negative")]
    InvalidUsage,
    /// Input was addressed to an ephemeral session, which only ever takes its
    /// launch prompt (ADR 0003; `ARCHITECTURE.md`, "Session lifecycle").
    #[error("ephemeral sessions accept no input")]
    EphemeralInput,
    /// The session is in a final state, so there is nothing to send input to.
    #[error("session is {0}")]
    NotAcceptingInput(SessionState),
    /// An ephemeral session is never retried; a new one is launched instead
    /// (ADR 0003).
    #[error("ephemeral sessions are not retried; launch a new one")]
    EphemeralNotRetried,
    /// Only a `failed` session can be retried.
    #[error("session is {0}, only a failed session can be retried")]
    NotRetryable(SessionState),
    /// An ephemeral session has no interactive input, so it needs its prompt at
    /// launch (`SPEC.md`, "Sessions").
    #[error("an ephemeral session needs a task_id or a message")]
    EphemeralRequiresPrompt,
    /// A live session cannot be deleted; it has to end first.
    #[error("session must be done or failed")]
    NotDeletable(SessionState),
}

impl SessionError {
    /// The HTTP status this rejection maps to.
    ///
    /// Everything the session's current state refuses is a conflict — the same
    /// request would have succeeded, or will succeed, in another state — and
    /// everything malformed in the request itself is a 400. An ephemeral
    /// session's refusals are conflicts too: the session's kind is the state of
    /// the world the caller is fighting, not a field they can fix.
    pub fn status(&self) -> StatusCode {
        match self {
            SessionError::InvalidTransition { .. }
            | SessionError::EphemeralInput
            | SessionError::NotAcceptingInput(_)
            | SessionError::EphemeralNotRetried
            | SessionError::NotRetryable(_)
            | SessionError::NotDeletable(_) => StatusCode::CONFLICT,
            SessionError::InvalidBranch
            | SessionError::InvalidTitle
            | SessionError::InvalidUsage
            | SessionError::EphemeralRequiresPrompt => StatusCode::BAD_REQUEST,
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

/// A validated session title: trimmed, 1 to [`MAX_SESSION_TITLE_CHARS`]
/// characters.
///
/// The title is a label, not an identifier, so nothing is normalised beyond the
/// trim; the schema's column is unbounded `TEXT` and the 200-character limit is
/// the API contract (`SPEC.md`, "Sessions"). A *derived* title is truncated to
/// [`MAX_DERIVED_TITLE_CHARS`] by [`default_title`] instead of being rejected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionTitle(String);

impl SessionTitle {
    /// Trim `raw` and accept it when it is 1 to 200 characters long.
    pub fn parse(raw: &str) -> SessionResult<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_SESSION_TITLE_CHARS {
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

/// Validate a caller-supplied title and return it trimmed
/// (`PUT /sessions/{id}`, `SPEC.md`, "Sessions").
///
/// The same rule as [`SessionTitle::parse`], for the routes that bind a plain
/// `String`.
pub fn validate_title(raw: &str) -> SessionResult<String> {
    SessionTitle::parse(raw).map(String::from)
}

/// The title a session gets when the caller did not give one (`SPEC.md`,
/// "Sessions").
///
/// The task's title when the session was launched for a task, else the first
/// line of the first message — the first line, not the first non-empty one —
/// trimmed and truncated to [`MAX_DERIVED_TITLE_CHARS`] characters, else
/// `None`. A message whose first line is blank leaves the session untitled; a
/// `PUT` can name it later.
///
/// Truncation counts characters and cuts on a char boundary, so a multi-byte
/// title can never panic or be split mid-character.
pub fn default_title(task_title: Option<&str>, message: Option<&str>) -> Option<String> {
    if let Some(title) = task_title {
        let trimmed = title.trim();
        if !trimmed.is_empty() {
            return Some(truncate_chars(trimmed, MAX_SESSION_TITLE_CHARS));
        }
    }

    let first_line = message?.split('\n').next().unwrap_or_default().trim();
    if first_line.is_empty() {
        return None;
    }

    Some(truncate_chars(first_line, MAX_DERIVED_TITLE_CHARS))
}

/// `text` cut to at most `max` characters, on a char boundary.
fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((byte, _)) => text[..byte].to_string(),
        None => text.to_string(),
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
    pub launch_source: SessionLaunchSource,
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

impl Session {
    /// The branch a session's work lives on, from its id alone
    /// (`docs/data-model.md`, `sessions.branch`).
    ///
    /// The associated form of [`session_branch`], for the call sites that
    /// already have the `Session` type in scope.
    pub fn branch_name(id: Uuid) -> String {
        session_branch(id)
    }

    /// Whether this session can be sent a message now (`ARCHITECTURE.md`,
    /// "Session lifecycle", the state table).
    ///
    /// A conversational session takes input in `creating`, `running` and
    /// `parked` — queued while `creating`, relaunching from `parked` — and
    /// nothing at all once it is `done` or `failed`. An ephemeral session takes
    /// only its launch prompt, in any state (ADR 0003).
    pub fn accepts_input(&self) -> SessionResult<()> {
        if self.kind == SessionKind::Ephemeral {
            return Err(SessionError::EphemeralInput);
        }

        match self.state {
            SessionState::Creating | SessionState::Running | SessionState::Parked => Ok(()),
            state @ (SessionState::Done | SessionState::Failed) => {
                Err(SessionError::NotAcceptingInput(state))
            }
        }
    }

    /// Whether this session can be retried, which moves it `failed → parked`
    /// and relaunches (`ARCHITECTURE.md`, "Session lifecycle").
    ///
    /// Only a conversational session, and only from `failed`. An ephemeral one
    /// is never retried; the caller launches a new one (ADR 0003).
    pub fn can_retry(&self) -> SessionResult<()> {
        if self.kind == SessionKind::Ephemeral {
            return Err(SessionError::EphemeralNotRetried);
        }

        if self.state != SessionState::Failed {
            return Err(SessionError::NotRetryable(self.state));
        }

        Ok(())
    }

    /// Whether this session can be deleted, which only a session that has
    /// stopped can be (`SPEC.md`, "Sessions", `DELETE /sessions/{id}`).
    pub fn can_delete(&self) -> SessionResult<()> {
        match self.state {
            SessionState::Done | SessionState::Failed => Ok(()),
            state => Err(SessionError::NotDeletable(state)),
        }
    }
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
    /// Who launched this session, from the launch actor. Set once, here, and
    /// never updated (see [`SessionLaunchSource`]).
    pub launch_source: SessionLaunchSource,
    pub title: Option<SessionTitle>,
    pub base_ref: String,
    pub branch: String,
    pub mcp_token_hash: String,
    pub task_id: Option<Uuid>,
    pub handoff_id: Option<Uuid>,
}

impl NewSession {
    /// A session with a fresh id, its derived branch and no task, hand-off or
    /// title, launched by a user.
    ///
    /// `launch_source` starts at [`SessionLaunchSource::User`] because that is
    /// what the column's default is and what every launch a person makes is;
    /// an unattended launch sets it from its own actor
    /// ([`crate::session::LaunchActor::launch_source`]).
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
            launch_source: SessionLaunchSource::User,
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

/// Reject an ephemeral launch that carries no prompt (`SPEC.md`, "Sessions",
/// `POST /projects/{pid}/sessions`).
///
/// An ephemeral session accepts no input after launch (ADR 0003), so its whole
/// prompt has to be there at launch: either a task, whose generated message
/// names it, or a `message`. A conversational session may start silent.
pub fn validate_launch_prompt(
    kind: SessionKind,
    task_id: Option<Uuid>,
    message: Option<&str>,
) -> SessionResult<()> {
    let has_prompt = task_id.is_some() || message.is_some_and(|m| !m.trim().is_empty());

    if kind == SessionKind::Ephemeral && !has_prompt {
        return Err(SessionError::EphemeralRequiresPrompt);
    }

    Ok(())
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
///
/// `signal` is set when the transition was caused by a stop — `SIGINT`, then
/// `SIGTERM` after the grace period (`ARCHITECTURE.md`, "Stop semantics") — and
/// is omitted from the payload otherwise, as `SPEC.md`, "AgentEvent" specifies.
pub fn state_change_payload(
    from: SessionState,
    to: SessionState,
    reason: &str,
    signal: Option<StopSignal>,
) -> Value {
    let mut payload = serde_json::json!({
        "from": from.as_str(),
        "to": to.as_str(),
        "reason": reason,
    });

    if let Some(signal) = signal {
        payload["signal"] = serde_json::to_value(signal).unwrap_or(Value::Null);
    }

    payload
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

    const ALL_KINDS: [SessionKind; 2] = [SessionKind::Conversational, SessionKind::Ephemeral];

    fn session(kind: SessionKind, state: SessionState) -> Session {
        Session {
            id: Uuid::nil(),
            project_id: Uuid::nil(),
            profile_id: Uuid::nil(),
            kind,
            created_by: None,
            launch_source: SessionLaunchSource::User,
            title: None,
            task_id: None,
            handoff_id: None,
            state,
            base_ref: "main".into(),
            branch: session_branch(Uuid::nil()),
            container_id: None,
            cli_session_id: None,
            mcp_token_hash: FAKE_TOKEN_HASH.into(),
            last_seq: 0,
            last_activity_at: Utc::now(),
            cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            error: None,
            created_at: Utc::now(),
            parked_at: None,
            ended_at: None,
        }
    }

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
            (Creating, Done),
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
            SessionError::EphemeralRequiresPrompt,
        ] {
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            assert!(!error.to_string().is_empty(), "{error:?} has no message");
        }

        for error in [
            SessionError::EphemeralInput,
            SessionError::NotAcceptingInput(SessionState::Done),
            SessionError::EphemeralNotRetried,
            SessionError::NotRetryable(SessionState::Running),
            SessionError::NotDeletable(SessionState::Running),
        ] {
            assert_eq!(error.status(), StatusCode::CONFLICT);
            assert!(!error.to_string().is_empty(), "{error:?} has no message");
        }

        // The messages the API returns verbatim (`SPEC.md`, "Sessions").
        assert_eq!(
            SessionError::EphemeralInput.to_string(),
            "ephemeral sessions accept no input",
        );
        assert_eq!(
            SessionError::NotAcceptingInput(SessionState::Done).to_string(),
            "session is done",
        );
        assert_eq!(
            SessionError::EphemeralRequiresPrompt.to_string(),
            "an ephemeral session needs a task_id or a message",
        );
        assert_eq!(
            SessionError::NotDeletable(SessionState::Running).to_string(),
            "session must be done or failed",
        );
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
            state_change_payload(SessionState::Running, SessionState::Parked, "stopped", None),
            serde_json::json!({ "from": "running", "to": "parked", "reason": "stopped" }),
        );
    }

    #[test]
    fn a_stopped_state_change_records_the_signal() {
        assert_eq!(
            state_change_payload(
                SessionState::Running,
                SessionState::Parked,
                "stopped",
                Some(StopSignal::Sigint),
            ),
            serde_json::json!({
                "from": "running", "to": "parked", "reason": "stopped", "signal": "SIGINT",
            }),
        );
        assert_eq!(
            state_change_payload(
                SessionState::Running,
                SessionState::Failed,
                "stop timed out",
                Some(StopSignal::Sigterm),
            )["signal"],
            serde_json::json!("SIGTERM"),
        );
    }

    #[test]
    fn only_a_conversational_session_that_has_not_ended_accepts_input() {
        for state in ALL_STATES {
            let accepted = matches!(
                state,
                SessionState::Creating | SessionState::Running | SessionState::Parked
            );
            let result = session(SessionKind::Conversational, state).accepts_input();
            assert_eq!(result.is_ok(), accepted, "conversational {state}");
            if !accepted {
                assert_eq!(result, Err(SessionError::NotAcceptingInput(state)));
            }

            assert_eq!(
                session(SessionKind::Ephemeral, state).accepts_input(),
                Err(SessionError::EphemeralInput),
                "ephemeral {state}",
            );
        }
    }

    #[test]
    fn only_a_failed_conversational_session_is_retried() {
        for state in ALL_STATES {
            let result = session(SessionKind::Conversational, state).can_retry();
            if state == SessionState::Failed {
                assert!(result.is_ok());
            } else {
                assert_eq!(result, Err(SessionError::NotRetryable(state)));
            }

            assert_eq!(
                session(SessionKind::Ephemeral, state).can_retry(),
                Err(SessionError::EphemeralNotRetried),
                "ephemeral {state}",
            );
        }
    }

    #[test]
    fn only_a_stopped_session_is_deletable() {
        for kind in ALL_KINDS {
            for state in ALL_STATES {
                let result = session(kind, state).can_delete();
                if matches!(state, SessionState::Done | SessionState::Failed) {
                    assert!(result.is_ok(), "{kind:?} {state}");
                } else {
                    assert_eq!(result, Err(SessionError::NotDeletable(state)));
                }
            }
        }
    }

    #[test]
    fn an_ephemeral_launch_needs_a_task_or_a_message() {
        let task = Some(Uuid::new_v4());

        assert!(validate_launch_prompt(SessionKind::Ephemeral, task, None).is_ok());
        assert!(validate_launch_prompt(SessionKind::Ephemeral, None, Some("go")).is_ok());
        assert_eq!(
            validate_launch_prompt(SessionKind::Ephemeral, None, None),
            Err(SessionError::EphemeralRequiresPrompt),
        );
        assert_eq!(
            validate_launch_prompt(SessionKind::Ephemeral, None, Some("  \n ")),
            Err(SessionError::EphemeralRequiresPrompt),
        );

        // A conversational session may start with nothing at all.
        assert!(validate_launch_prompt(SessionKind::Conversational, None, None).is_ok());
    }

    #[test]
    fn a_default_title_prefers_the_task_and_falls_back_to_the_first_line() {
        assert_eq!(
            default_title(Some("fix the parser"), Some("ignored")).as_deref(),
            Some("fix the parser"),
        );
        assert_eq!(
            default_title(None, Some("  first line  \nsecond line\n")).as_deref(),
            Some("first line"),
        );
        assert_eq!(default_title(None, None), None);
        assert_eq!(default_title(None, Some("")), None);
        assert_eq!(default_title(None, Some("   ")), None);

        // "First line", not "first non-empty line": a leading blank line leaves
        // the session untitled.
        assert_eq!(default_title(None, Some("\nsecond line")), None);

        // An empty task title falls through to the message.
        assert_eq!(
            default_title(Some("  "), Some("from the message")).as_deref(),
            Some("from the message"),
        );
    }

    #[test]
    fn a_derived_title_is_cut_at_eighty_characters_on_a_char_boundary() {
        let long = "a".repeat(MAX_DERIVED_TITLE_CHARS + 20);
        let derived = default_title(None, Some(&long)).unwrap();
        assert_eq!(derived.chars().count(), MAX_DERIVED_TITLE_CHARS);
        assert_eq!(derived, "a".repeat(MAX_DERIVED_TITLE_CHARS));

        // Exactly 80 characters is kept whole.
        let exact = "b".repeat(MAX_DERIVED_TITLE_CHARS);
        assert_eq!(default_title(None, Some(&exact)).as_deref(), Some(&*exact));

        // A multi-byte character straddling the boundary is not split: 79 ASCII
        // characters then an emoji fits, 80 then an emoji drops it.
        let fits = format!("{}🚀", "c".repeat(MAX_DERIVED_TITLE_CHARS - 1));
        assert_eq!(default_title(None, Some(&fits)).as_deref(), Some(&*fits));

        let cut = format!("{}🚀", "c".repeat(MAX_DERIVED_TITLE_CHARS));
        let derived = default_title(None, Some(&cut)).unwrap();
        assert_eq!(derived, "c".repeat(MAX_DERIVED_TITLE_CHARS));
        assert!(!derived.contains('🚀'));

        // And a long task title is bounded by the API's own limit.
        let task = "d".repeat(MAX_SESSION_TITLE_CHARS + 5);
        assert_eq!(
            default_title(Some(&task), None).unwrap().chars().count(),
            MAX_SESSION_TITLE_CHARS,
        );
    }

    #[test]
    fn a_validated_title_is_one_to_two_hundred_characters() {
        assert_eq!(validate_title(" x ").unwrap(), "x");

        let longest = "e".repeat(MAX_SESSION_TITLE_CHARS);
        assert_eq!(validate_title(&longest).unwrap(), longest);
        // Trimming happens before counting.
        assert_eq!(validate_title(&format!("  {longest}  ")).unwrap(), longest);

        assert_eq!(validate_title(""), Err(SessionError::InvalidTitle));
        assert_eq!(
            validate_title(&"e".repeat(MAX_SESSION_TITLE_CHARS + 1)),
            Err(SessionError::InvalidTitle),
        );
        // Characters, not bytes: 200 emoji are fine.
        assert!(validate_title(&"🚀".repeat(MAX_SESSION_TITLE_CHARS)).is_ok());
    }

    #[test]
    fn a_session_kind_is_the_profiles_kind() {
        assert_eq!(
            SessionKind::from(ProfileKind::Ephemeral),
            SessionKind::Ephemeral
        );
        assert_eq!(
            serde_json::to_value(SessionKind::Conversational).unwrap(),
            serde_json::json!("conversational"),
        );
    }

    #[test]
    fn a_session_branch_is_derived_from_its_id() {
        let id = Uuid::new_v4();
        assert_eq!(Session::branch_name(id), format!("session/{id}"));
    }

    #[test]
    fn a_session_row_never_serialises_its_token_hash() {
        let session = Session {
            id: Uuid::nil(),
            project_id: Uuid::nil(),
            profile_id: Uuid::nil(),
            kind: ProfileKind::Conversational,
            created_by: None,
            launch_source: SessionLaunchSource::User,
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
