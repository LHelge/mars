//! Profile-aware `tools/list` and `tools/call` (`ARCHITECTURE.md`, "MCP
//! design" → Tool exposure; `SPEC.md`, "MCP tool contracts").
//!
//! Two rules are pinned here, over a real `rmcp` client on a real listener.
//! **Listing** answers the eight task tools to everyone and a git tool only to
//! a profile that names it in `mcp_tools`, in the document's own order and with
//! its verbatim descriptions. **Calling** refuses an unknown name as the
//! caller's mistake, an unlisted git tool as a refusal, and arguments that do
//! not fit the tool's input type as `invalid_argument` — each with the
//! documented message and a `data.code` the agent can branch on.
//!
//! Needs a container engine (`DOCKER_HOST`); see `tests/common/db.rs`.

#![cfg(feature = "integration-tests")]

mod common;

use common::TestApp;
use common::mcp::McpClient;
use rmcp::model::ErrorData;
use serde_json::{Value, json};

use mars_orchestrator::mcp::ToolName;
use mars_orchestrator::models::SessionState;

/// A client connected as a live session of a profile with exactly these
/// `mcp_tools`.
async fn client_with_tools(app: &TestApp, mcp_tools: &[&str]) -> McpClient {
    let (project_id, _) = app.seed_mcp_project().await;
    let profile_id = app.seed_mcp_profile(project_id, mcp_tools).await;
    let seeded = app
        .seed_mcp_session(project_id, profile_id, SessionState::Running)
        .await;

    McpClient::connect(app, &seeded.token)
        .await
        .expect("a running session's token authenticates")
}

/// The `data.code` every tool failure carries (`SPEC.md`, "MCP tool
/// contracts").
fn code(err: &ErrorData) -> String {
    err.data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("every tool error carries data.code: {err:?}"))
        .to_string()
}

/// The names `tools/list` answered, in the order it answered them.
async fn listed(client: &McpClient) -> Vec<String> {
    client
        .list_tools()
        .await
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect()
}

#[tokio::test]
async fn a_profile_that_names_no_tool_is_listed_the_eight_task_tools() {
    let app = TestApp::spawn().await;
    let client = client_with_tools(&app, &[]).await;

    assert_eq!(
        listed(&client).await,
        [
            "ready",
            "claim",
            "get_task",
            "update",
            "release",
            "comment",
            "needs_human",
            "create_task",
        ],
    );
}

#[tokio::test]
async fn every_listed_tool_carries_its_verbatim_description_and_an_object_schema() {
    let app = TestApp::spawn().await;
    let client = client_with_tools(&app, &[]).await;

    for tool in client.list_tools().await {
        let name = ToolName::parse(&tool.name).expect("a listed name is a known tool");

        assert_eq!(tool.description.as_deref(), Some(name.description()));
        assert_eq!(tool.input_schema.get("type"), Some(&json!("object")));
        assert!(tool.output_schema.is_some(), "{}", name.as_str());
    }
}

#[tokio::test]
async fn two_named_git_tools_are_listed_after_the_task_tools() {
    let app = TestApp::spawn().await;
    let client = client_with_tools(&app, &["merge", "push"]).await;

    let names = listed(&client).await;

    assert_eq!(names.len(), 10, "{names:?}");
    assert_eq!(&names[8..], ["merge", "push"]);
}

#[tokio::test]
async fn a_profile_that_names_every_git_tool_is_listed_all_twelve() {
    let app = TestApp::spawn().await;
    let client =
        client_with_tools(&app, &["list_session_branches", "merge", "rebase", "push"]).await;

    assert_eq!(
        listed(&client).await,
        ToolName::ALL.map(ToolName::as_str).to_vec(),
    );
}

#[tokio::test]
async fn calling_a_git_tool_the_profile_does_not_name_is_forbidden() {
    let app = TestApp::spawn().await;
    let client = client_with_tools(&app, &["merge"]).await;

    let err = client
        .call("rebase", json!({ "branch": "main", "onto": "origin/main" }))
        .await
        .expect_err("rebase is not in this profile");

    assert_eq!(code(&err), "forbidden");
    assert_eq!(err.message, "tool rebase is not allowed for this profile");
}

#[tokio::test]
async fn calling_a_tool_that_does_not_exist_is_an_invalid_argument() {
    let app = TestApp::spawn().await;
    let client = client_with_tools(&app, &[]).await;

    let err = client
        .call("no_such_tool", json!({}))
        .await
        .expect_err("no such tool");

    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(err.message, "unknown tool: no_such_tool");
}

#[tokio::test]
async fn an_argument_of_the_wrong_type_is_an_invalid_argument_with_serdes_reason() {
    let app = TestApp::spawn().await;
    let client = client_with_tools(&app, &[]).await;

    let err = client
        .call("ready", json!({ "limit": "twenty" }))
        .await
        .expect_err("a limit is a number");

    assert_eq!(code(&err), "invalid_argument");
    assert!(
        err.message.starts_with("invalid arguments:"),
        "{}",
        err.message,
    );
}

#[tokio::test]
async fn a_malformed_task_argument_is_answered_with_the_documented_sentence() {
    let app = TestApp::spawn().await;
    let client = client_with_tools(&app, &[]).await;

    // `12.5` fits neither arm of the untagged task argument, so `serde` fails
    // the whole object; the dispatcher answers that with the same sentence a
    // task reference it could parse but not understand would get.
    let err = client
        .call("claim", json!({ "task": 12.5 }))
        .await
        .expect_err("a fractional number is not a task");

    assert_eq!(code(&err), "invalid_argument");
    assert_eq!(
        err.message,
        r##"task must be a UUID, a number, or "#<number>""##,
    );
}
