//! The API-facing task shapes (`SPEC.md`, "Tasks").
//!
//! These structs *are* the wire format: field names are `snake_case` and match
//! `SPEC.md` one for one, so `src/types/` in the frontend mirrors them
//! literally. They are read-only projections — nothing here is ever
//! deserialised from a request body or written to a table — assembled from the
//! row types by the loaders in `repositories/tasks/dto.rs`.
//!
//! What the row types do *not* carry, and these do:
//!
//! - `state` is the state's **name**, not `state_id`; the board's columns are
//!   named states (`SPEC.md`, "Task states");
//! - `depends_on` is every outgoing edge with its kind, and `blocks` is the
//!   reverse: the tasks that have a `blocks` dependency on this one;
//! - `handoff` is the record `tasks.current_handoff_id` names, resolved, or
//!   `null`.
//!
//! **Nullable fields are sent as `null`, never omitted.** The frontend types
//! are exact, so `lease_since`, `closed_at`, `parent_id` and their like always
//! appear. The one place omission is correct is `TaskEvent`'s optional
//! sections, which `SPEC.md` writes with a `?` (see
//! [`TaskEvent`](crate::events::TaskEvent)).
//!
//! `Deserialize` is derived all the same: a `TaskEvent` payload round-trips
//! through `task_events.payload`, so a stored event has to read back as the
//! task it described.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::{ReviewStatus, Task, TaskComment, TaskDependencyKind, TaskHandoff};
// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

/// One outgoing dependency edge: what this task depends on, and what that
/// means (`SPEC.md`, "Tasks": `depends_on: {task_id, kind}[]`).
///
/// `task_id` is the *other* end — the prerequisite — because the owning task
/// is the one the edge is listed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct DependencyRef {
    /// The prerequisite task.
    pub task_id: Uuid,
    /// What the edge means.
    pub kind: TaskDependencyKind,
}

/// A comment as the API sends it (`SPEC.md`, "Tasks").
///
/// The `task_comments` row field for field; bodies are stored and displayed
/// unredacted (ADR 0027).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommentDto {
    pub id: Uuid,
    pub task_id: Uuid,
    pub author_user_id: Option<Uuid>,
    pub author_session_id: Option<Uuid>,
    pub system: bool,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

impl From<TaskComment> for CommentDto {
    fn from(row: TaskComment) -> Self {
        Self {
            id: row.id,
            task_id: row.task_id,
            author_user_id: row.author_user_id,
            author_session_id: row.author_session_id,
            system: row.system,
            body: row.body,
            created_at: row.created_at,
        }
    }
}

/// A code hand-off as the API sends it (`SPEC.md`, "Code hand-offs and
/// review").
///
/// The `task_handoffs` row field for field. Every actor is optional because
/// deletion nulls the foreign keys while the branch, the commit and the review
/// timestamp remain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HandoffDto {
    pub id: Uuid,
    pub task_id: Uuid,
    pub source_session_id: Option<Uuid>,
    pub source_branch: String,
    pub commit: String,
    pub comment_id: Option<Uuid>,
    pub review_status: ReviewStatus,
    pub reviewed_by_user_id: Option<Uuid>,
    pub reviewed_by_session_id: Option<Uuid>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub created_by_user_id: Option<Uuid>,
    pub created_by_session_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

impl From<TaskHandoff> for HandoffDto {
    fn from(row: TaskHandoff) -> Self {
        Self {
            id: row.id,
            task_id: row.task_id,
            source_session_id: row.source_session_id,
            source_branch: row.source_branch,
            commit: row.commit,
            comment_id: row.comment_id,
            review_status: row.review_status,
            reviewed_by_user_id: row.reviewed_by_user_id,
            reviewed_by_session_id: row.reviewed_by_session_id,
            reviewed_at: row.reviewed_at,
            created_by_user_id: row.created_by_user_id,
            created_by_session_id: row.created_by_session_id,
            created_at: row.created_at,
        }
    }
}

/// One session's involvement with a task
/// (`TaskDetail.sessions`, `SPEC.md`, "Tasks").
///
/// The `task_sessions` row without its `task_id`: the detail it hangs off
/// already names the task.
///
/// This is also the row type the repository reads `task_sessions` into
/// (`TaskRepository::list_task_sessions`, `touch_task_session`): the table has
/// no behaviour beyond its `ON CONFLICT` rules, which live in the repository,
/// so there is no domain model between the row and this shape. `task_id` is
/// the one column left out, and every caller passes it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskSessionLinkDto {
    pub session_id: Uuid,
    pub first_touched_at: DateTime<Utc>,
    pub last_touched_at: DateTime<Utc>,
}

/// A task as the API sends it (`SPEC.md`, "Tasks").
///
/// Assembled only through [`TaskDto::from_parts`], so every producer — the
/// routes, the MCP tools and the `TaskEvent` payloads — puts the same four
/// neighbours around the row and none of them can forget one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub number: i32,
    pub title: String,
    pub description: String,
    /// The state's name, not its id.
    pub state: String,
    /// 0 (critical) to 3 (low), as a JSON number.
    pub priority: i16,
    pub blocked: bool,
    pub labels: Vec<String>,
    pub parent_id: Option<Uuid>,
    pub assignee_user_id: Option<Uuid>,
    pub lease_holder_session_id: Option<Uuid>,
    pub lease_since: Option<DateTime<Utc>>,
    pub attempts: i16,
    pub needs_human_reason: Option<String>,
    /// The record `current_handoff_id` names, or `null` — including when the
    /// column points nowhere because the record was deleted.
    pub handoff: Option<HandoffDto>,
    /// Every outgoing edge, with its kind.
    pub depends_on: Vec<DependencyRef>,
    /// The tasks that have a `blocks` dependency on this one.
    pub blocks: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl TaskDto {
    /// The single assembly function: a row, its state's name, and its three
    /// neighbouring lists.
    ///
    /// `created_by_user_id`, `created_by_session_id` and `state_id` are the
    /// row columns deliberately left behind: `SPEC.md`'s `Task` does not carry
    /// them.
    pub fn from_parts(
        task: &Task,
        state_name: &str,
        depends_on: Vec<DependencyRef>,
        blocks: Vec<Uuid>,
        handoff: Option<HandoffDto>,
    ) -> Self {
        Self {
            id: task.id,
            project_id: task.project_id,
            number: task.number,
            title: task.title.clone(),
            description: task.description.clone(),
            state: state_name.to_string(),
            priority: task.priority,
            blocked: task.blocked,
            labels: task.labels.clone(),
            parent_id: task.parent_id,
            assignee_user_id: task.assignee_user_id,
            lease_holder_session_id: task.lease_holder_session_id,
            lease_since: task.lease_since,
            attempts: task.attempts,
            needs_human_reason: task.needs_human_reason.clone(),
            handoff,
            depends_on,
            blocks,
            created_at: task.created_at,
            updated_at: task.updated_at,
            closed_at: task.closed_at,
        }
    }
}

/// A task with everything the detail drawer shows
/// (`TaskDetail = Task & { ... }`, `SPEC.md`, "Tasks").
///
/// The `Task` half is flattened, so the JSON is one object with the task's
/// fields beside `comments`, `handoffs`, `children` and `sessions` — exactly
/// what the `&` in the specification means.
///
/// Children are [`TaskDto`]s, never details: nesting is one level deep, so
/// `children[].children` does not exist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskDetailDto {
    /// The task itself, spread into this object.
    #[serde(flatten)]
    pub task: TaskDto,
    /// Oldest first.
    pub comments: Vec<CommentDto>,
    /// Oldest first; the *current* one is [`TaskDto::handoff`], not the last
    /// of these.
    pub handoffs: Vec<HandoffDto>,
    /// In board order: priority, then number.
    pub children: Vec<TaskDto>,
    /// In the order the sessions first touched the task.
    pub sessions: Vec<TaskSessionLinkDto>,
}

/// How many Unicode scalar values a `description_excerpt` keeps.
const EXCERPT_CHARS: usize = 200;

/// A task as the `ready` tool lists it (`SPEC.md`, "MCP tool contracts" →
/// `ready`).
///
/// `TaskSummary = { id, number, title, state, priority, labels,
/// description_excerpt, attempts, depends_on_count }` — a menu entry, not a
/// task: enough for an agent to choose what to claim, and nothing that would
/// tempt it to start working from the list instead of reading the task. The
/// full text, the comments and the dependencies are `get_task`'s.
///
/// Read-only like its siblings here, assembled in `tracker::leases` from the
/// repository's claimable read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskSummary {
    pub id: Uuid,
    pub number: i32,
    pub title: String,
    /// The state's name, not its id.
    pub state: String,
    /// 0 (critical) to 3 (low), as a JSON number.
    pub priority: i16,
    pub labels: Vec<String>,
    /// The description, flattened to one line and cut to 200 characters; see
    /// [`description_excerpt`].
    pub description_excerpt: String,
    pub attempts: i16,
    /// Outgoing dependencies of every kind.
    pub depends_on_count: i64,
}

/// The excerpt rule, written once (`SPEC.md`, "MCP tool contracts" →
/// `ready`).
///
/// "Trim the description, replace each newline sequence (CRLF, LF or CR) with
/// one space, then take the first 200 Unicode scalar values and trim trailing
/// whitespace, without adding an ellipsis." Each of those steps is here in
/// that order, and the order matters: flattening first means the 200 are
/// counted over the single line an agent actually reads, and trimming last
/// means a cut that lands mid-gap does not end in a space.
///
/// Scalar values, not bytes: a description of emoji or CJK text is cut after
/// 200 characters, never in the middle of one.
pub fn description_excerpt(description: &str) -> String {
    let mut flattened = String::with_capacity(description.len());
    let mut chars = description.trim().chars().peekable();

    while let Some(character) = chars.next() {
        match character {
            // CRLF is one newline sequence and becomes one space; a lone CR is
            // one too.
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                flattened.push(' ');
            }
            '\n' => flattened.push(' '),
            other => flattened.push(other),
        }
    }

    flattened
        .chars()
        .take(EXCERPT_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    fn at(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0).single().expect("a timestamp")
    }

    fn task(project_id: Uuid) -> Task {
        Task {
            id: Uuid::new_v4(),
            project_id,
            number: 7,
            title: "Wire the tracker".to_string(),
            description: String::new(),
            state_id: Uuid::new_v4(),
            priority: 2,
            blocked: false,
            labels: vec!["backend".to_string()],
            parent_id: None,
            assignee_user_id: None,
            lease_holder_session_id: None,
            lease_since: None,
            attempts: 0,
            needs_human_reason: None,
            current_handoff_id: None,
            created_by_user_id: None,
            created_by_session_id: None,
            created_at: at(1_700_000_000),
            updated_at: at(1_700_000_000),
            closed_at: None,
        }
    }

    #[test]
    fn a_handoff_carries_the_thirteen_documented_fields_and_nothing_else() {
        // An obviously fake but well-formed object id (`CLAUDE.md`, rule 3).
        const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

        let row = TaskHandoff {
            id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            source_session_id: Some(Uuid::new_v4()),
            source_branch: "session/one".to_string(),
            commit: COMMIT.to_string(),
            comment_id: Some(Uuid::new_v4()),
            review_status: ReviewStatus::ChangesRequested,
            reviewed_by_user_id: Some(Uuid::new_v4()),
            reviewed_by_session_id: None,
            reviewed_at: Some(at(1_700_000_100)),
            created_by_user_id: None,
            created_by_session_id: Some(Uuid::new_v4()),
            created_at: at(1_700_000_000),
        };
        let encoded = serde_json::to_value(HandoffDto::from(row.clone())).unwrap();
        let object = encoded.as_object().unwrap();

        // The thirteen `SPEC.md` names and no fourteenth; the encoder sorts
        // the keys, so the set is what is asserted, not their order.
        let mut documented = [
            "id",
            "task_id",
            "source_session_id",
            "source_branch",
            "commit",
            "comment_id",
            "review_status",
            "reviewed_by_user_id",
            "reviewed_by_session_id",
            "reviewed_at",
            "created_by_user_id",
            "created_by_session_id",
            "created_at",
        ];
        documented.sort_unstable();
        assert_eq!(
            object.keys().map(String::as_str).collect::<Vec<_>>(),
            documented,
        );
        assert_eq!(encoded["id"], json!(row.id));
        assert_eq!(encoded["task_id"], json!(row.task_id));
        assert_eq!(encoded["source_session_id"], json!(row.source_session_id));
        assert_eq!(encoded["source_branch"], json!("session/one"));
        assert_eq!(encoded["commit"], json!(COMMIT));
        assert_eq!(encoded["comment_id"], json!(row.comment_id));
        assert_eq!(encoded["review_status"], json!("changes_requested"));
        // A deleted actor is `null`, never an omitted key.
        assert_eq!(object.get("reviewed_by_session_id"), Some(&json!(null)));
        assert_eq!(object.get("created_by_user_id"), Some(&json!(null)));
        // RFC 3339 (`CLAUDE.md`, "API conventions").
        assert_eq!(encoded["created_at"], json!("2023-11-14T22:13:20Z"));
        assert_eq!(encoded["reviewed_at"], json!("2023-11-14T22:15:00Z"));

        // The one conversion path, and it round-trips for a stored event
        // payload.
        assert_eq!(
            serde_json::from_value::<HandoffDto>(encoded).unwrap(),
            HandoffDto::from(row),
        );
    }

    #[test]
    fn a_task_carries_the_state_name_and_a_numeric_priority() {
        let row = task(Uuid::new_v4());
        let dto = TaskDto::from_parts(&row, "backlog", Vec::new(), Vec::new(), None);
        let encoded = serde_json::to_value(&dto).unwrap();

        assert_eq!(encoded["state"], json!("backlog"));
        assert_eq!(encoded["priority"], json!(2));
        assert!(encoded["priority"].is_number());
        assert!(encoded.as_object().unwrap().get("state_id").is_none());
    }

    #[test]
    fn nullable_fields_are_sent_as_null_rather_than_omitted() {
        let row = task(Uuid::new_v4());
        let dto = TaskDto::from_parts(&row, "backlog", Vec::new(), Vec::new(), None);
        let encoded = serde_json::to_value(&dto).unwrap();
        let object = encoded.as_object().unwrap();

        for nullable in [
            "parent_id",
            "assignee_user_id",
            "lease_holder_session_id",
            "lease_since",
            "needs_human_reason",
            "handoff",
            "closed_at",
        ] {
            assert_eq!(object.get(nullable), Some(&json!(null)), "{nullable}");
        }
    }

    #[test]
    fn dependencies_carry_their_kind_and_blocks_is_a_plain_id_list() {
        let row = task(Uuid::new_v4());
        let prerequisite = Uuid::new_v4();
        let dependant = Uuid::new_v4();
        let dto = TaskDto::from_parts(
            &row,
            "ready",
            vec![
                DependencyRef {
                    task_id: prerequisite,
                    kind: TaskDependencyKind::Blocks,
                },
                DependencyRef {
                    task_id: prerequisite,
                    kind: TaskDependencyKind::DiscoveredFrom,
                },
            ],
            vec![dependant],
            None,
        );
        let encoded = serde_json::to_value(&dto).unwrap();

        assert_eq!(
            encoded["depends_on"],
            json!([
                { "task_id": prerequisite, "kind": "blocks" },
                { "task_id": prerequisite, "kind": "discovered_from" },
            ])
        );
        assert_eq!(encoded["blocks"], json!([dependant]));
    }

    #[test]
    fn a_detail_flattens_the_task_beside_its_lists() {
        let row = task(Uuid::new_v4());
        let detail = TaskDetailDto {
            task: TaskDto::from_parts(&row, "backlog", Vec::new(), Vec::new(), None),
            comments: Vec::new(),
            handoffs: Vec::new(),
            children: Vec::new(),
            sessions: Vec::new(),
        };
        let encoded = serde_json::to_value(&detail).unwrap();

        assert_eq!(encoded["id"], json!(row.id));
        assert_eq!(encoded["state"], json!("backlog"));
        assert_eq!(encoded["comments"], json!([]));
        assert_eq!(encoded["children"], json!([]));
        assert_eq!(encoded["sessions"], json!([]));
        assert_eq!(encoded["handoffs"], json!([]));
    }

    #[test]
    fn a_task_round_trips_so_a_stored_event_payload_reads_back() {
        let row = task(Uuid::new_v4());
        let dto = TaskDto::from_parts(
            &row,
            "review",
            vec![DependencyRef {
                task_id: Uuid::new_v4(),
                kind: TaskDependencyKind::Related,
            }],
            vec![Uuid::new_v4()],
            None,
        );
        let encoded = serde_json::to_value(&dto).unwrap();
        assert_eq!(serde_json::from_value::<TaskDto>(encoded).unwrap(), dto);
    }

    #[test]
    fn a_session_link_drops_the_task_id_the_detail_already_names() {
        let link = TaskSessionLinkDto {
            session_id: Uuid::new_v4(),
            first_touched_at: at(1_700_000_000),
            last_touched_at: at(1_700_000_100),
        };
        let encoded = serde_json::to_value(link).unwrap();

        assert_eq!(encoded["session_id"], json!(link.session_id));
        assert!(encoded.as_object().unwrap().get("task_id").is_none());
    }

    #[test]
    fn an_excerpt_is_trimmed_and_flattened_onto_one_line() {
        assert_eq!(
            description_excerpt("  First line.\nSecond line.\r\nThird.\rFourth.  "),
            "First line. Second line. Third. Fourth."
        );
    }

    #[test]
    fn an_excerpt_keeps_two_hundred_scalar_values_without_an_ellipsis() {
        let excerpt = description_excerpt(&"é".repeat(250));

        assert_eq!(excerpt.chars().count(), 200);
        assert!(!excerpt.ends_with('…'));
        assert!(!excerpt.ends_with("..."));
    }

    #[test]
    fn an_excerpt_cut_in_a_gap_does_not_end_in_whitespace() {
        let description = format!("{}   tail", "a".repeat(199));

        assert_eq!(description_excerpt(&description), "a".repeat(199));
    }

    #[test]
    fn a_short_description_is_its_own_excerpt_and_an_empty_one_stays_empty() {
        assert_eq!(description_excerpt("Wire the tracker"), "Wire the tracker");
        assert_eq!(description_excerpt("   \n  "), "");
    }
}
