//! Mars orchestrator binary.
//!
//! The startup order is the one in `ARCHITECTURE.md`, "Orchestrator
//! internals" and "Restart procedure": configuration, tracing, keyring, pool,
//! migrations, keyring verification, the container engine (socket, session
//! networks, startup probe), then the recovery and background work each epic
//! adds, then the two listeners. Nothing here is conditional — a step that
//! fails is fatal and the process exits rather than serving in a half-built
//! state, and every engine step is above the binds so a host with a broken
//! engine never answers on a port at all.
//!
//! Everything up to and including the keyring verification is [`bootstrap`],
//! because the `rotate-secrets` subcommand needs exactly that much and nothing
//! after it (`ARCHITECTURE.md`, "Secrets", Rotation). Sharing the function is
//! what keeps the two paths from drifting: a step added to the startup order is
//! a step the subcommand runs too, or it is below the split and deliberately
//! server-only.
//!
//! `healthcheck` is the exception to that sharing: it runs none of the
//! bootstrap, because all it does is ask the already-running orchestrator's own
//! API how it is (`mars_orchestrator::healthcheck`).
//!
//! The serving itself lives in the library (`mars_orchestrator::run`), so the
//! integration tests exercise the same router and the same shutdown path.

mod cli;

use std::process::ExitCode;
use std::time::Duration;

use futures_util::FutureExt;
use mars_orchestrator::cron::CronService;
use mars_orchestrator::email::{EmailClient, LogEmailClient, ResendClient};
use mars_orchestrator::engine::{EngineError, bootstrap_engine};
use mars_orchestrator::git::{
    CommitIdentity, GitCommand, GitCredentialProvider, PatCredentialProvider,
};
use mars_orchestrator::prelude::*;
use mars_orchestrator::secrets;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Connections in the pool. The orchestrator's own work is short queries plus
/// the session owners' event appends; 20 leaves headroom for both without
/// crowding a small Postgres.
const POOL_MAX_CONNECTIONS: u32 = 20;

/// How long a caller waits for a pooled connection before giving up.
const POOL_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// The floor for the drain deadline, so a `STOP_GRACE_SECS` of 0 still lets
/// in-flight requests finish.
const MIN_DRAIN_GRACE: Duration = Duration::from_secs(1);

/// What every fatal start-up fault exits with, on both paths.
const EXIT_FAILURE: i32 = 1;

/// `EX_USAGE` from `sysexits.h`: the arguments were wrong, so nothing was
/// attempted — not even reading the configuration.
const EXIT_USAGE: i32 = 64;

/// The one line an unrecognised argument gets. No argument parser stands behind
/// it (`cli`), so this is written out rather than generated.
const USAGE: &str = "usage: mars-orchestrator [healthcheck|rotate-secrets]";

/// What [`bootstrap`] built: everything both paths need and nothing either one
/// has to build for itself.
struct Bootstrap {
    config: Config,
    pool: PgPool,
    keyring: secrets::SecretsKeyring,
}

#[tokio::main]
async fn main() -> ExitCode {
    // The dispatch is before the configuration on purpose: a mistyped argument
    // is answered the same way on a host with no `.env` at all, and an operator
    // who wanted `rotate-secrets` never starts a server by accident.
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    match arguments.as_slice() {
        [] => {
            serve(bootstrap().await).await;
            ExitCode::SUCCESS
        }
        // No bootstrap at all on this arm: the probe needs one variable and
        // one request, and a health check that failed on an unrelated missing
        // variable would report the wrong thing (`crate::healthcheck`).
        [subcommand] if subcommand == "healthcheck" => cli::healthcheck().await,
        [subcommand] if subcommand == "rotate-secrets" => {
            cli::rotate_secrets(bootstrap().await).await
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(EXIT_USAGE);
        }
    }
}

/// Configuration, tracing, the `git` probe, keyring, pool, migrations, keyring
/// verification.
///
/// The shared prefix of both paths, in the documented order. The keyring is
/// built before the pool because a master key that cannot be parsed is a
/// configuration fault, and verified after the migrations because the table has
/// to exist first.
///
/// Every failure here is fatal and exits [`EXIT_FAILURE`] rather than returning
/// an error: there is no half-configured state either path could serve or sweep
/// from, and the caller has nothing to add to the message.
async fn bootstrap() -> Bootstrap {
    // Before tracing exists, so this one message goes to stderr by hand.
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(EXIT_FAILURE);
        }
    };

    if let Err(err) = init_tracing(&config.rust_log) {
        eprintln!("error: {err}");
        std::process::exit(EXIT_FAILURE);
    }

    // Every git operation shells out to the binary (ADR 0011), so a missing or
    // unusable `git` is a broken installation, not something to discover at the
    // first clone. The version is pinned in the orchestrator image.
    match GitCommand::new().arg("--version").run_ok().await {
        Ok(output) => info!(git_version = %output.stdout.trim(), "git binary found"),
        Err(err) => {
            error!(error = %err, "`git` could not be run; install git and put it on PATH");
            std::process::exit(EXIT_FAILURE);
        }
    }

    // Before the pool, because a master key that cannot be parsed is a
    // configuration fault and nothing in the process can recover from it. The
    // error names the offending version, never the value (rule 3).
    let keyring = match secrets::load_keyring(&config) {
        Ok(keyring) => keyring,
        Err(err) => {
            error!(error = %err, "the secrets master keys could not be loaded");
            std::process::exit(EXIT_FAILURE);
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
            std::process::exit(EXIT_FAILURE);
        }
    };
    info!(max_connections = POOL_MAX_CONNECTIONS, "database connected");

    // A signal that arrives here is not acted on until the listeners exist, so
    // an interrupted start finishes its migration rather than abandoning it.
    if let Err(err) = sqlx::migrate!("./migrations").run(&pool).await {
        error!(error = %err, "migrations failed");
        std::process::exit(EXIT_FAILURE);
    }
    info!("migrations applied");

    // After the migrations, because the table has to exist, and before
    // anything serves: a master key that is missing or wrong would otherwise
    // only be discovered at a session launch (`ARCHITECTURE.md`, "Secrets",
    // Keyring). The error lists every offending version and no key material
    // (rule 3).
    if let Err(err) = secrets::verify_keyring_at_startup(&pool, &keyring).await {
        error!(error = %err, "the stored secrets cannot be read with the configured master keys");
        std::process::exit(EXIT_FAILURE);
    }
    info!("stored secrets verified against the master keyring");

    Bootstrap {
        config,
        pool,
        keyring,
    }
}

/// Serve the API and MCP until a signal, then drain and exit.
///
/// Everything below the bootstrap split: the work and the listeners that only a
/// running orchestrator has. `rotate-secrets` reaches none of it.
async fn serve(bootstrap: Bootstrap) {
    let Bootstrap {
        config,
        pool,
        keyring,
    } = bootstrap;

    // Above the binds, and only on this path: `rotate-secrets` sweeps the
    // database and needs no engine at all, so the shared bootstrap stops at
    // the keyring verification.
    let engine = match bootstrap_engine(&config).await {
        Ok(engine) => engine,
        Err(err) => {
            match &err {
                // The engine ran the probe container and the result was wrong:
                // the message names the uid pair and the fix for this engine
                // (`ARCHITECTURE.md`, "Engine adapter", Startup probe).
                EngineError::Probe(_) => {
                    error!(reason = %err, "startup probe failed; refusing to start")
                }
                // The one line that names the socket, so an operator whose user
                // cannot read it can fix the permissions. It is a startup log
                // and never an answer to a caller (rule 3).
                EngineError::Connection(_) => error!(
                    reason = %err,
                    docker_host = %config.docker_host,
                    "the container engine is unreachable; refusing to start"
                ),
                _ => error!(reason = %err, "the container engine refused a startup step"),
            }
            std::process::exit(EXIT_FAILURE);
        }
    };

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

    // The v1 provider (ADR 0002): the project's `GIT_CREDENTIAL` secret,
    // decrypted and audited on every read, as an `Authorization: Basic`
    // header. It shares the pool and the keyring with the rest of the process;
    // cloning the keyring shares its keys rather than copying them.
    let email: Arc<dyn EmailClient> = select_email_client(&config);
    let git_credentials: Arc<dyn GitCredentialProvider> = Arc::new(PatCredentialProvider::new(
        pool.clone(),
        keyring.clone(),
        CommitIdentity {
            name: config.git_bot_name.clone(),
            email: config.git_bot_email.clone(),
        },
    ));

    let state = AppState::new(
        Arc::new(config),
        pool,
        engine,
        email,
        git_credentials,
        keyring,
    );

    // The tracker's side of a session ending: every path that makes a session
    // `done` or `failed` releases the tasks it held
    // (`ARCHITECTURE.md`, "Task tracker" → "Liveness comes from the session,
    // not from tool calls"). Installed before recovery runs, so a session
    // recovery fails at startup releases its leases too.
    let state = {
        let hook = mars_orchestrator::tracker::session_ended_hook(&state);
        state.with_session_ended_hook(hook)
    };

    // Step 2 and 3 of the restart procedure, and before either listener accepts
    // a request: an owner adopted here must be in the registry before a launch
    // or a resume can race it (`ARCHITECTURE.md`, "Restart procedure"). The
    // engine's startup probe is already done, so nothing is adopted through an
    // engine that has not been verified. A listing that fails is fatal: an
    // orchestrator that does not know which containers are running would park
    // sessions whose CLI is alive. The cron jobs start right after this, which
    // is step 4.
    match mars_orchestrator::session::recover(&state).await {
        Ok(report) => info!(
            adopted = report.adopted,
            parked = report.parked,
            failed = report.failed,
            "startup recovery done",
        ),
        Err(err) => {
            error!(error = %err, "startup recovery failed; refusing to serve");
            std::process::exit(EXIT_FAILURE);
        }
    }

    // Step 4 of the restart procedure: the cron jobs, after recovery and
    // before either listener accepts a request, so the first sweep sees the
    // state recovery left rather than one a request has already changed
    // (`ARCHITECTURE.md`, "Restart procedure", "Background jobs"). Every job
    // runs once here and then at its own interval.
    let (cron_shutdown, cron_shutdown_rx) = watch::channel(false);
    let cron_jobs = Arc::new(CronService::new(state.clone())).start(cron_shutdown_rx);

    // One signal future, watched three times: by the listeners, which start
    // draining, by the deadline, which gives up on a request that will not
    // finish, and by the cron loops, which take a `watch` rather than a future
    // because each of the six holds its own end of it.
    let signal = shutdown_signal().shared();
    tokio::spawn({
        let signal = signal.clone();
        async move {
            signal.await;
            // The receivers outlive this task; a send that finds none is the
            // loops having already stopped, which is the state it wanted.
            let _ = cron_shutdown.send(true);
        }
    });
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

    // After the listeners, on the same grace: a job mid-tick is a transaction
    // that is better committed than abandoned, and the six loops stop between
    // ticks, so the usual wait here is the tail of one job.
    await_cron_jobs(cron_jobs, drain_grace).await;

    info!("orchestrator stopped");
}

/// Wait for the cron loops to stop, for at most `grace`.
///
/// Dropping a handle only detaches its task, so the timeout is what the
/// process actually exits on; the `warn!` is there so an operator can tell a
/// slow job from a hung one.
async fn await_cron_jobs(jobs: Vec<JoinHandle<()>>, grace: Duration) {
    let drained = async {
        for job in jobs {
            // A loop that panicked has already been logged by whatever
            // unwound it; there is nothing to do here but not hide it.
            if let Err(err) = job.await {
                error!(error = %err, "a cron loop did not stop cleanly");
            }
        }
    };

    if tokio::time::timeout(grace, drained).await.is_err() {
        warn!(
            grace_secs = grace.as_secs(),
            "background jobs still running at the drain deadline; exiting anyway"
        );
    }
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
