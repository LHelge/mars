//! Mars orchestrator binary.
//!
//! The startup order is the one in `ARCHITECTURE.md`, "Orchestrator
//! internals" and "Restart procedure": configuration, tracing, pool,
//! migrations, then the recovery and background work each epic adds, then the
//! two listeners. Nothing here is conditional — a step that fails is fatal and
//! the process exits rather than serving in a half-built state.
//!
//! The serving itself lives in the library (`mars_orchestrator::run`), so the
//! integration tests exercise the same router and the same shutdown path.

use std::time::Duration;

use futures_util::FutureExt;
use mars_orchestrator::email::{EmailClient, LogEmailClient, ResendClient};
use mars_orchestrator::engine::{ContainerEngine, PlaceholderEngine};
use mars_orchestrator::git::{
    CommitIdentity, GitCredentialProvider, PlaceholderCredentialProvider,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::secrets;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};

/// Connections in the pool. The orchestrator's own work is short queries plus
/// the session owners' event appends; 20 leaves headroom for both without
/// crowding a small Postgres.
const POOL_MAX_CONNECTIONS: u32 = 20;

/// How long a caller waits for a pooled connection before giving up.
const POOL_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// The floor for the drain deadline, so a `STOP_GRACE_SECS` of 0 still lets
/// in-flight requests finish.
const MIN_DRAIN_GRACE: Duration = Duration::from_secs(1);

#[tokio::main]
async fn main() {
    // Before tracing exists, so this one message goes to stderr by hand.
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    };

    if let Err(err) = init_tracing(&config.rust_log) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }

    // Before the pool, because a master key that cannot be parsed is a
    // configuration fault and nothing in the process can recover from it. The
    // error names the offending version, never the value (rule 3).
    let keyring = match secrets::load_keyring(&config) {
        Ok(keyring) => keyring,
        Err(err) => {
            error!(error = %err, "the secrets master keys could not be loaded");
            std::process::exit(1);
        }
    };
    info!(
        key_versions = ?keyring.versions(),
        current_key_version = keyring.current_version(),
        "master keyring loaded"
    );

    // The URL is a credential; the database is identified by nothing at all
    // here, and by its host only once sqlx reports a failure (rule 3).
    let pool = match PgPoolOptions::new()
        .max_connections(POOL_MAX_CONNECTIONS)
        .acquire_timeout(POOL_ACQUIRE_TIMEOUT)
        .connect(&config.database_url)
        .await
    {
        Ok(pool) => pool,
        Err(err) => {
            // No retry loop: under compose the orchestrator depends on a
            // healthy Postgres, and outside it a restart is the right answer.
            error!(error = %err, "database connection failed");
            std::process::exit(1);
        }
    };
    info!(max_connections = POOL_MAX_CONNECTIONS, "database connected");

    // A signal that arrives here is not acted on until the listeners exist, so
    // an interrupted start finishes its migration rather than abandoning it.
    if let Err(err) = sqlx::migrate!("./migrations").run(&pool).await {
        error!(error = %err, "migrations failed");
        std::process::exit(1);
    }
    info!("migrations applied");

    // Container engine epic: ensure networks, startup probe
    // Session lifecycle epic: adopt running containers, fail sessions in creating
    // Background jobs epic: CronService::start

    let api_port = config.api_port;
    let mcp_port = config.mcp_port;
    let drain_grace = Duration::from_secs(config.stop_grace_secs).max(MIN_DRAIN_GRACE);

    // Both on all interfaces: nginx reaches the API over the compose network
    // and session containers reach MCP over `mars-sessions`, which has no
    // gateway of its own (`ARCHITECTURE.md`, "Networks"). Bound before
    // serving so a port clash fails now rather than half-started.
    let api = bind(api_port).await;
    let mcp = bind(mcp_port).await;
    info!(api_port, mcp_port, "listening");

    // The remaining collaborators have no production implementation yet: the
    // container engine and git operations epics each replace their placeholder
    // with the real thing behind the same trait.
    let engine: Arc<dyn ContainerEngine> = Arc::new(PlaceholderEngine);
    let email: Arc<dyn EmailClient> = select_email_client(&config);
    let git_credentials: Arc<dyn GitCredentialProvider> =
        Arc::new(PlaceholderCredentialProvider::new(CommitIdentity {
            name: config.git_bot_name.clone(),
            email: config.git_bot_email.clone(),
        }));

    let state = AppState::new(
        Arc::new(config),
        pool,
        engine,
        email,
        git_credentials,
        keyring,
    );

    // One signal future, watched twice: once by the listeners, which start
    // draining, and once by the deadline, which gives up on a request that
    // will not finish.
    let signal = shutdown_signal().shared();
    let deadline = {
        let signal = signal.clone();
        async move {
            signal.await;
            tokio::time::sleep(drain_grace).await;
        }
    };

    tokio::select! {
        result = mars_orchestrator::run(state, api, mcp, signal) => {
            if let Err(err) = result {
                error!(error = %err, "orchestrator stopped with an error");
                std::process::exit(1);
            }
        }
        () = deadline => {
            warn!(
                grace_secs = drain_grace.as_secs(),
                "requests still in flight at the drain deadline; exiting anyway"
            );
        }
    }

    info!("orchestrator stopped");
}

/// Pick the delivery path for outgoing mail, or exit.
///
/// `RESEND_API_KEY` set means real delivery; unset means the whole message,
/// link included, goes to this log instead, which is what local development
/// uses (`README.md`, "Configuration", ADR 0014, ADR 0026). The key itself is
/// never logged (rule 3).
fn select_email_client(config: &Config) -> Arc<dyn EmailClient> {
    let Some(api_key) = config.resend_api_key.clone() else {
        info!(
            "RESEND_API_KEY is unset; invitation and reset links are written to this log (ADR 0026)"
        );
        return Arc::new(LogEmailClient::new());
    };

    // `Config::from_env` already refuses a key without a sender; this is the
    // same rule stated where the client is built, so neither can drift.
    let Some(from) = config.mail_from.clone() else {
        error!("missing required configuration: MAIL_FROM (required when RESEND_API_KEY is set)");
        std::process::exit(1);
    };

    info!(mail_from = %from, "email is delivered through Resend");
    Arc::new(ResendClient::new(api_key, from))
}

/// Bind one listener on all interfaces, or exit.
async fn bind(port: u16) -> TcpListener {
    match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(listener) => listener,
        Err(err) => {
            error!(port, error = %err, "could not bind listener");
            std::process::exit(1);
        }
    }
}

/// Resolve on `SIGINT` or `SIGTERM`.
///
/// `SIGTERM` is what an engine sends when it stops the container and what
/// compose sends on `down`; `SIGINT` is Ctrl-C during development. Both mean
/// the same thing here, so `cargo run` and a container stop take one path.
async fn shutdown_signal() {
    // Registering a handler fails only if the process cannot install one at
    // all, which is a broken environment, not a running state to recover from.
    let mut interrupt = signal(SignalKind::interrupt()).expect("SIGINT handler installs");
    let mut terminate = signal(SignalKind::terminate()).expect("SIGTERM handler installs");

    let received = tokio::select! {
        _ = interrupt.recv() => "SIGINT",
        _ = terminate.recv() => "SIGTERM",
    };

    info!(signal = received, "shutdown signal received");
}
