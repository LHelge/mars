//! `merge`: merge into an integration branch (git; profile-gated; `SPEC.md`,
//! "MCP tool contracts" → `merge`).
//!
//! The same two forms the REST endpoint takes, on the same code path (ADR
//! 0007): `{ source }` merges a branch, `{ task_id, handoff_id }` merges the
//! commit an approved hand-off pinned. What this module decides is only what
//! the arguments own — which of the two forms was selected, and that the
//! message is within the limit — and both rules are
//! [`crate::routes::git`]'s, imported rather than restated so a caller never
//! meets two spellings of the same refusal.
//!
//! The one difference from REST is the one `SPEC.md` names: `task_id` also
//! accepts a per-project task number, so it is resolved to a UUID here before
//! the service is called. Everything else — ref kinds, syncing, the project
//! git lock, the hand-off verification under it and the `git` outcome events —
//! belongs to [`GitService`], which is called with [`GitActor::Session`]
//! instead of the route's [`GitActor::User`].
//!
//! No tracker transaction is opened: the task lookup is a plain read, and the
//! merge takes the git lock on its own (`ARCHITECTURE.md`, "Git model" →
//! Serialization).

use uuid::Uuid;

use crate::git::{GitActor, GitService};
use crate::mcp::tools::{CommitOutput, McpMergeInput};
use crate::mcp::{McpError, McpResult, SessionContext};
use crate::models::TaskRef;
use crate::prelude::*;
use crate::repositories::TaskRepository;
use crate::routes::git::{MERGE_FORM, checked_message};

/// What a `task_id` naming no task of this project is told.
const TASK_NOT_FOUND: &str = "task not found";

/// Which of the two documented merge forms the arguments selected.
///
/// The REST route's [`MergeSelection`](crate::routes::git) in all but the
/// task reference, which is a [`TaskRef`] here because an agent may name a
/// task by its number.
#[derive(Debug)]
enum Selection {
    /// `{ source }`: an explicit branch merge that records no task approval.
    Branch(String),
    /// `{ task_id, handoff_id }`: the hand-off's pinned commit, once the
    /// verifier has found it current and approved under the git lock.
    Handoff { task: TaskRef, handoff_id: Uuid },
}

/// The selection, or `invalid_argument` with the documented sentence.
///
/// Every mixture is refused, including a half task form: `{ task_id }` alone
/// names no commit and `{ source, task_id }` asks for two different merges.
fn selection(input: &McpMergeInput) -> McpResult<Selection> {
    match (&input.source, &input.task_id, input.handoff_id) {
        (Some(source), None, None) => Ok(Selection::Branch(source.clone())),
        (None, Some(task), Some(handoff_id)) => Ok(Selection::Handoff {
            task: task.parse()?,
            handoff_id,
        }),
        _ => Err(McpError::invalid_argument(MERGE_FORM)),
    }
}

/// Merge, and answer the commit the integration head now points at.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: McpMergeInput,
) -> McpResult<CommitOutput> {
    let selection = selection(&input)?;
    let message = checked_message(input.message.as_ref())?;
    let actor = GitActor::Session(ctx.session_id);
    let service = GitService::from_state(state);

    let outcome = match selection {
        Selection::Branch(source) => {
            service
                .merge_branch(ctx.project_id, &source, &input.target, message, &actor)
                .await?
        }
        Selection::Handoff { task, handoff_id } => {
            // A plain read, before anything is locked: a task this project
            // does not have is the caller's mistake and changes nothing.
            let task_id = TaskRepository::new(&state.pool)
                .find_task(ctx.project_id, task)
                .await?
                .ok_or_else(|| McpError::not_found(TASK_NOT_FOUND))?
                .id;

            service
                .merge_handoff(
                    ctx.project_id,
                    task_id,
                    handoff_id,
                    &input.target,
                    message,
                    &actor,
                )
                .await?
        }
    };

    Ok(CommitOutput {
        commit: outcome.commit,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::mcp::McpErrorCode;

    fn input(value: serde_json::Value) -> McpMergeInput {
        serde_json::from_value(value).expect("the arguments parse")
    }

    #[test]
    fn exactly_one_of_the_two_forms_is_accepted() {
        let handoff_id = Uuid::from_u128(2);

        assert!(matches!(
            selection(&input(json!({ "target": "main", "source": "origin/main" })))
                .expect("the branch form is accepted"),
            Selection::Branch(source) if source == "origin/main",
        ));

        // A number is a task reference here and nowhere in REST.
        assert!(matches!(
            selection(&input(
                json!({ "target": "main", "task_id": "#12", "handoff_id": handoff_id }),
            ))
            .expect("the task form is accepted"),
            Selection::Handoff { task: TaskRef::Number(12), handoff_id: id } if id == handoff_id,
        ));

        for args in [
            json!({ "target": "main" }),
            json!({ "target": "main", "source": "feature", "task_id": 1 }),
            json!({ "target": "main", "task_id": 1 }),
            json!({ "target": "main", "handoff_id": handoff_id }),
            json!({
                "target": "main",
                "source": "feature",
                "task_id": 1,
                "handoff_id": handoff_id,
            }),
        ] {
            let err = selection(&input(args.clone())).expect_err("the mixture is refused");
            assert_eq!(err.code, McpErrorCode::InvalidArgument, "{args}");
            assert_eq!(err.message, MERGE_FORM, "{args}");
        }
    }

    #[test]
    fn an_over_long_message_is_an_invalid_argument() {
        let long = "m".repeat(10 * 1024 + 1);
        let err = checked_message(Some(&long))
            .map_err(McpError::from)
            .expect_err("one byte past the limit is refused");

        assert_eq!(err.code, McpErrorCode::InvalidArgument);
    }
}
