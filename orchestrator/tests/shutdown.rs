//! `run` serves both listeners and stops on one signal.
//!
//! Nothing here needs Postgres or a container engine: the pool is lazy, so the
//! health endpoint answers 503 and the shutdown path is what is under test.

use std::time::Duration;

use mars_orchestrator::cron::{CronService, JobName};
use mars_orchestrator::email::LogEmailClient;
use mars_orchestrator::engine::PlaceholderEngine;
use mars_orchestrator::git::{CommitIdentity, PatCredentialProvider};
use mars_orchestrator::prelude::*;
use mars_orchestrator::run;
use mars_orchestrator::secrets::{MASTER_KEY_LEN, SecretsKeyring};
use sqlx::postgres::PgPoolOptions;
use std::collections::HashMap;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};

/// How long a statement against the unreachable database waits before it
/// fails. Short, because every millisecond of it is time the shutdown test
/// spends waiting for a tick it already asked to be the last one.
const UNREACHABLE_AFTER: Duration = Duration::from_millis(200);

/// Obviously fake values; nothing listens on port 1, so the health probe
/// reports `database: false` when its own timeout fires (rule 3).
fn test_state() -> AppState {
    let vars: HashMap<&str, &str> = [
        ("PUBLIC_URL", "https://mars.example.invalid"),
        ("JWT_SECRET", "not-a-real-signing-secret"),
        ("DATABASE_URL", "postgres://invalid:invalid@127.0.0.1:1/x"),
        ("DOCKER_HOST", "unix:///run/user/1000/podman/podman.sock"),
        ("DATA_DIR_HOST", "/srv/mars/data"),
        ("SECRETS_MASTER_KEYS", "1=not-a-real-key"),
        ("GIT_BOT_NAME", "Mars Bot"),
        ("GIT_BOT_EMAIL", "mars-bot@example.invalid"),
        ("SESSION_IMAGE_DEFAULT", "mars-session-claude:dev"),
    ]
    .into_iter()
    .collect();

    let config = Config::from_vars(|name| vars.get(name).map(|value| value.to_string()))
        .expect("a complete required set loads");
    // A job body that reaches for this pool has to give up quickly rather than
    // sit out sqlx's 30-second default: the cron test below signals shutdown
    // while the first tick is still running, and the loop only stops once that
    // tick has finished (`src/cron/scheduler.rs`).
    let pool = PgPoolOptions::new()
        .acquire_timeout(UNREACHABLE_AFTER)
        .connect_lazy(&config.database_url)
        .expect("a lazy pool never connects");

    let identity = CommitIdentity {
        name: config.git_bot_name.clone(),
        email: config.git_bot_email.clone(),
    };

    let keyring = SecretsKeyring::from_entries(vec![(1, [0u8; MASTER_KEY_LEN])])
        .expect("one entry is a valid keyring");

    AppState::new(
        Arc::new(config),
        pool.clone(),
        Arc::new(PlaceholderEngine),
        Arc::new(LogEmailClient),
        Arc::new(PatCredentialProvider::new(pool, keyring.clone(), identity)),
        keyring,
    )
}

#[tokio::test]
async fn both_listeners_serve_and_stop_on_one_shutdown_signal() {
    let api = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the api listener binds");
    let mcp = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the mcp listener binds");

    let api_addr = api.local_addr().expect("the api listener has an address");
    let mcp_addr = mcp.local_addr().expect("the mcp listener has an address");

    let (tx, rx) = oneshot::channel::<()>();
    let server = tokio::spawn(run(test_state(), api, mcp, async move {
        rx.await.ok();
    }));

    let client = reqwest::Client::new();

    // The API listener serves the router: an unreachable database is 503, not
    // a connection failure.
    let health = client
        .get(format!("http://{api_addr}/api/health"))
        .send()
        .await
        .expect("the api listener answers");
    assert_eq!(health.status().as_u16(), 503);
    assert_eq!(
        health.json::<serde_json::Value>().await.unwrap(),
        serde_json::json!({ "orchestrator": true, "database": false, "engine": true })
    );

    // The MCP listener serves its own router: `/mcp` and nothing else, so any
    // other path is the ordinary 404 (`ARCHITECTURE.md`, "MCP design").
    let unknown = client
        .get(format!("http://{mcp_addr}/anything"))
        .send()
        .await
        .expect("the mcp listener answers");
    assert_eq!(unknown.status().as_u16(), 404);

    tx.send(()).expect("the server is still running");

    let result = tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("both listeners stop within the deadline")
        .expect("the server task does not panic");

    assert!(result.is_ok(), "run returned an error: {result:?}");
}

/// The cron loops stop on the same signal as the listeners, and stop fast.
///
/// `main` waits for the job handles after `run` returns and bounds that wait
/// with `STOP_GRACE_SECS` (`ARCHITECTURE.md`, "Restart procedure"); this is
/// the unbounded version of that wait, so a loop that only noticed the signal
/// at its next tick would hang here instead of being papered over by the
/// grace. What is under test is the loop; a job body that runs here reaches
/// the unreachable database of [`test_state`] and gives up within
/// [`UNREACHABLE_AFTER`], so no body can decide this deadline either.
#[tokio::test]
async fn the_cron_jobs_stop_on_the_same_signal_as_the_listeners() {
    let api = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the api listener binds");
    let mcp = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the mcp listener binds");

    let state = test_state();
    let (cron_shutdown, cron_shutdown_rx) = watch::channel(false);
    let jobs = Arc::new(CronService::new(state.clone())).start(cron_shutdown_rx);
    // One loop per job, and the dispatcher's waker beside its loop
    // (`ARCHITECTURE.md`, "Dispatcher"): every handle `start` returns is
    // waited for below, so the waker stops on this signal too.
    assert_eq!(
        jobs.len(),
        JobName::ALL.len() + 1,
        "one loop per job, plus the dispatcher's waker"
    );

    let api_addr = api.local_addr().expect("the api listener has an address");
    let (tx, rx) = oneshot::channel::<()>();
    let server = tokio::spawn(run(state, api, mcp, async move {
        rx.await.ok();
    }));

    // One request first, so the deadline below covers the stopping and not the
    // starting: `run` opens the shared Postgres listener before it serves, and
    // against the unreachable database in `test_state` that connection attempt
    // is the slowest thing in this file.
    reqwest::get(format!("http://{api_addr}/api/health"))
        .await
        .expect("the api listener answers");

    tx.send(()).expect("the server is still running");
    cron_shutdown.send(true).expect("the loops are listening");

    let stopped = async {
        server
            .await
            .expect("the server task does not panic")
            .expect("run returns cleanly");
        for job in jobs {
            job.await.expect("a cron loop does not panic");
        }
    };

    tokio::time::timeout(Duration::from_secs(2), stopped)
        .await
        .expect("the listeners and the cron loops stop within the deadline");
}
