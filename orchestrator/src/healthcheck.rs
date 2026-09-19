//! The `healthcheck` subcommand's probe.
//!
//! The orchestrator image carries the binary and `git` and nothing else
//! (`ARCHITECTURE.md`, "Trust boundaries"), so a compose `healthcheck:` has no
//! `curl` and no `wget` to call. The binary answers that itself: it asks its
//! own API for `GET /api/health` (`SPEC.md`, "Health") over the loopback
//! interface and turns the answer into an exit code.
//!
//! What it deliberately does not do is bootstrap. No pool, no migrations, no
//! engine, no keyring — and no tracing, so `RUST_LOG` can say anything without
//! putting a line in the health log. The one output is a `healthcheck:
//! <reason>` line on stderr when the probe fails; a healthy answer prints
//! nothing at all. It also means the probe needs one variable, `API_PORT`,
//! rather than a whole [`Config`]: a health check that failed because
//! `SECRETS_MASTER_KEYS` was unset in its environment would report the
//! orchestrator unhealthy for a reason that has nothing to do with it.

use std::process::ExitCode;
use std::time::Duration;

use crate::prelude::config::API_PORT_DEFAULT;
use crate::prelude::*;

/// How long the connection itself may take. Short: the peer is this same
/// container's loopback listener, so anything slower is already a fault.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// The ceiling for the whole request, connection included. `/api/health`
/// touches the database and the engine, so it is not instantaneous, but it is
/// well under this.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(5);

/// Why the probe did not see a healthy orchestrator.
///
/// `Display` is the text after `healthcheck: ` on stderr — one short line an
/// operator reads in `podman ps` or the compose health log.
#[derive(Debug, thiserror::Error)]
pub enum HealthcheckError {
    /// `API_PORT` is set to something that is not a port number.
    #[error("invalid API_PORT")]
    InvalidApiPort,
    /// The client could not be built, which means the TLS or DNS backend
    /// failed to initialise.
    #[error("client setup failed: {0}")]
    Client(String),
    /// Nothing accepted the connection: the API is not listening yet, or not
    /// any more.
    #[error("connection refused")]
    ConnectionRefused,
    /// The connection failed for some other reason.
    #[error("connection failed: {0}")]
    Connection(String),
    /// Neither the connection nor the response arrived inside the timeouts.
    #[error("timed out")]
    Timeout,
    /// The request failed in some other way.
    #[error("request failed: {0}")]
    Request(String),
    /// The API answered, but not with 200 — a 503 means the database or the
    /// engine is down (`SPEC.md`, "Health"), which is exactly the state
    /// compose should see as unhealthy.
    #[error("status {0}")]
    Status(u16),
}

/// Resolve the port the probe talks to, or report why it could not.
///
/// The same sources and the same default as [`Config::from_env`] — `.env`,
/// then the process environment, then [`API_PORT_DEFAULT`] — and nothing else
/// from the configuration, so an environment missing any other required
/// variable still gets a health check.
///
/// The error case is already reported when it returns, so the caller only
/// forwards the code.
pub fn api_port_from_env() -> std::result::Result<u16, ExitCode> {
    Config::load_dotenv();

    match std::env::var("API_PORT") {
        Err(_) => Ok(API_PORT_DEFAULT),
        Ok(value) => value.trim().parse().map_err(|_| {
            // The value is an operator's own port number, not a secret, but
            // the reason line stays a fixed string: it is read by whoever can
            // also read the variable.
            report(&HealthcheckError::InvalidApiPort)
        }),
    }
}

/// Probe `GET http://127.0.0.1:<api_port>/api/health` and turn it into an exit
/// code.
///
/// Success is exactly HTTP 200 and nothing is printed. Every other outcome
/// prints one `healthcheck: <reason>` line on stderr and fails, so compose
/// marks the service unhealthy and anything with
/// `depends_on: condition: service_healthy` waits.
pub async fn run_healthcheck(api_port: u16) -> ExitCode {
    match probe(api_port).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => report(&err),
    }
}

/// The probe itself, without the printing, so the unit tests below assert the
/// outcome rather than a process exit code (`ExitCode` cannot be compared).
async fn probe(api_port: u16) -> std::result::Result<(), HealthcheckError> {
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        // A health endpoint that redirects is not a healthy one; following it
        // would also let the answer come from somewhere else entirely.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| HealthcheckError::Client(err.to_string()))?;

    // The listener binds `0.0.0.0`, so loopback reaches it from inside the
    // same container and from nowhere else.
    let url = format!("http://127.0.0.1:{api_port}/api/health");

    // The body is not read: the status is the contract (`SPEC.md`, "Health"),
    // and the flags in the body are for a human looking at the endpoint.
    let response = client.get(&url).send().await.map_err(classify)?;

    let status = response.status().as_u16();
    if status == 200 {
        Ok(())
    } else {
        Err(HealthcheckError::Status(status))
    }
}

/// Print the one stderr line and hand back the failing code.
fn report(err: &HealthcheckError) -> ExitCode {
    eprintln!("healthcheck: {err}");
    ExitCode::FAILURE
}

/// Turn a `reqwest` failure into the reason an operator needs: refused,
/// timed out, or something else with its own text.
fn classify(err: reqwest::Error) -> HealthcheckError {
    if err.is_timeout() {
        return HealthcheckError::Timeout;
    }

    if err.is_connect() {
        return match io_kind(&err) {
            Some(std::io::ErrorKind::ConnectionRefused) => HealthcheckError::ConnectionRefused,
            _ => HealthcheckError::Connection(err.to_string()),
        };
    }

    HealthcheckError::Request(err.to_string())
}

/// The `io::ErrorKind` behind a transport failure, if there is one: `reqwest`
/// wraps it a couple of layers down and exposes no accessor for it.
fn io_kind(err: &reqwest::Error) -> Option<std::io::ErrorKind> {
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(err);
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            return Some(io.kind());
        }
        source = current.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::get;
    use tokio::net::TcpListener;

    use super::*;

    /// Serve one fixed status on `/api/health` and hand back the port. The
    /// server is dropped with the returned handle at the end of the test.
    async fn serve_health(status: StatusCode) -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("binds");
        let port = listener.local_addr().expect("has an address").port();
        let app = Router::new().route("/api/health", get(move || async move { status }));
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (port, handle)
    }

    #[tokio::test]
    async fn ok_when_the_api_answers_200() {
        let (port, server) = serve_health(StatusCode::OK).await;

        assert!(probe(port).await.is_ok());

        server.abort();
    }

    #[tokio::test]
    async fn fails_when_the_api_answers_503() {
        let (port, server) = serve_health(StatusCode::SERVICE_UNAVAILABLE).await;

        let err = probe(port).await.expect_err("503 is not healthy");
        assert!(matches!(err, HealthcheckError::Status(503)), "{err}");
        assert_eq!(err.to_string(), "status 503");

        server.abort();
    }

    #[tokio::test]
    async fn fails_when_nothing_listens() {
        // Bind and drop: the port was free a moment ago and nothing took it,
        // so the connection is refused rather than left hanging.
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("binds");
        let port = listener.local_addr().expect("has an address").port();
        drop(listener);

        let err = tokio::time::timeout(TOTAL_TIMEOUT * 2, probe(port))
            .await
            .expect("the probe gives up on its own")
            .expect_err("a closed port is not healthy");
        assert!(matches!(err, HealthcheckError::ConnectionRefused), "{err}");
    }

    #[tokio::test]
    async fn success_prints_nothing_and_exits_zero() {
        let (port, server) = serve_health(StatusCode::OK).await;

        // `ExitCode` has no `PartialEq`; `Debug` is what distinguishes the
        // two values without depending on an unstable accessor.
        let code = run_healthcheck(port).await;
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));

        server.abort();
    }
}
