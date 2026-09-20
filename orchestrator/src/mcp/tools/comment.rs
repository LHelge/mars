//! `comment`: leave a comment on any task in the project (`SPEC.md`, "MCP tool
//! contracts" → `comment`).
//!
//! "Any session in the project may comment on any task." So there is no holder
//! check here and none in [`add_comment`]: a comment is how a session that
//! noticed something tells whoever picks the task up next, and requiring a
//! lease would silence exactly the sessions with something to say.
//!
//! The change is still a change, so it commits like any other: the row, the
//! `commented` event and the `task_sessions` link together, and nothing at all
//! when the body is blank or the task is not this project's.

use crate::mcp::tools::common::{begin_mutation, finish, resolve_task_for_mutation};
use crate::mcp::tools::{CommentInput, CommentOutput, non_empty};
use crate::mcp::{McpResult, SessionContext};
use crate::prelude::*;
use crate::tracker::{CommentAuthor, CommentDto, TrackerMutation, add_comment};

/// `{ task, body }` → `{ comment: Comment }`.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: CommentInput,
) -> McpResult<CommentOutput> {
    // The model would refuse it too, as 400; refusing here keeps the lock out
    // of a call that was never going to write anything.
    non_empty("body", &input.body)?;

    let mut mutation = begin_mutation(state, ctx).await?;
    let written = comment(&mut mutation, ctx, &input).await;
    let comment = finish(state, mutation, written).await?;

    Ok(CommentOutput { comment })
}

/// The comment itself, inside the open mutation.
///
/// The link comes from [`add_comment`], which touches the mutation's actor —
/// the calling session — so a comment on a task this session never claimed
/// still records that it worked on it (ADR 0030).
async fn comment(
    m: &mut TrackerMutation<'_>,
    ctx: &SessionContext,
    input: &CommentInput,
) -> McpResult<CommentDto> {
    let task = resolve_task_for_mutation(m, ctx, &input.task).await?;

    Ok(add_comment(
        m,
        &task,
        CommentAuthor::Session(ctx.session_id),
        &input.body,
    )
    .await?)
}
