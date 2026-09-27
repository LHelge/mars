//! `create_plan`: file a parent, its sub-tasks and their edges in one change
//! (`SPEC.md`, "MCP tool contracts" → `create_plan`; ADR 0056).
//!
//! The batch form of `create_task`, and like it a thin transport over the
//! tracker: every rule of a single task is
//! [`create_task`](crate::tracker::create_task)'s and every rule of the batch —
//! local refs, creation order, one provenance, the bound — is
//! [`tracker::plan`](crate::tracker::plan)'s. What this handler owes is the
//! same three jobs as `create_task`'s:
//!
//! 1. the rejections that need no database — a title, priority or label that
//!    is not one, a task reference that is not one, both parents at once, and
//!    everything [`validate_plan`] decides (the bound, the refs, a cycle) —
//!    refused *before* the project lock is taken;
//! 2. reading a `depends_on` entry: the `ref` of a sub-task of this plan when
//!    it is one, a task reference otherwise, and the documented refusal when
//!    it is neither;
//! 3. the creator, from the [`SessionContext`] and never from an argument.
//!
//! The whole plan runs inside one [`TrackerMutation`](crate::tracker::TrackerMutation):
//! any failure rolls back every task, edge, event and number it had written.

use std::collections::HashSet;

use crate::mcp::tools::common::{begin_mutation, finish};
use crate::mcp::tools::{
    CreatePlanInput, PlanOutput, PlanParentInput, PlanTaskInput, TaskArg, validate_priority,
};
use crate::mcp::{McpError, McpResult, SessionContext};
use crate::models::{Label, TaskTitle};
use crate::prelude::*;
use crate::tracker::plan::{
    PlanDependency, PlanInput, PlanParent, PlanTask, PlanTaskFields, create_plan, unknown_ref,
    validate_plan,
};

/// What naming both kinds of parent is told.
pub const BOTH_PARENTS: &str = "pass parent or new_parent, not both";

/// `{ parent?, new_parent?, tasks, discovered_from? }` →
/// `{ parent: Task | null, tasks: Task[], refs }`.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: CreatePlanInput,
) -> McpResult<PlanOutput> {
    let plan = validate(input, ctx)?;

    let mut mutation = begin_mutation(state, ctx).await?;
    let created = create_plan(&mut mutation, plan)
        .await
        .map_err(McpError::from);
    let outcome = finish(state, mutation, created).await?;

    Ok(PlanOutput {
        parent: outcome.parent,
        tasks: outcome.tasks,
        refs: outcome.refs,
    })
}

/// The arguments as the tracker's [`PlanInput`], with every rejection that
/// needs neither the lock nor the database already made.
fn validate(input: CreatePlanInput, ctx: &SessionContext) -> McpResult<PlanInput> {
    let parent = match (input.parent, input.new_parent) {
        (Some(_), Some(_)) => return Err(McpError::invalid_argument(BOTH_PARENTS)),
        (Some(existing), None) => Some(PlanParent::Existing(existing.parse()?)),
        (None, Some(new)) => Some(PlanParent::New(parent_fields(new)?)),
        (None, None) => None,
    };

    let refs: HashSet<String> = input.tasks.iter().map(|task| task.r#ref.clone()).collect();
    let tasks = input
        .tasks
        .into_iter()
        .map(|task| sub_task(task, &refs))
        .collect::<McpResult<Vec<_>>>()?;

    let plan = PlanInput {
        parent,
        tasks,
        discovered_from: input
            .discovered_from
            .as_ref()
            .map(TaskArg::parse)
            .transpose()?,
        // From the bearer token's session, never from an argument
        // (`ARCHITECTURE.md`, "MCP design" → Authentication).
        session_id: ctx.session_id,
    };

    validate_plan(&plan).map_err(McpError::from)?;

    Ok(plan)
}

/// A new parent's fields, checked.
fn parent_fields(input: PlanParentInput) -> McpResult<PlanTaskFields> {
    check_fields(&input.title, input.priority, input.labels.as_deref())?;

    Ok(PlanTaskFields {
        title: input.title,
        description: input.description,
        state: input.state,
        priority: input.priority,
        labels: input.labels.unwrap_or_default(),
        depends_on: input
            .depends_on
            .iter()
            .flatten()
            .map(TaskArg::parse)
            .collect::<McpResult<Vec<_>>>()?,
    })
}

/// One sub-task, checked, with each `depends_on` entry read against the
/// plan's refs.
///
/// A string that is one of the refs is that sub-task: a ref can never also be
/// a task reference ([`is_valid_ref`](crate::tracker::plan::is_valid_ref)),
/// so the reading is never ambiguous. Anything else must be a task reference,
/// and an entry that is neither gets the one message that names both.
fn sub_task(input: PlanTaskInput, refs: &HashSet<String>) -> McpResult<PlanTask> {
    check_fields(&input.title, input.priority, input.labels.as_deref())?;

    let mut depends_on = Vec::new();
    for entry in input.depends_on.into_iter().flatten() {
        depends_on.push(match entry {
            TaskArg::Text(text) if refs.contains(&text) => PlanDependency::Local(text),
            TaskArg::Text(text) => match TaskArg::Text(text.clone()).parse() {
                Ok(reference) => PlanDependency::Task(reference),
                Err(_) => return Err(McpError::from(unknown_ref(&text))),
            },
            number @ TaskArg::Number(_) => PlanDependency::Task(number.parse()?),
        });
    }

    Ok(PlanTask {
        local_ref: input.r#ref,
        fields: PlanTaskFields {
            title: input.title,
            description: input.description,
            state: input.state,
            priority: input.priority,
            labels: input.labels.unwrap_or_default(),
            depends_on: Vec::new(),
        },
        depends_on,
    })
}

/// The title, the priority and the labels, through the model functions the
/// creation itself uses, so the message is the one it would have given under
/// the lock.
fn check_fields(title: &str, priority: Option<i16>, labels: Option<&[String]>) -> McpResult<()> {
    TaskTitle::parse(title).map_err(Error::from)?;

    if let Some(priority) = priority {
        validate_priority(priority)?;
    }

    if let Some(labels) = labels {
        Label::parse_list(labels).map_err(Error::from)?;
    }

    Ok(())
}
