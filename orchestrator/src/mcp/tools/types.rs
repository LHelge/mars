//! The input and output shapes of the twelve tools, and the small validations
//! that need neither the database nor the git repository.
//!
//! `SPEC.md`, "MCP tool contracts" gives every tool an `Input` and an `Output`
//! object; the structs here are those objects, deserialised by `serde` and
//! described to the agent by `schemars`. Nothing here talks to a repository:
//! a handler takes one of these, turns a [`TaskArg`] into a
//! [`TaskRef`] and hands the rest to the tracker or the git service.
//!
//! **Outputs re-use the REST DTOs.** `Task`, `TaskDetail`, `Comment` and
//! `SessionBranch` are one wire shape each, not two: the MCP wrappers hold
//! [`TaskDto`], [`TaskDetailDto`], [`CommentDto`], [`TaskSummary`] and
//! [`SessionBranch`] themselves, so a field added to the REST shape cannot
//! fail to reach the agent.
//!
//! **Unknown input fields are ignored.** No `deny_unknown_fields` anywhere: a
//! model that adds a plausible-looking extra argument gets its call executed
//! rather than a schema lecture, which is the same leniency the REST API
//! offers.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::mcp::{McpError, McpResult};
use crate::models::{HandoffCaller, HandoffInput, SessionBranch, TaskRef, ValidatedHandoff};
use crate::tracker::{CommentDto, TaskDetailDto, TaskDto, TaskSummary};
// The crate convention (`CLAUDE.md`, "Backend conventions").
#[allow(unused_imports)]
use crate::prelude::*;

/// The rejection every malformed `task` argument gets (`SPEC.md`, "MCP tool
/// contracts": "Every `task` argument accepts a task's UUID or its
/// per-project number (as a number or a string such as `"12"` or `"#12"`)").
pub const TASK_ARG_MESSAGE: &str = r##"task must be a UUID, a number, or "#<number>""##;

/// The default `ready` limit, and the range an explicit one must be in.
const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 100;

/// The rejection an out-of-range `limit` gets.
pub const LIMIT_MESSAGE: &str = "limit must be between 1 and 100";

/// The rejection an out-of-range `priority` gets; the same text the tracker
/// model uses, because it is the same rule (`docs/data-model.md`, `tasks`).
pub const PRIORITY_MESSAGE: &str = "priority must be between 0 (critical) and 3 (low)";

/// A task, as an agent may name it.
///
/// `SPEC.md`: "Every `task` argument accepts a task's UUID or its per-project
/// number (as a number or a string such as `"12"` or `"#12"`)." The `#` is
/// display syntax the board shows, so a model that copied a task id out of a
/// comment is understood rather than corrected.
///
/// Untagged, so the JSON is a bare number or a bare string. Two kinds of input
/// therefore never reach [`TaskArg::parse`] at all: a negative number and a
/// number with a fractional part or an exponent (`-1`, `12.5`, `12.0`) fit
/// neither arm, and `serde` fails the *whole* argument object with its own
/// unhelpful "data did not match any variant" text. The dispatcher turns every
/// deserialisation failure into `invalid_argument`, and for a `task` field
/// that message must be [`TASK_ARG_MESSAGE`], so that the two ways of writing
/// a bad task reference are answered the same way.
///
/// Whitespace is not trimmed: `" 12"` is not a task reference.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TaskArg {
    /// A bare JSON number: a per-project task number.
    Number(u64),
    /// A string: a UUID, a run of digits, or `#` and a run of digits.
    Text(String),
}

impl TaskArg {
    /// The reference a repository can resolve, or `invalid_argument`.
    ///
    /// `0` is syntactically valid and is not rejected here: task numbers start
    /// at 1, so it resolves to `not_found` at the repository, which is the
    /// honest answer for a number that names no task.
    pub fn parse(&self) -> McpResult<TaskRef> {
        match self {
            TaskArg::Number(number) => i32::try_from(*number)
                .map(TaskRef::Number)
                .map_err(|_| invalid_task()),
            // A `#` prefix is display syntax for a *number*, so `#<uuid>` is
            // not a task reference either.
            TaskArg::Text(text) => match text.strip_prefix('#') {
                Some(digits) => parse_number(digits),
                None => text.parse::<TaskRef>().map_err(|_| invalid_task()),
            },
        }
    }
}

/// `#<digits>`, once the `#` is off: non-empty, all ASCII digits, and inside
/// the column's `INTEGER`.
fn parse_number(digits: &str) -> McpResult<TaskRef> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_task());
    }

    digits
        .parse::<i32>()
        .map(TaskRef::Number)
        .map_err(|_| invalid_task())
}

fn invalid_task() -> McpError {
    McpError::invalid_argument(TASK_ARG_MESSAGE)
}

/// Absent and `null` are different answers.
///
/// `update`'s `parent` is the one field where they are: absent leaves the
/// parent alone, `null` detaches the task from it. Everything else in the
/// tracker treats an absent field as "no change" and has no null form.
fn deserialize_double_option<'de, D, T>(
    deserializer: D,
) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

// ---- inputs ----

/// `ready`: `{ limit?: number = 20 }`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ReadyInput {
    /// 1 through 100 inclusive; 20 when absent. Out-of-range values are
    /// refused, never clamped (`SPEC.md`, "MCP tool contracts" → `ready`).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `claim` and `get_task`: `{ task }` and nothing else.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TaskOnlyInput {
    /// The task's UUID or per-project number.
    pub task: TaskArg,
}

/// `update`: every field of a task an agent may change, plus the hand-off.
///
/// Every field but `task` is optional and absent means "leave it alone"; the
/// tri-state `parent` is the one exception, where `null` means "detach".
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct UpdateInput {
    /// The task the caller holds.
    pub task: TaskArg,
    /// A state name from the project's own states.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// 0 (critical) to 3 (low).
    #[serde(default)]
    pub priority: Option<i16>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    /// Absent leaves the parent; `null` detaches; a reference re-parents.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub parent: Option<Option<TaskArg>>,
    /// New `blocks` dependencies.
    #[serde(default)]
    pub add_depends_on: Option<Vec<TaskArg>>,
    /// `blocks` dependencies to remove; other edge kinds are left alone.
    #[serde(default)]
    pub remove_depends_on: Option<Vec<TaskArg>>,
    /// A revision publication or a forward (`SPEC.md`, "Code hand-offs and
    /// review"). Validated through [`validate_handoff`], which supplies the
    /// calling session.
    #[serde(default)]
    pub handoff: Option<HandoffInput>,
}

/// `release`: `{ task, reason }`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReleaseInput {
    pub task: TaskArg,
    /// Recorded as a comment.
    pub reason: String,
}

/// `comment`: `{ task, body }`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CommentInput {
    pub task: TaskArg,
    pub body: String,
}

/// `needs_human`: `{ task, reason }`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct NeedsHumanInput {
    pub task: TaskArg,
    /// Stored as `needs_human_reason` and recorded as a comment.
    pub reason: String,
}

/// `create_task`: the new task and its links.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CreateTaskInput {
    /// 1 to 200 characters (`docs/data-model.md`, `tasks`).
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The project's default state — its first `queue` state — when absent.
    #[serde(default)]
    pub state: Option<String>,
    /// 0 (critical) to 3 (low); 2 when absent.
    #[serde(default)]
    pub priority: Option<i16>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    #[serde(default)]
    pub parent: Option<TaskArg>,
    /// `blocks` dependencies on existing tasks.
    #[serde(default)]
    pub depends_on: Option<Vec<TaskArg>>,
    /// The held task this work was discovered from; required only when the
    /// caller holds more than one.
    #[serde(default)]
    pub discovered_from: Option<TaskArg>,
}

/// `list_session_branches`: `{}`.
///
/// A struct rather than nothing, so the tool still advertises an object schema
/// and a caller that sends `{}` — or sends fields nobody asked for —
/// deserialises cleanly.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct EmptyInput {}

/// `merge`: the REST `MergeInput`, except that `task_id` also accepts a
/// per-project task number (`SPEC.md`, "MCP tool contracts" → `merge`).
///
/// Flat, like the REST body: the two forms — `{ source }` and
/// `{ task_id, handoff_id }` — are mutually exclusive *selections* rather than
/// two shapes, and the git tools task decides between them with the
/// documented message.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct McpMergeInput {
    /// The integration head to merge into.
    pub target: String,
    /// The merge commit message; the bot's default when absent.
    #[serde(default)]
    pub message: Option<String>,
    /// The branch form.
    #[serde(default)]
    pub source: Option<String>,
    /// The task form, first half. Unlike REST, a task *number* is accepted.
    #[serde(default)]
    pub task_id: Option<TaskArg>,
    /// The task form, second half: the hand-off whose pinned commit is merged.
    #[serde(default)]
    pub handoff_id: Option<Uuid>,
}

/// `rebase`: `{ branch, onto }`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RebaseInput {
    /// An integration head or a session ref; never an upstream-tracking name.
    pub branch: String,
    /// An integration head or an upstream-tracking name.
    pub onto: String,
}

/// `push`: `{ ref, remote_branch?, force?: boolean = false }`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PushInput {
    /// An integration head or a session ref. `ref` is a Rust keyword and a
    /// field name on the wire.
    pub r#ref: String,
    /// The upstream branch to update; the source's own name when absent.
    #[serde(default)]
    pub remote_branch: Option<String>,
    /// Absent means `false`; a force push additionally requires `push` in the
    /// profile's `mcp_tools`.
    #[serde(default)]
    pub force: Option<bool>,
}

// ---- outputs ----

/// `ready` → `{ tasks: TaskSummary[] }`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReadyOutput {
    pub tasks: Vec<TaskSummary>,
}

/// `claim`, `update`, `release`, `needs_human`, `create_task` →
/// `{ task: Task }`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TaskOutput {
    pub task: TaskDto,
}

/// `get_task` → `{ task: TaskDetail }`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TaskDetailOutput {
    pub task: TaskDetailDto,
}

/// `comment` → `{ comment: Comment }`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommentOutput {
    pub comment: CommentDto,
}

/// `list_session_branches` → `{ branches: SessionBranch[] }`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BranchesOutput {
    pub branches: Vec<SessionBranch>,
}

/// `merge` and `rebase` → `{ commit }`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommitOutput {
    pub commit: String,
}

/// `push` → `{ remote_branch, commit }`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PushOutput {
    pub remote_branch: String,
    pub commit: String,
}

// ---- shared validation ----

/// 0 (critical) through 3 (low), or `invalid_argument`.
pub fn validate_priority(priority: i16) -> McpResult<i16> {
    match priority {
        0..=3 => Ok(priority),
        _ => Err(McpError::invalid_argument(PRIORITY_MESSAGE)),
    }
}

/// `ready`'s limit: 20 when absent, otherwise 1 through 100 inclusive.
///
/// "An explicit `limit` must be an integer from 1 through 100 inclusive; other
/// values return `invalid_argument` and are never clamped." The `i64` is what
/// the repository's `LIMIT` binds.
pub fn validate_limit(limit: Option<u32>) -> McpResult<i64> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);

    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(McpError::invalid_argument(LIMIT_MESSAGE));
    }

    Ok(i64::from(limit))
}

/// A required text argument that is not whitespace.
///
/// The tracker trims a comment body before storing it, so a body of spaces is
/// an empty one and is refused here rather than stored blank.
pub fn non_empty(field: &str, value: &str) -> McpResult<()> {
    if value.trim().is_empty() {
        return Err(McpError::invalid_argument(format!(
            "{field} must not be empty"
        )));
    }

    Ok(())
}

/// The hand-off rules that need neither the database nor the git repository,
/// for a caller that is a session.
///
/// The rules and their messages live in the model
/// ([`HandoffInput::validate`]), because `SPEC.md` says the MCP `update` tool
/// returns "the exact 400 and 409 messages" of "Code hand-offs and review":
/// an empty comment, a commit that is not a full object id and a
/// `source_session_id` an MCP caller must not supply are all refused there, in
/// the same words the REST route uses. This is the seam that supplies the
/// calling session and widens the rejection into `invalid_argument`.
pub fn validate_handoff(handoff: &HandoffInput, session_id: Uuid) -> McpResult<ValidatedHandoff> {
    handoff
        .validate(&HandoffCaller::Session { session_id })
        .map_err(|err| McpError::from(Error::from(err)))
}

#[cfg(test)]
mod tests {
    use schemars::schema_for;
    use serde_json::{Value, json};

    use super::*;

    fn task_arg(value: Value) -> McpResult<TaskRef> {
        serde_json::from_value::<TaskArg>(value)
            .expect("the value is a number or a string")
            .parse()
    }

    #[test]
    fn a_task_is_named_by_uuid_by_number_or_by_hash_number() {
        let id = Uuid::new_v4();

        for (input, expected) in [
            (json!(12), TaskRef::Number(12)),
            (json!(0), TaskRef::Number(0)),
            (json!(i32::MAX), TaskRef::Number(i32::MAX)),
            (json!("12"), TaskRef::Number(12)),
            (json!("#12"), TaskRef::Number(12)),
            (json!("007"), TaskRef::Number(7)),
            (json!("#007"), TaskRef::Number(7)),
            (json!(id.to_string()), TaskRef::Id(id)),
        ] {
            assert_eq!(task_arg(input.clone()), Ok(expected), "{input}");
        }
    }

    #[test]
    fn everything_else_is_the_one_documented_rejection() {
        let id = Uuid::new_v4();

        for input in [
            json!(""),
            json!("#"),
            json!("##12"),
            json!("abc"),
            json!("12abc"),
            json!("-12"),
            json!(" 12"),
            json!("12 "),
            json!("#12 "),
            // A `#` is display syntax for a number, never for a UUID.
            json!(format!("#{id}")),
            // Above `INTEGER`, as a number and as a string.
            json!(u64::from(u32::MAX) + 1),
            json!((i64::from(i32::MAX) + 1).to_string()),
        ] {
            assert_eq!(
                task_arg(input.clone()),
                Err(McpError::invalid_argument(TASK_ARG_MESSAGE)),
                "{input}",
            );
        }
    }

    #[test]
    fn a_negative_or_fractional_number_fails_deserialisation_rather_than_parsing() {
        // The edge case the dispatcher has to translate: neither arm of the
        // untagged enum accepts these, so `serde` rejects the whole argument
        // object with its own text and `parse` never runs.
        for input in [
            json!(-1),
            json!(12.5),
            json!(12.0),
            json!(null),
            json!([12]),
        ] {
            assert!(
                serde_json::from_value::<TaskArg>(input.clone()).is_err(),
                "{input}",
            );
        }
    }

    #[test]
    fn an_absent_parent_a_null_parent_and_a_named_parent_are_three_answers() {
        let absent: UpdateInput = serde_json::from_value(json!({ "task": 1 })).unwrap();
        assert_eq!(absent.parent, None);

        let detach: UpdateInput =
            serde_json::from_value(json!({ "task": 1, "parent": null })).unwrap();
        assert_eq!(detach.parent, Some(None));

        let reparent: UpdateInput =
            serde_json::from_value(json!({ "task": 1, "parent": "#3" })).unwrap();
        assert_eq!(
            reparent.parent.unwrap().unwrap().parse(),
            Ok(TaskRef::Number(3)),
        );
    }

    #[test]
    fn an_update_takes_every_documented_field_and_ignores_the_rest() {
        let input: UpdateInput = serde_json::from_value(json!({
            "task": "#12",
            "state": "review",
            "title": "Wire the tracker",
            "description": "…",
            "priority": 1,
            "labels": ["backend"],
            "parent": 3,
            "add_depends_on": [4, "#5"],
            "remove_depends_on": ["6"],
            "invented_by_the_model": true,
        }))
        .unwrap();

        assert_eq!(input.task.parse(), Ok(TaskRef::Number(12)));
        assert_eq!(input.state.as_deref(), Some("review"));
        assert_eq!(input.priority, Some(1));
        assert_eq!(input.labels.as_deref(), Some(&["backend".to_string()][..]));
        assert_eq!(input.add_depends_on.unwrap().len(), 2);
        assert_eq!(input.remove_depends_on.unwrap().len(), 1);
        assert!(input.handoff.is_none());
    }

    #[test]
    fn a_handoff_is_tagged_by_kind_and_validated_against_the_calling_session() {
        /// An obviously fake but well-formed object id (`CLAUDE.md`, rule 3).
        const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
        let session_id = Uuid::new_v4();

        let input: UpdateInput = serde_json::from_value(json!({
            "task": 12,
            "state": "review",
            "handoff": {
                "kind": "revision",
                "commit": COMMIT,
                "comment": "Implemented and tested.",
            },
        }))
        .unwrap();

        assert_eq!(
            validate_handoff(&input.handoff.unwrap(), session_id),
            Ok(ValidatedHandoff::Revision {
                // Derived from the caller, never from the input.
                source_session_id: session_id,
                commit: COMMIT.to_string(),
                comment: "Implemented and tested.".to_string(),
            }),
        );

        let handoff_id = Uuid::new_v4();
        let forward: HandoffInput = serde_json::from_value(json!({
            "kind": "forward",
            "handoff_id": handoff_id,
            "comment": "Looks right.",
            "review": "approved",
        }))
        .unwrap();

        assert_eq!(
            validate_handoff(&forward, session_id),
            Ok(ValidatedHandoff::Forward {
                handoff_id,
                comment: "Looks right.".to_string(),
                review: Some(crate::models::ReviewDecision::Approved),
            }),
        );
    }

    #[test]
    fn a_handoff_is_rejected_with_the_documented_messages() {
        const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
        let session_id = Uuid::new_v4();

        let cases = [
            (
                json!({
                    "kind": "revision",
                    "source_session_id": Uuid::new_v4(),
                    "commit": COMMIT,
                    "comment": "Implemented.",
                }),
                "source_session_id is derived from the calling session",
            ),
            (
                json!({ "kind": "revision", "commit": COMMIT, "comment": "  " }),
                "comment body must not be empty",
            ),
            (
                json!({ "kind": "revision", "commit": "abc", "comment": "Implemented." }),
                "commit must be a full lowercase hexadecimal git object id",
            ),
            (
                json!({
                    "kind": "revision",
                    "commit": COMMIT,
                    "comment": "Implemented.",
                    "review": "approved",
                }),
                "review applies to forward hand-offs only",
            ),
        ];

        for (body, message) in cases {
            let handoff: HandoffInput = serde_json::from_value(body.clone()).unwrap();
            assert_eq!(
                validate_handoff(&handoff, session_id),
                Err(McpError::invalid_argument(message)),
                "{body}",
            );
        }
    }

    #[test]
    fn a_priority_outside_the_four_values_is_refused() {
        for priority in 0..=3 {
            assert_eq!(validate_priority(priority), Ok(priority));
        }
        for priority in [-1, 4, i16::MAX] {
            assert_eq!(
                validate_priority(priority),
                Err(McpError::invalid_argument(PRIORITY_MESSAGE)),
            );
        }
    }

    #[test]
    fn a_limit_defaults_to_twenty_and_is_never_clamped() {
        assert_eq!(validate_limit(None), Ok(20));
        assert_eq!(validate_limit(Some(1)), Ok(1));
        assert_eq!(validate_limit(Some(100)), Ok(100));

        for limit in [0, 101, u32::MAX] {
            assert_eq!(
                validate_limit(Some(limit)),
                Err(McpError::invalid_argument(LIMIT_MESSAGE)),
                "{limit}",
            );
        }
    }

    #[test]
    fn a_required_text_argument_names_itself_when_it_is_blank() {
        assert_eq!(non_empty("reason", "blocked on credentials"), Ok(()));
        assert_eq!(
            non_empty("reason", " \n "),
            Err(McpError::invalid_argument("reason must not be empty")),
        );
        assert_eq!(
            non_empty("body", ""),
            Err(McpError::invalid_argument("body must not be empty")),
        );
    }

    /// A property's schema with every `$defs` reference it makes inlined once,
    /// as text: `schemars` describes a named type by reference, so the arms of
    /// [`TaskArg`] live in `$defs` rather than under the property.
    fn property_schema(schema: &Value, property: &str) -> String {
        let node = schema["properties"][property].to_string();
        let defs = &schema["$defs"];
        let mut text = node.clone();

        for (name, definition) in defs.as_object().into_iter().flatten() {
            if node.contains(&format!("#/$defs/{name}")) {
                text.push_str(&definition.to_string());
            }
        }

        text
    }

    /// Does this property accept a number and a string alike?
    fn accepts_number_and_string(schema: &Value, property: &str) -> bool {
        let text = property_schema(schema, property);
        text.contains("integer") && text.contains("\"type\":\"string\"")
    }

    #[test]
    fn every_input_type_produces_a_schema() {
        for schema in [
            serde_json::to_value(schema_for!(ReadyInput)).unwrap(),
            serde_json::to_value(schema_for!(TaskOnlyInput)).unwrap(),
            serde_json::to_value(schema_for!(UpdateInput)).unwrap(),
            serde_json::to_value(schema_for!(ReleaseInput)).unwrap(),
            serde_json::to_value(schema_for!(CommentInput)).unwrap(),
            serde_json::to_value(schema_for!(NeedsHumanInput)).unwrap(),
            serde_json::to_value(schema_for!(CreateTaskInput)).unwrap(),
            serde_json::to_value(schema_for!(EmptyInput)).unwrap(),
            serde_json::to_value(schema_for!(McpMergeInput)).unwrap(),
            serde_json::to_value(schema_for!(RebaseInput)).unwrap(),
            serde_json::to_value(schema_for!(PushInput)).unwrap(),
        ] {
            assert_eq!(schema["type"], json!("object"), "{schema}");
        }
    }

    #[test]
    fn a_task_argument_is_described_as_a_number_or_a_string() {
        let schema = serde_json::to_value(schema_for!(TaskOnlyInput)).unwrap();
        assert!(accepts_number_and_string(&schema, "task"), "{schema}");
        assert_eq!(schema["required"], json!(["task"]));

        let update = serde_json::to_value(schema_for!(UpdateInput)).unwrap();
        assert!(accepts_number_and_string(&update, "task"), "{update}");
        assert!(accepts_number_and_string(&update, "parent"), "{update}");
    }

    #[test]
    fn push_describes_its_ref_by_its_wire_name() {
        let schema = serde_json::to_value(schema_for!(PushInput)).unwrap();

        assert!(schema["properties"]["ref"].is_object(), "{schema}");
        assert_eq!(schema["required"], json!(["ref"]));
    }

    #[test]
    fn every_output_type_produces_a_schema() {
        for schema in [
            serde_json::to_value(schema_for!(ReadyOutput)).unwrap(),
            serde_json::to_value(schema_for!(TaskOutput)).unwrap(),
            serde_json::to_value(schema_for!(TaskDetailOutput)).unwrap(),
            serde_json::to_value(schema_for!(CommentOutput)).unwrap(),
            serde_json::to_value(schema_for!(BranchesOutput)).unwrap(),
            serde_json::to_value(schema_for!(CommitOutput)).unwrap(),
            serde_json::to_value(schema_for!(PushOutput)).unwrap(),
        ] {
            assert_eq!(schema["type"], json!("object"), "{schema}");
        }
    }

    #[test]
    fn a_merge_takes_either_form_and_a_task_number_in_the_task_form() {
        let branch: McpMergeInput =
            serde_json::from_value(json!({ "target": "main", "source": "origin/main" })).unwrap();
        assert_eq!(branch.source.as_deref(), Some("origin/main"));
        assert!(branch.task_id.is_none());

        let handoff_id = Uuid::new_v4();
        let task: McpMergeInput = serde_json::from_value(json!({
            "target": "main",
            "task_id": 12,
            "handoff_id": handoff_id,
        }))
        .unwrap();
        assert_eq!(task.task_id.unwrap().parse(), Ok(TaskRef::Number(12)));
        assert_eq!(task.handoff_id, Some(handoff_id));
    }

    #[test]
    fn a_push_reads_its_ref_and_defaults_force_to_absent() {
        let input: PushInput =
            serde_json::from_value(json!({ "ref": "refs/sessions/one" })).unwrap();

        assert_eq!(input.r#ref, "refs/sessions/one");
        assert_eq!(input.remote_branch, None);
        assert_eq!(input.force, None);
    }
}
