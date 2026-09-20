//! `push`: publish a mirror branch upstream (git; profile-gated; `SPEC.md`,
//! "MCP tool contracts" → `push`).
//!
//! A thin adapter over [`GitService::push`], attributed to the calling session
//! (ADR 0007). Only an integration head or a session ref may be pushed and a
//! session's lands on `session/<id>` unless `remote_branch` says otherwise;
//! both rules are the service's, and so is the non-fast-forward rejection that
//! becomes `conflict` with every local ref exactly as it was.
//!
//! **`force` is the one thing decided here**, and it is decided by absence:
//! the argument is optional on the wire and missing means `false`, so
//! `--force` is only ever added because the caller wrote the word. "Force
//! pushes are refused unless `force` is true and the profile has `push` in
//! `mcp_tools`" (`SPEC.md`) is those two halves and no third: the profile half
//! is the dispatcher's gate, which has already run by the time a call reaches
//! this module, and a second permission check here would be a second place for
//! the rule to drift.

use crate::git::{GitActor, GitService};
use crate::mcp::tools::{PushInput, PushOutput};
use crate::mcp::{McpResult, SessionContext};
use crate::prelude::*;

/// Push, and answer the upstream branch that moved and the commit it holds.
pub async fn handle(
    state: &AppState,
    ctx: &SessionContext,
    input: PushInput,
) -> McpResult<PushOutput> {
    let outcome = GitService::from_state(state)
        .push(
            ctx.project_id,
            &input.r#ref,
            input.remote_branch.as_deref(),
            forced(&input),
            &GitActor::Session(ctx.session_id),
        )
        .await?;

    Ok(PushOutput {
        remote_branch: outcome.remote_branch,
        commit: outcome.commit,
    })
}

/// Did the caller ask for a force push? Absent means no.
fn forced(input: &PushInput) -> bool {
    input.force.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn input(value: serde_json::Value) -> PushInput {
        serde_json::from_value(value).expect("the arguments parse")
    }

    /// The documented default, and the only way past it: nothing but
    /// `force: true` reaches the service as a force push.
    #[test]
    fn only_an_explicit_true_force_pushes() {
        assert!(!forced(&input(json!({ "ref": "main" }))));
        assert!(!forced(&input(json!({ "ref": "main", "force": false }))));
        assert!(forced(&input(json!({ "ref": "main", "force": true }))));
    }
}
