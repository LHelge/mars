//! The tool contract as data — names, descriptions and input/output shapes —
//! the thirteen `tools/list` entries built from it, and the dispatch table
//! `tools/call` goes through.
//!
//! [`names`], [`descriptions`] and [`types`] are pure: no state, no database,
//! no git. [`catalog`] turns them into the [`Tool`] values the listing sends,
//! once per process. [`dispatch`] is the one `match` from a [`ToolName`] to a
//! handler module, and every handler lives in a file of its own so the tool
//! tasks that fill them in never touch the same lines.
//!
//! Keeping the contract in one place is what lets `tests/mcp_descriptions.rs`
//! compare it with the document byte for byte.

pub mod descriptions;
pub mod names;
pub mod types;

pub mod claim;
pub mod comment;
pub mod common;
pub mod create_plan;
pub mod create_task;
pub mod get_task;
pub mod list_session_branches;
pub mod merge;
pub mod needs_human;
pub mod push;
pub mod ready;
pub mod rebase;
pub mod release;
pub mod update;

use std::sync::{Arc, OnceLock};

use rmcp::handler::server::common::{schema_for_empty_input, schema_for_input, schema_for_output};
use rmcp::model::{JsonObject, Tool};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::mcp::{McpError, McpResult, SessionContext};
use crate::prelude::*;

pub use names::ToolName;
pub use types::{
    BranchesOutput, CommentInput, CommentOutput, CommitOutput, CreatePlanInput, CreateTaskInput,
    EmptyInput, McpMergeInput, NeedsHumanInput, PlanOutput, PlanParentInput, PlanTaskInput,
    PushInput, PushOutput, ReadyInput, ReadyOutput, RebaseInput, ReleaseInput, TASK_ARG_MESSAGE,
    TaskArg, TaskDetailOutput, TaskOnlyInput, TaskOutput, UpdateInput, non_empty, validate_handoff,
    validate_limit, validate_priority,
};

/// The thirteen `tools/list` entries, in [`ToolName::ALL`] order, built once.
///
/// Schema generation walks the type with `schemars` and allocates; a listing
/// happens on every `tools/list` of every MCP session, so the thirteen [`Tool`]
/// values are built once and cloned afterwards — a clone is a `Cow<'static>`
/// name, a `Cow<'static>` description and two `Arc`s.
///
/// The profile gate is *not* applied here: the listing is filtered per request
/// by the caller, because two sessions of the same process can have different
/// `mcp_tools`.
pub fn catalog() -> &'static [Tool] {
    static CATALOG: OnceLock<Vec<Tool>> = OnceLock::new();

    CATALOG.get_or_init(|| ToolName::ALL.into_iter().map(tool).collect())
}

/// One [`Tool`], with the schema of its own input and output type.
fn tool(name: ToolName) -> Tool {
    let (input, output) = match name {
        ToolName::Ready => schemas::<ReadyInput, ReadyOutput>(),
        ToolName::Claim => schemas::<TaskOnlyInput, TaskOutput>(),
        ToolName::GetTask => schemas::<TaskOnlyInput, TaskDetailOutput>(),
        ToolName::Update => schemas::<UpdateInput, TaskOutput>(),
        ToolName::Release => schemas::<ReleaseInput, TaskOutput>(),
        ToolName::Comment => schemas::<CommentInput, CommentOutput>(),
        ToolName::NeedsHuman => schemas::<NeedsHumanInput, TaskOutput>(),
        ToolName::CreateTask => schemas::<CreateTaskInput, TaskOutput>(),
        ToolName::CreatePlan => schemas::<CreatePlanInput, PlanOutput>(),
        ToolName::ListSessionBranches => schemas::<EmptyInput, BranchesOutput>(),
        ToolName::Merge => schemas::<McpMergeInput, CommitOutput>(),
        ToolName::Rebase => schemas::<RebaseInput, CommitOutput>(),
        ToolName::Push => schemas::<PushInput, PushOutput>(),
    };

    Tool::new(name.as_str(), name.description(), input).with_raw_output_schema(output)
}

/// The input and output schema of one tool.
///
/// `rmcp`'s own generators are used rather than `schemars::schema_for!`
/// directly, so the listing carries exactly what the macro-generated servers
/// carry: draft 2020-12, with the wrapper type's own `title` and `description`
/// stripped (they are the Rust type name and its doc comment, which are noise
/// to the model).
///
/// `schema_for_input` refuses a schema whose root is not an object. Every
/// input type here is a struct, so that cannot happen — `every_tool_has_an_object_input_schema`
/// is what fails if one ever stops being one — and the fallback is the empty
/// object schema rather than a panic, because this runs while answering a
/// request.
fn schemas<I, O>() -> (Arc<JsonObject>, Arc<JsonObject>)
where
    I: schemars::JsonSchema + std::any::Any,
    O: schemars::JsonSchema + std::any::Any,
{
    (
        schema_for_input::<I>().unwrap_or_else(|_| schema_for_empty_input()),
        schema_for_output::<O>(),
    )
}

/// Route one authorised call to its handler.
///
/// The caller has already resolved the [`SessionContext`], parsed the name and
/// checked it against the profile; what is left is to turn `args` into the
/// tool's input type and to turn the handler's output back into JSON.
///
/// The handlers take no session id: they read it from `ctx`, so a caller
/// cannot name a session it does not hold the token for (`ARCHITECTURE.md`,
/// "MCP design" → Authentication).
pub async fn dispatch(
    state: &AppState,
    ctx: &SessionContext,
    tool: ToolName,
    args: Value,
) -> McpResult<Value> {
    match tool {
        ToolName::Ready => output(ready::handle(state, ctx, input(args)?).await?),
        ToolName::Claim => output(claim::handle(state, ctx, input(args)?).await?),
        ToolName::GetTask => output(get_task::handle(state, ctx, input(args)?).await?),
        ToolName::Update => output(update::handle(state, ctx, input(args)?).await?),
        ToolName::Release => output(release::handle(state, ctx, input(args)?).await?),
        ToolName::Comment => output(comment::handle(state, ctx, input(args)?).await?),
        ToolName::NeedsHuman => output(needs_human::handle(state, ctx, input(args)?).await?),
        ToolName::CreateTask => output(create_task::handle(state, ctx, input(args)?).await?),
        ToolName::CreatePlan => output(create_plan::handle(state, ctx, input(args)?).await?),
        ToolName::ListSessionBranches => {
            output(list_session_branches::handle(state, ctx, input(args)?).await?)
        }
        ToolName::Merge => output(merge::handle(state, ctx, input(args)?).await?),
        ToolName::Rebase => output(rebase::handle(state, ctx, input(args)?).await?),
        ToolName::Push => output(push::handle(state, ctx, input(args)?).await?),
    }
}

/// The arguments object as the tool's input type, or `invalid_argument`.
///
/// Two shapes of rejection, because two things can be wrong. A malformed
/// `task` — `12.5`, `-1`, `null` — fails the untagged [`TaskArg`] before
/// [`TaskArg::parse`] ever runs, and `serde`'s "data did not match any variant"
/// text would send the model off guessing; that one is answered with
/// [`TASK_ARG_MESSAGE`], the same sentence a syntactically valid but
/// unresolvable reference gets. Everything else keeps `serde`'s own message,
/// which is the more useful answer for a missing required argument ("missing
/// field `task`") or a wrong type ("invalid type: string \"twenty\", expected
/// u32" — `serde_json::from_value` reports the type it wanted rather than the
/// field it wanted it in).
fn input<T: DeserializeOwned>(args: Value) -> McpResult<T> {
    serde_json::from_value(args).map_err(|err| {
        let message = err.to_string();

        if message.contains(TASK_ARG_VARIANT) {
            McpError::invalid_argument(TASK_ARG_MESSAGE)
        } else {
            McpError::invalid_argument(format!("invalid arguments: {message}"))
        }
    })
}

/// What `serde` puts in the message when no arm of the untagged [`TaskArg`]
/// matched. `a_malformed_task_argument_is_answered_with_the_documented_message`
/// is what fails if a `serde` release rewords it.
const TASK_ARG_VARIANT: &str = "untagged enum TaskArg";

/// A handler's output as JSON.
///
/// Serialising an owned struct of `String`s, numbers and DTOs cannot fail in
/// practice; if it somehow does, the agent gets the generic `internal` answer
/// and the reason goes to the log (`CLAUDE.md` rule 3).
fn output<T: Serialize>(value: T) -> McpResult<Value> {
    serde_json::to_value(value).map_err(|err| {
        error!(error = ?err, "tool output did not serialise");
        McpError::internal()
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_catalog_is_the_thirteen_tools_in_the_documented_order() {
        let names: Vec<_> = catalog().iter().map(|tool| tool.name.as_ref()).collect();

        assert_eq!(names, ToolName::ALL.map(ToolName::as_str));
    }

    #[test]
    fn every_tool_carries_its_verbatim_description_and_both_schemas() {
        for (tool, name) in catalog().iter().zip(ToolName::ALL) {
            assert_eq!(tool.description.as_deref(), Some(name.description()));
            assert_eq!(
                tool.input_schema.get("type"),
                Some(&json!("object")),
                "{}",
                name.as_str(),
            );
            assert!(tool.output_schema.is_some(), "{}", name.as_str());
        }
    }

    #[test]
    fn every_tool_has_an_object_input_schema() {
        // The empty-object fallback in `schemas` is unreachable exactly as
        // long as this passes: `schema_for_input` fails only on a root that is
        // not an object.
        assert!(
            [
                schema_for_input::<ReadyInput>(),
                schema_for_input::<TaskOnlyInput>(),
                schema_for_input::<UpdateInput>(),
                schema_for_input::<ReleaseInput>(),
                schema_for_input::<CommentInput>(),
                schema_for_input::<NeedsHumanInput>(),
                schema_for_input::<CreateTaskInput>(),
                schema_for_input::<CreatePlanInput>(),
                schema_for_input::<EmptyInput>(),
                schema_for_input::<McpMergeInput>(),
                schema_for_input::<RebaseInput>(),
                schema_for_input::<PushInput>(),
            ]
            .iter()
            .all(|schema| schema.is_ok()),
        );
    }

    #[test]
    fn a_schema_describes_the_tools_own_arguments() {
        let claim = &catalog()[1];
        assert_eq!(claim.name.as_ref(), "claim");
        assert_eq!(claim.input_schema["required"], json!(["task"]));

        // `list_session_branches` takes nothing, and still advertises an
        // object.
        let branches = &catalog()[9];
        assert_eq!(branches.name.as_ref(), "list_session_branches");
        assert_eq!(branches.input_schema["type"], json!("object"));
    }

    #[test]
    fn the_listing_is_built_once_and_handed_out_by_reference() {
        assert!(std::ptr::eq(catalog(), catalog()));
    }

    #[test]
    fn absent_and_empty_arguments_are_the_same_thing_for_a_tool_with_no_required_field() {
        let input: ReadyInput = input(json!({})).expect("an empty object is a valid ready input");

        assert_eq!(input.limit, None);
    }

    #[test]
    fn a_malformed_task_argument_is_answered_with_the_documented_message() {
        for args in [json!({ "task": 12.5 }), json!({ "task": -1 })] {
            assert_eq!(
                input::<TaskOnlyInput>(args.clone()).err(),
                Some(McpError::invalid_argument(TASK_ARG_MESSAGE)),
                "{args}",
            );
        }
    }

    #[test]
    fn any_other_bad_argument_keeps_serdes_own_explanation() {
        let err =
            input::<ReadyInput>(json!({ "limit": "twenty" })).expect_err("a string is not a limit");

        assert_eq!(err.code, crate::mcp::McpErrorCode::InvalidArgument);
        assert_eq!(
            err.message,
            "invalid arguments: invalid type: string \"twenty\", expected u32",
        );

        let missing = input::<TaskOnlyInput>(json!({})).expect_err("task is required");
        assert_eq!(missing.message, "invalid arguments: missing field `task`");
    }

    #[test]
    fn an_argument_body_that_is_not_an_object_is_an_invalid_argument() {
        for args in [json!([1, 2]), json!("ready"), json!(null)] {
            let err = input::<ReadyInput>(args.clone())
                .expect_err("only an object is a valid argument body");

            assert_eq!(err.code, crate::mcp::McpErrorCode::InvalidArgument);
            assert!(err.message.starts_with("invalid arguments: "), "{args}");
        }
    }
}
