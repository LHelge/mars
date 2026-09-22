//! MCP conformance against the pinned Claude Code CLI (`CLAUDE.md`, "Testing
//! expectations", MCP conformance fixtures).
//!
//! The other MCP suites talk to the server through `rmcp`'s own client over
//! loopback, which sends a loopback `Host`, speaks whatever `rmcp` speaks and
//! validates nothing strictly. Two bugs reached the live stack through that
//! gap: `pacy2`, every session refused with 403 by the transport's `Host`
//! check, and `4gd2v`, a `tools/list` the CLI dropped whole for lacking
//! `ttlMs` and `cacheScope`. This suite closes it from the CLI's side:
//!
//! - `tests/fixtures/mcp/<cli version>/` is what a real CLI of that version
//!   sent the Mars MCP server, one request per file, recorded by
//!   `scripts/mcp-record/record.sh` (`images/claude/VERIFY.md`, "Recording the
//!   MCP conformance fixtures");
//! - each request is replayed over real HTTP against `mcp_router` with a
//!   seeded session's token and the **recorded** `Host`, the `MCP_URL`
//!   authority a session container uses;
//! - each answer is checked against what that CLI version accepts, which for
//!   protocol revision 2026-07-28 is `common::mcp_2026_07_28`.
//!
//! A CLI version bump records a new directory and never edits an old one; the
//! old directories keep being replayed for as long as they are here. An `rmcp`
//! bump runs this suite first: both bugs were library defaults.
//!
//! Needs a container engine for the database (`tests/common/db.rs`), and
//! neither `DOCKER_HOST` nor the CLI image nor any network beyond loopback.

#![cfg(feature = "integration-tests")]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::http::Uri;
use common::TestApp;
use common::mcp_2026_07_28 as revision;
use common::mcp_fixtures::{self, Exchange, Recorder};
use mars_orchestrator::mcp::{INSTRUCTIONS, mcp_router};
use mars_orchestrator::models::SessionState;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// The Claude Code version the session image pins
/// (`images/claude/Dockerfile`, `CLAUDE_CODE_VERSION`).
fn pinned_cli_version() -> String {
    let dockerfile = Path::new(env!("CARGO_MANIFEST_DIR")).join("../images/claude/Dockerfile");
    let text = std::fs::read_to_string(&dockerfile).expect("the claude image's Dockerfile");
    text.lines()
        .find_map(|line| line.strip_prefix("ARG CLAUDE_CODE_VERSION="))
        .expect("the Dockerfile pins CLAUDE_CODE_VERSION")
        .trim()
        .to_string()
}

/// Every recorded CLI version, oldest name first.
fn recorded_versions() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp");
    let mut versions: Vec<String> = std::fs::read_dir(&root)
        .expect("tests/fixtures/mcp is readable")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.is_dir())
        .map(|path| {
            path.file_name()
                .expect("a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    versions.sort();
    versions
}

/// The `MCP_URL` authority the test configuration shares with compose's
/// default, which is what a recorded `Host` has to be.
fn mcp_url_authority(app: &TestApp) -> String {
    let uri: Uri = app.state.config.mcp_url.parse().expect("MCP_URL is a URL");
    uri.authority().expect("MCP_URL names a host").to_string()
}

/// The MCP router on an ephemeral loopback port, with one running session to
/// authenticate as.
struct Replay {
    app: TestApp,
    addr: std::net::SocketAddr,
    server: JoinHandle<()>,
    http: reqwest::Client,
    bearer: String,
}

impl Drop for Replay {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// What the server answered to one replayed request.
struct Answer {
    status: u16,
    /// The JSON-RPC message answering the request, when it expects one and the
    /// status says there is a body to read it from.
    message: Option<Value>,
}

impl Replay {
    async fn start() -> Replay {
        let app = TestApp::spawn().await;
        let (project_id, profile_id) = app.seed_mcp_project().await;
        let seeded = app
            .seed_mcp_session(project_id, profile_id, SessionState::Running)
            .await;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral loopback port is free");
        let addr = listener.local_addr().expect("a bound address");
        let router = mcp_router(app.state.clone());
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        Replay {
            app,
            addr,
            server,
            http: reqwest::Client::new(),
            bearer: format!("Bearer {}", seeded.token),
        }
    }

    /// Send `exchange` as recorded — method, path, headers including `Host`,
    /// body — with this replay's own bearer token.
    async fn send(&self, exchange: &Exchange) -> Answer {
        assert!(
            !exchange.headers.contains_key("mcp-session-id"),
            "a stateful recording: substitute this replay's own session id before replaying it"
        );
        let method = reqwest::Method::from_bytes(exchange.method.as_bytes())
            .expect("a recorded HTTP method");
        let mut request = self
            .http
            .request(method, format!("http://{}{}", self.addr, exchange.path))
            .header("authorization", &self.bearer);
        for (name, value) in &exchange.headers {
            request = request.header(name, value);
        }
        if let Some(body) = &exchange.body {
            request = request.body(body.to_string());
        }

        let response = tokio::time::timeout(Duration::from_secs(30), request.send())
            .await
            .expect("the server answers within 30 s")
            .expect("the mcp listener answers");
        let status = response.status().as_u16();
        if !exchange.expects_answer() {
            return Answer {
                status,
                message: None,
            };
        }

        let id = exchange
            .body
            .as_ref()
            .and_then(|body| body.get("id"))
            .cloned();
        let text = tokio::time::timeout(Duration::from_secs(30), response.text())
            .await
            .expect("the answer completes within 30 s")
            .expect("a readable body");
        Answer {
            status,
            message: answer_to(&text, id.as_ref()),
        }
    }
}

/// The JSON-RPC answer with `id` in a body that is either one JSON message or
/// a Server-Sent Events stream of them, as the transport chooses; `None` for
/// a body that is neither, such as the bearer middleware's own error body.
fn answer_to(body: &str, id: Option<&Value>) -> Option<Value> {
    let is_answer = |message: &Value| {
        message.get("jsonrpc").is_some()
            && message.get("id") == id
            && message.get("method").is_none()
    };
    if let Ok(message) = serde_json::from_str::<Value>(body) {
        return is_answer(&message).then_some(message);
    }
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
        .find(is_answer)
}

/// The checks for the revision a request negotiated.
fn check(exchange: &Exchange, answer: &Value) -> Vec<String> {
    let version = exchange
        .headers
        .get("mcp-protocol-version")
        .map(String::as_str);
    match version {
        Some(revision::PROTOCOL_VERSION) => {
            revision::check_answer(exchange.body.as_ref().expect("a body"), answer)
        }
        other => panic!(
            "no validator for protocol revision {other:?}: add a module beside \
             common::mcp_2026_07_28 from that revision's schema"
        ),
    }
}

/// The recorded `ready` call: the one successful `tools/call` of the pinned
/// version's conversation.
fn recorded_call(version: &str, tool: &str) -> Exchange {
    mcp_fixtures::load(&mcp_fixtures::version_dir(version))
        .into_iter()
        .map(|(_, exchange)| exchange)
        .find(|exchange| {
            exchange.rpc_method() == Some("tools/call")
                && exchange.headers.get("mcp-name").map(String::as_str) == Some(tool)
        })
        .unwrap_or_else(|| panic!("{version} recorded a call of {tool}"))
}

#[test]
fn the_pinned_cli_version_has_been_recorded() {
    let version = pinned_cli_version();

    assert!(
        mcp_fixtures::version_dir(&version).is_dir(),
        "no MCP fixtures for Claude Code {version}: record them with \
         scripts/mcp-record/record.sh (images/claude/VERIFY.md)"
    );
}

#[test]
fn the_fixtures_carry_no_credential() {
    for version in recorded_versions() {
        for (name, exchange) in mcp_fixtures::load(&mcp_fixtures::version_dir(&version)) {
            assert!(
                !exchange.headers.contains_key("authorization"),
                "{version}/{name} keeps an Authorization header"
            );
            let text = serde_json::to_string(&exchange).expect("an exchange serialises");
            assert!(
                !text.contains("Bearer") && !text.contains("sk-ant-"),
                "{version}/{name} carries a credential-shaped value"
            );
        }
    }
}

#[tokio::test]
async fn every_recorded_request_is_answered_as_the_cli_expects() {
    for version in recorded_versions() {
        let replay = Replay::start().await;
        let authority = mcp_url_authority(&replay.app);

        for (name, exchange) in mcp_fixtures::load(&mcp_fixtures::version_dir(&version)) {
            // The point of replaying the recorded `Host` is that it is not
            // loopback: it is the name a session container dials (`pacy2`).
            assert_eq!(
                exchange.headers.get("host"),
                Some(&authority),
                "{version}/{name} was recorded against MCP_URL's authority"
            );

            let answer = replay.send(&exchange).await;

            assert_eq!(
                answer.status, exchange.status,
                "{version}/{name}: HTTP status"
            );
            if exchange.expects_answer() {
                let message = answer
                    .message
                    .unwrap_or_else(|| panic!("{version}/{name}: no JSON-RPC answer"));
                let problems = check(&exchange, &message);
                assert!(
                    problems.is_empty(),
                    "{version}/{name}: the CLI would refuse this answer: {problems:#?}\n{message}"
                );
            }
        }
    }
}

/// The CLI's SDK drops the whole listing when one tool fails its schema, so
/// every tool is held to it here by name.
#[tokio::test]
async fn every_listed_tool_has_a_valid_name_and_object_schemas() {
    let version = pinned_cli_version();
    let listing = mcp_fixtures::load(&mcp_fixtures::version_dir(&version))
        .into_iter()
        .map(|(_, exchange)| exchange)
        .find(|exchange| exchange.rpc_method() == Some("tools/list"))
        .expect("the pinned version recorded a tools/list");
    let replay = Replay::start().await;

    let answer = replay.send(&listing).await;

    let message = answer.message.expect("a tools/list answer");
    let tools = message["result"]["tools"]
        .as_array()
        .expect("a list of tools");
    // The seeded profile names no git tool, so this is the task-tracker set.
    assert_eq!(tools.len(), 8, "{message}");
    for tool in tools {
        let name = tool["name"].as_str().expect("a tool name");
        assert!(revision::is_valid_tool_name(name), "{name}");
        for key in ["inputSchema", "outputSchema"] {
            assert!(tool[key].is_object(), "{name}: {key} is {}", tool[key]);
            assert_eq!(tool[key]["type"], "object", "{name}: {key}");
        }
    }
}

/// The CLI of this revision never sends `initialize`, so the discovery probe
/// is where it learns what this server is for (`ARCHITECTURE.md`, "MCP
/// design", Instructions).
#[tokio::test]
async fn the_discovery_answer_carries_the_instructions() {
    let discover = mcp_fixtures::load(&mcp_fixtures::version_dir(&pinned_cli_version()))
        .into_iter()
        .map(|(_, exchange)| exchange)
        .find(|exchange| exchange.rpc_method() == Some("server/discover"))
        .expect("the pinned version recorded a server/discover");
    let replay = Replay::start().await;

    let answer = replay.send(&discover).await;

    let message = answer.message.expect("a server/discover answer");
    assert_eq!(message["result"]["instructions"], INSTRUCTIONS, "{message}");
}

/// A tool the CLI was never offered is one it never calls: it answers the
/// model itself with "No such tool available" (observed on 2.1.274, see
/// `images/claude/VERIFY.md`), so no recording can contain the request. The
/// server's answer to it still has to be an error the CLI can read, so the
/// recorded `ready` call is replayed with only the name changed. The status is
/// not pinned: `rmcp` answers this invalid-params error with 400 on this
/// revision where a tool's own failure is a 200, and no CLI behaviour has been
/// observed that would prefer one over the other.
#[tokio::test]
async fn a_call_of_a_tool_the_listing_never_offered_is_a_readable_error() {
    let mut exchange = recorded_call(&pinned_cli_version(), "ready");
    exchange
        .headers
        .insert("mcp-name".to_string(), "no_such_tool".to_string());
    exchange.body.as_mut().expect("a body")["params"]["name"] = "no_such_tool".into();
    let replay = Replay::start().await;

    let answer = replay.send(&exchange).await;

    let message = answer.message.expect("a JSON-RPC answer");
    assert!(message.get("error").is_some(), "{message}");
    let problems = check(&exchange, &message);
    assert!(problems.is_empty(), "{message}: {problems:#?}");
}

#[test]
fn the_validator_refuses_a_listing_without_the_cache_fields() {
    let request = serde_json::json!({ "jsonrpc": "2.0", "id": 0, "method": "tools/list" });
    let answer = serde_json::json!({ "jsonrpc": "2.0", "id": 0, "result": {
        "resultType": "complete",
        "tools": [{ "name": "ready", "inputSchema": { "type": "object" } }],
    }});

    let problems = revision::check_answer(&request, &answer);

    assert_eq!(problems.len(), 2, "{problems:?}");
}

#[test]
fn the_validator_refuses_a_bad_tool_name_and_a_non_object_schema() {
    let mut problems = Vec::new();

    revision::check_tool(
        &serde_json::json!({ "name": "ready", "inputSchema": { "type": "string" } }),
        &mut problems,
    );
    revision::check_tool(
        &serde_json::json!({ "name": "has space", "inputSchema": { "type": "object" } }),
        &mut problems,
    );

    assert_eq!(problems.len(), 2, "{problems:?}");
}

/// Serve the MCP router behind a [`Recorder`] for a real CLI to talk to, and
/// write what it sent as a new fixture directory.
///
/// Opt-in: without `MARS_MCP_RECORD` (the directory to write) it prints one
/// line and passes, which is how every ordinary run sees it.
/// `scripts/mcp-record/record.sh` sets it, together with
/// `MARS_MCP_RECORD_CONFIG` (where to write the session's `mcp.json`, next to
/// which the script creates `done` once the CLI has exited, or `abort` when
/// the run failed and nothing is to be written) and optionally
/// `MARS_MCP_RECORD_PORT` (default 7001, the port of `MCP_URL`).
#[tokio::test]
async fn record() {
    let Some(out) = std::env::var_os("MARS_MCP_RECORD").map(PathBuf::from) else {
        println!("MARS_MCP_RECORD is unset; not recording MCP fixtures");
        return;
    };
    let config = PathBuf::from(
        std::env::var_os("MARS_MCP_RECORD_CONFIG").expect("MARS_MCP_RECORD_CONFIG is set"),
    );
    let port: u16 = std::env::var("MARS_MCP_RECORD_PORT")
        .map(|port| port.parse().expect("a port number"))
        .unwrap_or(7001);

    let app = TestApp::spawn().await;
    let (project_id, profile_id) = app.seed_mcp_project().await;
    let seeded = app
        .seed_mcp_session(project_id, profile_id, SessionState::Running)
        .await;

    let recorder = Recorder::default();
    let router = recorder.wrap(mcp_router(app.state.clone()));
    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .expect("the recording port is free");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    // The document the launcher writes (`ARCHITECTURE.md`, "MCP design" →
    // "Per-session config file"), with this app's `MCP_URL` and the seeded
    // session's token: a test token that exists only for this run.
    let document = serde_json::json!({
        "mcpServers": { "mars-orchestrator": {
            "type": "http",
            "url": app.state.config.mcp_url,
            "headers": { "Authorization": format!("Bearer {}", seeded.token) },
        }}
    });
    std::fs::write(&config, document.to_string()).expect("the mcp.json is written");
    println!(
        "recorder: serving {} on port {port}",
        app.state.config.mcp_url
    );

    let done = config.with_file_name("done");
    let abort = config.with_file_name("abort");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(900);
    while !done.exists() {
        assert!(!abort.exists(), "the recording was aborted");
        assert!(
            tokio::time::Instant::now() < deadline,
            "the CLI did not finish"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    server.abort();

    assert!(recorder.len() > 0, "the CLI sent nothing");
    recorder.write(&out);
    println!(
        "recorder: wrote {} requests to {}",
        recorder.len(),
        out.display()
    );
}
