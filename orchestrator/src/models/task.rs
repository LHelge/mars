//! Tasks: the tracker's central row, its validated field types and the error
//! enum every tracker model reports through.
//!
//! `docs/data-model.md`, `tasks` is the column contract; `SPEC.md`, "Tasks" is
//! the field contract the API exposes. Validation lives here, SQL lives in
//! `repositories/` (`CLAUDE.md`, "Backend conventions").

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::task_state::is_state_name;
// The crate convention (`CLAUDE.md`, "Backend conventions"). Models report
// their own error rather than the crate-wide one, so the glob is here for the
// doc links and for what the tracker models grow into.
#[allow(unused_imports)]
use crate::prelude::*;

/// Longest accepted task title, in characters (`docs/data-model.md`, `tasks`).
pub const MAX_TITLE_CHARS: usize = 200;

/// Every way a tracker model can reject its input.
///
/// `Display` is the message the API returns in `{ status, error }`, so each
/// variant says what the caller has to change and never carries internal
/// detail. [`TaskError::status`] is the HTTP status the crate-wide [`Error`]
/// delegates to (`ARCHITECTURE.md`, "Orchestrator internals").
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskError {
    /// A task state name did not match `[a-z0-9][a-z0-9_-]*`.
    #[error(
        "state name must be 1-32 characters of lowercase letters, digits, '_' or '-', starting with a letter or digit"
    )]
    InvalidStateName,
    /// The title was empty after trimming, or longer than 200 characters.
    #[error("title must be 1-200 characters")]
    InvalidTitle,
    /// The priority was outside 0 (critical) to 3 (low).
    #[error("priority must be between 0 (critical) and 3 (low)")]
    InvalidPriority,
    /// A label did not match the state-name pattern.
    #[error(
        "each label must be 1-32 characters of lowercase letters, digits, '_' or '-', starting with a letter or digit"
    )]
    InvalidLabel,
    /// A task was given itself as its parent.
    #[error("a task cannot be its own parent")]
    SelfParent,
    /// A dependency edge pointed at its own task.
    #[error("a task cannot depend on itself")]
    SelfDependency,
    /// A comment body was empty after trimming.
    #[error("comment body must not be empty")]
    EmptyComment,
    /// A comment had no author, both authors, or an author while `system`.
    #[error("a comment has exactly one author, and a system comment has none")]
    InvalidCommentAuthor,
    /// A hand-off commit was not a full lowercase hexadecimal object id.
    #[error("commit must be a full lowercase hexadecimal git object id")]
    InvalidCommit,
    /// A hand-off had no creating actor, or both a user and a session.
    #[error("a hand-off has exactly one creating actor")]
    InvalidHandoffActor,
    /// Reviewer and review time did not match the review status.
    #[error(
        "a reviewed hand-off has exactly one reviewer and a review time, an unreviewed hand-off has neither"
    )]
    InvalidReview,
    /// A hand-off carried an empty source branch.
    #[error("source branch must not be empty")]
    EmptySourceBranch,
    /// A REST revision hand-off left out the session the commit comes from.
    #[error("revision hand-off requires source_session_id")]
    HandoffSourceRequired,
    /// An MCP revision hand-off supplied a source session of its own.
    #[error("source_session_id is derived from the calling session")]
    HandoffSourceNotAllowed,
    /// A hand-off did not come with a move to a different state.
    #[error("handoff requires a different target state")]
    HandoffRequiresStateChange,
    /// A revision hand-off carried a review decision.
    #[error("review applies to forward hand-offs only")]
    HandoffReviewOnRevision,
    /// A task reference was neither a UUID nor a per-project number.
    #[error("a task is addressed by its UUID or its per-project number")]
    InvalidTaskRef,
}

impl TaskError {
    /// The HTTP status this rejection maps to.
    ///
    /// Every variant is malformed input, so every variant is 400. Conflicts —
    /// a duplicate state name, a dependency cycle, a lost claim — are decided
    /// against the database and surface as [`Error::Conflict`] from the
    /// repository instead.
    pub fn status(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

/// The result type the tracker models return.
///
/// The prelude's `Result` is the crate-wide one; models report their own error
/// and let `?` widen it at the call site.
pub type TaskResult<T> = std::result::Result<T, TaskError>;

/// A validated task title: trimmed, 1–200 characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct TaskTitle(String);

impl TaskTitle {
    /// Trim `raw` and accept it when 1–200 characters remain.
    pub fn parse(raw: &str) -> TaskResult<Self> {
        let trimmed = raw.trim();
        let length = trimmed.chars().count();
        if length == 0 || length > MAX_TITLE_CHARS {
            return Err(TaskError::InvalidTitle);
        }
        Ok(Self(trimmed.to_string()))
    }

    /// The trimmed title.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<TaskTitle> for String {
    fn from(title: TaskTitle) -> Self {
        title.0
    }
}

impl std::fmt::Display for TaskTitle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A validated task priority: 0 (critical) to 3 (low), default 2.
///
/// The column is `SMALLINT` and the API sends a JSON number, so both paths
/// validate through [`Priority::try_from`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Priority(i16);

impl Priority {
    /// `0`: drop everything else.
    pub const CRITICAL: Self = Self(0);
    /// `1`: ahead of ordinary work.
    pub const HIGH: Self = Self(1);
    /// `2`: the default (`docs/data-model.md`, `tasks`).
    pub const MEDIUM: Self = Self(2);
    /// `3`: whenever there is room.
    pub const LOW: Self = Self(3);

    /// The stored value, ready to bind to the `SMALLINT` column.
    pub fn get(self) -> i16 {
        self.0
    }
}

impl Default for Priority {
    fn default() -> Self {
        Self::MEDIUM
    }
}

impl TryFrom<i16> for Priority {
    type Error = TaskError;

    fn try_from(value: i16) -> TaskResult<Self> {
        match value {
            0..=3 => Ok(Self(value)),
            _ => Err(TaskError::InvalidPriority),
        }
    }
}

impl From<Priority> for i16 {
    fn from(priority: Priority) -> Self {
        priority.0
    }
}

/// A validated label: 1–32 characters of the state-name pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Label(String);

impl Label {
    /// Accept `raw` when it matches the state-name pattern.
    pub fn parse(raw: &str) -> TaskResult<Self> {
        if is_state_name(raw) {
            Ok(Self(raw.to_string()))
        } else {
            Err(TaskError::InvalidLabel)
        }
    }

    /// Validate a whole list, dropping duplicates and keeping first-occurrence
    /// order so the stored `TEXT[]` mirrors what the caller sent.
    pub fn parse_list<S: AsRef<str>>(raw: &[S]) -> TaskResult<Vec<Self>> {
        let mut labels: Vec<Self> = Vec::with_capacity(raw.len());
        for candidate in raw {
            let label = Self::parse(candidate.as_ref())?;
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
        Ok(labels)
    }

    /// The label text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<Label> for String {
    fn from(label: Label) -> Self {
        label.0
    }
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a caller addressed a task: by its UUID or by its per-project number.
///
/// `SPEC.md`, "Tasks": "`{id}` and `{dep}` accept a task's UUID or its
/// per-project number", and the MCP tools accept the same two forms. The
/// number is only unique within a project, so a [`TaskRef::Number`] is
/// meaningless without the project it was read in; the repository takes both.
///
/// This is a domain identifier, not a row: the translation to a `WHERE` clause
/// lives in `repositories/tasks/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskRef {
    /// The task's own UUID.
    Id(Uuid),
    /// The task's `number` within the project it belongs to.
    Number(i32),
}

impl From<Uuid> for TaskRef {
    fn from(id: Uuid) -> Self {
        Self::Id(id)
    }
}

impl std::fmt::Display for TaskRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Id(id) => write!(f, "{id}"),
            Self::Number(number) => write!(f, "{number}"),
        }
    }
}

impl std::str::FromStr for TaskRef {
    type Err = TaskError;

    /// All digits is a number, anything else has to be a UUID.
    ///
    /// The two forms cannot collide: a UUID always carries hyphens or hex
    /// letters, and a bare run of digits is never 32 hex characters and a
    /// valid UUID at the same time. A number that does not fit an `INTEGER` —
    /// and therefore cannot be in the column — is rejected here rather than
    /// asked about, as is the `#42` the frontend's search box accepts: the
    /// `#` is display syntax, not part of the reference.
    fn from_str(raw: &str) -> TaskResult<Self> {
        if !raw.is_empty() && raw.bytes().all(|byte| byte.is_ascii_digit()) {
            return raw
                .parse::<i32>()
                .map(Self::Number)
                .map_err(|_| TaskError::InvalidTaskRef);
        }

        Uuid::parse_str(raw)
            .map(Self::Id)
            .map_err(|_| TaskError::InvalidTaskRef)
    }
}

/// A `tasks` row, column for column (`docs/data-model.md`, `tasks`).
///
/// The API-facing shape — `state` by name, `depends_on`, `blocks`, `handoff` —
/// is assembled by the routes from this row and its neighbours.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Task {
    pub id: Uuid,
    pub project_id: Uuid,
    pub number: i32,
    pub title: String,
    pub description: String,
    pub state_id: Uuid,
    pub priority: i16,
    pub blocked: bool,
    pub labels: Vec<String>,
    pub parent_id: Option<Uuid>,
    pub assignee_user_id: Option<Uuid>,
    pub lease_holder_session_id: Option<Uuid>,
    pub lease_since: Option<DateTime<Utc>>,
    pub attempts: i16,
    pub needs_human_reason: Option<String>,
    pub current_handoff_id: Option<Uuid>,
    pub created_by_user_id: Option<Uuid>,
    pub created_by_session_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// The caller-supplied half of a new task.
///
/// The id is generated up front so the caller knows it before the insert. The
/// repository fills in `number`, `blocked`, `attempts` and the timestamps, and
/// resolves `state_id` to the project's default queue state when it is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTask {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: TaskTitle,
    pub description: String,
    pub state_id: Option<Uuid>,
    pub priority: Priority,
    pub labels: Vec<Label>,
    pub parent_id: Option<Uuid>,
    pub assignee_user_id: Option<Uuid>,
    pub created_by_user_id: Option<Uuid>,
    pub created_by_session_id: Option<Uuid>,
}

impl NewTask {
    /// A task with a fresh id, the default priority, no labels and no parent.
    ///
    /// The remaining fields are public: callers set what they were given and
    /// call [`NewTask::validate`] before inserting.
    pub fn new(project_id: Uuid, title: &str) -> TaskResult<Self> {
        Ok(Self {
            id: Uuid::new_v4(),
            project_id,
            title: TaskTitle::parse(title)?,
            description: String::new(),
            state_id: None,
            priority: Priority::default(),
            labels: Vec::new(),
            parent_id: None,
            assignee_user_id: None,
            created_by_user_id: None,
            created_by_session_id: None,
        })
    }

    /// The cross-field rules; the per-field rules are the newtypes themselves.
    ///
    /// The remaining parent rules — same project, one level deep — need the
    /// other rows and belong to the repository under the project lock.
    pub fn validate(&self) -> TaskResult<()> {
        if self.parent_id == Some(self.id) {
            return Err(TaskError::SelfParent);
        }
        Ok(())
    }

    /// The labels as the `TEXT[]` binding wants them.
    pub fn label_strings(&self) -> Vec<String> {
        self.labels
            .iter()
            .map(|label| label.as_str().to_string())
            .collect()
    }
}

/// The fields `PUT /projects/{pid}/tasks/{id}` can change directly
/// (`SPEC.md`, "Tasks").
///
/// `None` means "leave it alone"; the two nested options mean "set it to
/// NULL". The state, the lease, `attempts`, `closed_at`, `blocked`,
/// `needs_human_reason` and the current hand-off are deliberately absent: they
/// are the tracker's to compose out of a state move, a claim, a release or an
/// escalation, through the crate-private `TaskRepository::set_task_state_fields`,
/// never a field a body sets on its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskUpdate {
    pub title: Option<TaskTitle>,
    pub description: Option<String>,
    pub priority: Option<Priority>,
    pub labels: Option<Vec<Label>>,
    pub assignee_user_id: Option<Option<Uuid>>,
    pub parent_id: Option<Option<Uuid>>,
}

impl TaskUpdate {
    /// Whether this update mentions any field at all.
    ///
    /// Mentioning a field is not the same as changing it: whether the values
    /// differ from the stored row is decided by the repository under the
    /// project lock, because only the row read there is authoritative
    /// (ADR 0030).
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.priority.is_none()
            && self.labels.is_none()
            && self.assignee_user_id.is_none()
            && self.parent_id.is_none()
    }

    /// The new labels as the `TEXT[]` binding wants them, if any were given.
    pub fn label_strings(&self) -> Option<Vec<String>> {
        self.labels.as_ref().map(|labels| {
            labels
                .iter()
                .map(|label| label.as_str().to_string())
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn every_error_is_a_bad_request_with_a_message() {
        for error in [
            TaskError::InvalidStateName,
            TaskError::InvalidTitle,
            TaskError::InvalidPriority,
            TaskError::InvalidLabel,
            TaskError::SelfParent,
            TaskError::SelfDependency,
            TaskError::EmptyComment,
            TaskError::InvalidCommentAuthor,
            TaskError::InvalidCommit,
            TaskError::InvalidHandoffActor,
            TaskError::InvalidReview,
            TaskError::EmptySourceBranch,
            TaskError::HandoffSourceRequired,
            TaskError::HandoffSourceNotAllowed,
            TaskError::HandoffRequiresStateChange,
            TaskError::HandoffReviewOnRevision,
            TaskError::InvalidTaskRef,
        ] {
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            assert!(!error.to_string().is_empty(), "{error:?} has no message");
        }
    }

    #[test]
    fn the_crate_error_delegates_to_the_model() {
        let error = Error::from(TaskError::InvalidTitle);
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error.to_string(), TaskError::InvalidTitle.to_string());
    }

    #[test]
    fn title_is_trimmed() {
        let title = TaskTitle::parse("  fix the reaper \n").unwrap();
        assert_eq!(title.as_str(), "fix the reaper");
        assert_eq!(title.to_string(), "fix the reaper");
        assert_eq!(String::from(title), "fix the reaper");
    }

    #[test]
    fn title_accepts_one_and_two_hundred_characters() {
        assert_eq!(TaskTitle::parse("a").unwrap().as_str(), "a");
        let longest = "t".repeat(MAX_TITLE_CHARS);
        assert_eq!(TaskTitle::parse(&longest).unwrap().as_str(), longest);
    }

    #[test]
    fn title_rejects_empty_whitespace_and_two_hundred_and_one_characters() {
        assert_eq!(TaskTitle::parse(""), Err(TaskError::InvalidTitle));
        assert_eq!(TaskTitle::parse("   \t\n "), Err(TaskError::InvalidTitle));
        assert_eq!(
            TaskTitle::parse(&"t".repeat(MAX_TITLE_CHARS + 1)),
            Err(TaskError::InvalidTitle)
        );
    }

    #[test]
    fn title_counts_characters_not_bytes() {
        assert!(TaskTitle::parse(&"å".repeat(MAX_TITLE_CHARS)).is_ok());
        assert_eq!(
            TaskTitle::parse(&"å".repeat(MAX_TITLE_CHARS + 1)),
            Err(TaskError::InvalidTitle)
        );
    }

    #[test]
    fn priority_accepts_zero_to_three() {
        for value in 0..=3i16 {
            assert_eq!(Priority::try_from(value).unwrap().get(), value);
        }
        assert_eq!(Priority::CRITICAL.get(), 0);
        assert_eq!(Priority::HIGH.get(), 1);
        assert_eq!(Priority::MEDIUM.get(), 2);
        assert_eq!(Priority::LOW.get(), 3);
    }

    #[test]
    fn priority_rejects_four_and_minus_one() {
        assert_eq!(Priority::try_from(4), Err(TaskError::InvalidPriority));
        assert_eq!(Priority::try_from(-1), Err(TaskError::InvalidPriority));
        assert_eq!(
            Priority::try_from(i16::MAX),
            Err(TaskError::InvalidPriority)
        );
        assert_eq!(
            Priority::try_from(i16::MIN),
            Err(TaskError::InvalidPriority)
        );
    }

    #[test]
    fn priority_defaults_to_medium_and_converts_back() {
        assert_eq!(Priority::default(), Priority::MEDIUM);
        assert_eq!(i16::from(Priority::default()), 2);
        assert!(Priority::CRITICAL < Priority::LOW);
    }

    #[test]
    fn labels_accept_the_state_name_pattern() {
        let labels = Label::parse_list(&["backend", "p0", "needs-design", "a_b", "x"]).unwrap();
        assert_eq!(
            labels.iter().map(Label::as_str).collect::<Vec<_>>(),
            ["backend", "p0", "needs-design", "a_b", "x"]
        );
        assert!(Label::parse(&"l".repeat(32)).is_ok());
    }

    #[test]
    fn labels_reject_empty_long_and_uppercase() {
        assert_eq!(Label::parse(""), Err(TaskError::InvalidLabel));
        assert_eq!(Label::parse(&"l".repeat(33)), Err(TaskError::InvalidLabel));
        assert_eq!(Label::parse("Backend"), Err(TaskError::InvalidLabel));
        assert_eq!(Label::parse("-leading"), Err(TaskError::InvalidLabel));
        assert_eq!(Label::parse("with space"), Err(TaskError::InvalidLabel));
        assert_eq!(
            Label::parse_list(&["ok", "NOT OK"]),
            Err(TaskError::InvalidLabel)
        );
    }

    #[test]
    fn labels_drop_duplicates_and_keep_the_first_occurrence() {
        let labels = Label::parse_list(&["b", "a", "b", "a", "c"]).unwrap();
        assert_eq!(
            labels.iter().map(Label::as_str).collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
    }

    #[test]
    fn a_new_task_starts_with_the_documented_defaults() {
        let task = NewTask::new(Uuid::new_v4(), "  write the reaper  ").unwrap();
        assert_eq!(task.title.as_str(), "write the reaper");
        assert_eq!(task.priority, Priority::MEDIUM);
        assert!(task.labels.is_empty());
        assert!(task.state_id.is_none());
        assert!(task.description.is_empty());
        assert!(task.validate().is_ok());
    }

    #[test]
    fn a_new_task_rejects_an_invalid_title() {
        assert_eq!(
            NewTask::new(Uuid::new_v4(), " ").unwrap_err(),
            TaskError::InvalidTitle
        );
    }

    #[test]
    fn a_task_cannot_be_its_own_parent() {
        let mut task = NewTask::new(Uuid::new_v4(), "epic child").unwrap();
        task.parent_id = Some(task.id);
        assert_eq!(task.validate(), Err(TaskError::SelfParent));

        task.parent_id = Some(Uuid::new_v4());
        assert!(task.validate().is_ok());
    }

    #[test]
    fn label_strings_mirror_the_validated_labels() {
        let mut task = NewTask::new(Uuid::new_v4(), "labelled").unwrap();
        task.labels = Label::parse_list(&["backend", "backend", "infra"]).unwrap();
        assert_eq!(task.label_strings(), ["backend", "infra"]);
    }

    #[test]
    fn a_run_of_digits_is_a_task_number() {
        assert_eq!(TaskRef::from_str("42").unwrap(), TaskRef::Number(42));
        assert_eq!(TaskRef::from_str("1").unwrap(), TaskRef::Number(1));
        assert_eq!(TaskRef::from_str("0").unwrap(), TaskRef::Number(0));
        assert_eq!(
            TaskRef::from_str(&i32::MAX.to_string()).unwrap(),
            TaskRef::Number(i32::MAX)
        );
        assert_eq!(TaskRef::Number(42).to_string(), "42");
    }

    #[test]
    fn anything_else_has_to_be_a_uuid() {
        let id = Uuid::new_v4();
        assert_eq!(TaskRef::from_str(&id.to_string()).unwrap(), TaskRef::Id(id));
        // The hyphenless form is a UUID too, and is never all digits.
        assert_eq!(
            TaskRef::from_str(&id.simple().to_string()).unwrap(),
            TaskRef::Id(id)
        );
        assert_eq!(TaskRef::from(id), TaskRef::Id(id));
        assert_eq!(TaskRef::Id(id).to_string(), id.to_string());
    }

    #[test]
    fn garbage_a_hash_and_an_out_of_range_number_are_rejected() {
        for raw in [
            "",
            " ",
            "#42",
            "42 ",
            "-1",
            "+1",
            "4.2",
            "nonsense",
            // One past `INTEGER`, so no `tasks.number` can ever hold it.
            "2147483648",
            "99999999999999999999",
            "0123456789abcdef0123456789abcdef0",
        ] {
            assert_eq!(
                TaskRef::from_str(raw),
                Err(TaskError::InvalidTaskRef),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn an_update_that_mentions_nothing_is_empty() {
        let mut update = TaskUpdate::default();
        assert!(update.is_empty());
        assert!(update.label_strings().is_none());

        update.assignee_user_id = Some(None);
        assert!(!update.is_empty());

        let mut update = TaskUpdate {
            labels: Some(Label::parse_list(&["infra", "infra"]).unwrap()),
            ..TaskUpdate::default()
        };
        assert!(!update.is_empty());
        assert_eq!(update.label_strings().unwrap(), ["infra"]);

        update.labels = Some(Vec::new());
        assert!(!update.is_empty());
        assert!(update.label_strings().unwrap().is_empty());
    }
}
