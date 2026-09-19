//! The `mars-orchestrator healthcheck` subcommand (`README.md`, "Deployment
//! shape"; `SPEC.md`, "Health").
//!
//! The probe itself is unit-tested in `src/healthcheck.rs`. What only the
//! process can answer is asserted here: that the subcommand needs none of the
//! configuration the server needs, that it says nothing at all on a healthy
//! answer, and that the exit code compose switches on is the one it hands back.
//!
//! No database and no container engine: the endpoint the child talks to is an
//! `axum` route this test serves on loopback, which is the whole surface the
//! subcommand touches.
//!
//! The child's environment is cleared and rebuilt with `API_PORT` alone —
//! that is the point of the first test — and its working directory is a
//! temporary one so no developer's `.env` can reach it.

use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use tokio::net::TcpListener;

/// How long a run of the binary is given before the test gives up on it.
///
/// The subcommand's own ceiling is 5 s and it starts no listener, so a healthy
/// run finishes in milliseconds; this limit is for a loaded machine, and it
/// exists to catch a *hang* — a binary that fell through to the server path
/// would sit on its listeners until the whole suite timed out.
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// What the binary said and how it ended.
struct ProcessResult {
    code: i32,
    stdout: String,
    stderr: String,
}

impl From<Output> for ProcessResult {
    fn from(output: Output) -> Self {
        Self {
            code: output.status.code().expect("the child exited normally"),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

/// Serve one fixed status on `/api/health` and hand back the port it listens
/// on. The server stops when the returned handle is aborted.
async fn serve_health(status: StatusCode) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("binds");
    let port = listener.local_addr().expect("has an address").port();
    let app = Router::new().route("/api/health", get(move || async move { status }));
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (port, handle)
}

/// A port nothing listens on: bound to get one the kernel handed out, then
/// released, so the connection is refused rather than left hanging.
async fn closed_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("binds");
    let port = listener.local_addr().expect("has an address").port();
    drop(listener);

    port
}

/// Spawn `mars-orchestrator healthcheck` with `API_PORT` and nothing else.
///
/// `env_clear` is the assertion as much as the isolation: the child is handed
/// no `DATABASE_URL`, no `SECRETS_MASTER_KEYS` and no `RUST_LOG`, so a run that
/// succeeds proves the subcommand builds no configuration and initialises no
/// tracing.
async fn run(api_port: &str, working_dir: &Path) -> ProcessResult {
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mars-orchestrator"))
        .arg("healthcheck")
        .current_dir(working_dir)
        .env_clear()
        .env("API_PORT", api_port)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the orchestrator binary starts");

    tokio::time::timeout(RUN_TIMEOUT, child.wait_with_output())
        .await
        .expect("the healthcheck subcommand exits on its own")
        .expect("the child is collected")
        .into()
}

#[tokio::test]
async fn healthcheck_exits_zero_and_says_nothing_when_the_api_is_healthy() {
    let working_dir = tempfile::tempdir().expect("a temporary directory is created");
    let (port, server) = serve_health(StatusCode::OK).await;

    let result = run(&port.to_string(), working_dir.path()).await;

    assert_eq!(result.code, 0, "stderr: {}", result.stderr);
    assert!(result.stdout.is_empty(), "stdout: {}", result.stdout);
    assert!(result.stderr.is_empty(), "stderr: {}", result.stderr);

    server.abort();
}

#[tokio::test]
async fn healthcheck_exits_one_when_the_api_reports_itself_unhealthy() {
    let working_dir = tempfile::tempdir().expect("a temporary directory is created");
    let (port, server) = serve_health(StatusCode::SERVICE_UNAVAILABLE).await;

    let result = run(&port.to_string(), working_dir.path()).await;

    assert_eq!(result.code, 1, "stderr: {}", result.stderr);
    assert_eq!(
        result.stderr.trim_end_matches('\n'),
        "healthcheck: status 503"
    );
    assert!(result.stdout.is_empty(), "stdout: {}", result.stdout);

    server.abort();
}

#[tokio::test]
async fn healthcheck_exits_one_when_nothing_listens() {
    let working_dir = tempfile::tempdir().expect("a temporary directory is created");
    let port = closed_port().await;

    let result = run(&port.to_string(), working_dir.path()).await;

    assert_eq!(result.code, 1, "stderr: {}", result.stderr);
    assert_eq!(
        result.stderr.trim_end_matches('\n'),
        "healthcheck: connection refused"
    );
    assert!(result.stdout.is_empty(), "stdout: {}", result.stdout);
}

#[tokio::test]
async fn healthcheck_exits_one_on_an_unusable_api_port() {
    let working_dir = tempfile::tempdir().expect("a temporary directory is created");

    let result = run("not-a-port", working_dir.path()).await;

    assert_eq!(result.code, 1, "stderr: {}", result.stderr);
    assert_eq!(
        result.stderr.trim_end_matches('\n'),
        "healthcheck: invalid API_PORT"
    );
    assert!(result.stdout.is_empty(), "stdout: {}", result.stdout);
}
