//! One throw-away Postgres per test *process*, one throw-away database per
//! test.
//!
//! Every repository test, the migration round-trip test and
//! `TestApp::spawn()` build on this. The first test in a binary to ask for a
//! pool starts `postgres:18` in a container and applies the crate's
//! migrations once, to a database called `mars_template`; every test then gets
//! `CREATE DATABASE test_<uuid> TEMPLATE mars_template` on that same server,
//! which costs milliseconds instead of the seconds a container start costs.
//! Tests in one binary still run in parallel and still share nothing: a
//! database of one's own isolates rows, sequences, `LISTEN`/`NOTIFY` channels
//! and advisory locks alike, because a PostgreSQL advisory lock tag carries
//! the database oid.
//!
//! Starting a container needs a reachable engine socket. The harness does not
//! configure one: it relies on the default socket resolution of
//! `testcontainers`, which reads `DOCKER_HOST` and then the usual Docker and
//! Podman socket paths. CI runners provide Docker; on a development machine
//! export `DOCKER_HOST` at the Podman socket first (`README.md`,
//! "Development").
//!
//! # Runtimes
//!
//! Each `#[tokio::test]` has a runtime of its own that is dropped when the
//! test ends, so nothing that outlives a test may be bound to one: a `PgPool`
//! whose reaper task died with its runtime hands out connections registered
//! with a terminated reactor. So the shared state here holds no pool. The
//! container is started on a leaked multi-threaded runtime of its own, from a
//! scoped thread outside any runtime (`Runtime::block_on` panics inside
//! another runtime), and what the `OnceLock` keeps afterwards is a URL and a
//! handle. Every database is created and dropped over a single connection
//! opened and closed on whichever runtime asked for it.
//!
//! # Reaping the container
//!
//! `testcontainers` 0.27 removes a container in `ContainerAsync::drop`, and
//! the Rust crate ships no Ryuk reaper to fall back on (unlike the Java and Go
//! libraries, so `TESTCONTAINERS_RYUK_*` does nothing here). Drop is no use
//! either: the container guard lives in a `static` for the whole process, and
//! the test harness ends with `std::process::exit`, which runs no
//! destructors. The container is therefore removed from a `libc::atexit`
//! hook — `atexit` handlers *do* run on `process::exit` — which opens a
//! `bollard` client on the same socket `testcontainers` used and forces the
//! container away. A run that is killed outright (`SIGKILL`) leaks its
//! container, which is the one case no in-process mechanism can cover.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::OnceLock;

use bollard::query_parameters::RemoveContainerOptionsBuilder;
use bollard::{API_DEFAULT_VERSION, Docker};
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection, PgPool};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use tokio::runtime::Handle;
use uuid::Uuid;

/// The same `Migrator` `main.rs` runs at startup, embedded at compile time
/// from the crate's `migrations/` directory, so tests never drift from the
/// schema the binary applies.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The pinned server version. Bumping it is a deliberate change: the schema in
/// `docs/data-model.md` is written against this major version.
pub const POSTGRES_TAG: &str = "18";

/// Enough connections for a test that talks to the database one query at a
/// time. Tests that fan out (concurrent event appends) ask for more through
/// the `_with` variants.
pub const DEFAULT_MAX_CONNECTIONS: u32 = 5;

/// The server arguments the container runs with.
///
/// `fsync=off` is what the `testcontainers` Postgres module passes by default
/// and is repeated here because `ImageExt::with_cmd` replaces the image's
/// command rather than extending it. `max_connections` is raised from the
/// built-in 100 because one server now carries every test in a binary at once:
/// sixteen parallel tests at [`DEFAULT_MAX_CONNECTIONS`] fit inside 100, but
/// the fan-out tests ask for more and each database creation opens one
/// connection of its own.
const SERVER_ARGS: [&str; 4] = ["-c", "fsync=off", "-c", "max_connections=300"];

/// The database the migrations are applied to once and every test database is
/// cloned from.
const TEMPLATE_DATABASE: &str = "mars_template";

/// The maintenance database `CREATE DATABASE` and `DROP DATABASE` are issued
/// against. The image creates it; nothing in Mars uses it.
const ADMIN_DATABASE: &str = "postgres";

/// Serialises `CREATE DATABASE` process-wide.
///
/// PostgreSQL takes only a `ShareLock` on the source database, so concurrent
/// clones of one template are legal, but a test harness that flakes is worse
/// than one that is a few hundred milliseconds slower, and the whole point of
/// the template is that a clone is cheap. Held on a session against
/// [`ADMIN_DATABASE`], so it can never meet a lock a test takes: an advisory
/// lock tag carries the database oid, and no test runs in the maintenance
/// database. Distinct from `ADMIN_MEMBERSHIP_LOCK_KEY` all the same, so a
/// `pg_locks` dump is never ambiguous.
const CREATE_DATABASE_LOCK_KEY: i64 = 0x4D41_5253_5445_5354;

/// Names a Postgres server the suite should use instead of starting its own:
/// `postgres://user:password@host:port`, with no database name.
///
/// For a run whose engine cannot give `testcontainers` a published port — the
/// Engine workflow's rootless Podman 4 answers `PortNotExposed` — and which
/// therefore brings a server of its own (a CI service container). The server
/// must be this run's alone: the template database is dropped and rebuilt on
/// it, and no container is started or removed. Unset, which is every local run
/// and Orchestrator CI, the suite starts its own server as the module
/// documentation describes.
pub const EXTERNAL_SERVER_ENV: &str = "MARS_TEST_POSTGRES_URL";

/// How long the removal hook's single engine request may take, in seconds.
const REMOVE_TIMEOUT_SECS: u64 = 30;

/// The process-wide server every test database is created on.
struct SharedPostgres {
    /// `postgres://postgres:postgres@host:port`, with no database name: a
    /// test database's URL is this plus `/<name>`.
    ///
    /// The credentials are the image's defaults and exist only for the
    /// lifetime of the container; nothing real is spelled out here (rule 3).
    base_url: String,
    /// The leaked runtime the container was started on, which is also where a
    /// [`TestDatabase`] drop schedules its `DROP DATABASE`: a test's own
    /// runtime is already shutting down by then and would never poll it.
    runtime: Handle,
    /// Never read; it keeps the container guard alive for the whole process so
    /// that `testcontainers` cannot remove the server under a running test.
    /// Removal is the `atexit` hook's job (see the module documentation).
    /// `None` on a server the run brought itself ([`EXTERNAL_SERVER_ENV`]).
    _container: Option<ContainerAsync<Postgres>>,
}

static SHARED: OnceLock<SharedPostgres> = OnceLock::new();

/// The shared server, started on first use.
///
/// Synchronous and blocking on purpose: `OnceLock::get_or_init` makes the
/// first caller pay the container start and every other caller wait for it,
/// which is exactly the arrangement wanted, and each `#[tokio::test]` has a
/// thread of its own to block.
fn shared() -> &'static SharedPostgres {
    SHARED.get_or_init(|| {
        // A scoped thread, because `Runtime::block_on` panics when it is
        // called from inside another runtime and this function is called from
        // a test's own runtime.
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let runtime = Box::leak(Box::new(
                        tokio::runtime::Builder::new_multi_thread()
                            .worker_threads(2)
                            .enable_all()
                            .build()
                            .expect("a runtime for the shared server"),
                    ));

                    runtime.block_on(boot(runtime.handle().clone()))
                })
                .join()
                .expect("the shared server starts")
        })
    })
}

/// Start the container, register its removal and build the template.
async fn boot(runtime: Handle) -> SharedPostgres {
    if let Some(base_url) = external_server() {
        drop_template(&base_url).await;
        create_template(&base_url).await;

        return SharedPostgres {
            base_url,
            runtime,
            _container: None,
        };
    }

    let container = Postgres::default()
        .with_tag(POSTGRES_TAG)
        .with_cmd(SERVER_ARGS)
        .start()
        .await
        .expect("postgres starts");

    register_removal(container.id());

    let base_url = format!(
        "postgres://postgres:postgres@{}:{}",
        container
            .get_host()
            .await
            .expect("the container has a host"),
        container
            .get_host_port_ipv4(5432)
            .await
            .expect("the container publishes 5432"),
    );

    create_template(&base_url).await;

    SharedPostgres {
        base_url,
        runtime,
        _container: Some(container),
    }
}

/// The server [`EXTERNAL_SERVER_ENV`] names, without a trailing slash.
fn external_server() -> Option<String> {
    let url = std::env::var(EXTERNAL_SERVER_ENV).ok()?;
    let url = url.trim().trim_end_matches('/');

    (!url.is_empty()).then(|| url.to_string())
}

/// Remove a template an earlier binary of the same run left on an external
/// server, so that this binary's migrations are the ones its tests clone.
async fn drop_template(base_url: &str) {
    let mut admin = admin_connection(base_url).await;
    execute(
        &mut admin,
        format!(
            "UPDATE pg_database SET datistemplate = false WHERE datname = '{TEMPLATE_DATABASE}'"
        ),
    )
    .await
    .expect("the old template database is unmarked");
    execute(
        &mut admin,
        format!("DROP DATABASE IF EXISTS \"{TEMPLATE_DATABASE}\""),
    )
    .await
    .expect("the old template database is dropped");
    let _ = admin.close().await;
}

/// Create [`TEMPLATE_DATABASE`], migrate it and mark it a template.
///
/// The pool that runs the migrations is closed before the flag goes up:
/// `CREATE DATABASE ... TEMPLATE` refuses a source database another session is
/// connected to, so a lingering idle connection here would fail every test in
/// the binary.
async fn create_template(base_url: &str) {
    let mut admin = admin_connection(base_url).await;
    execute(
        &mut admin,
        format!("CREATE DATABASE \"{TEMPLATE_DATABASE}\""),
    )
    .await
    .expect("the template database is created");
    let _ = admin.close().await;

    let template = PgPoolOptions::new()
        .max_connections(1)
        .connect(&format!("{base_url}/{TEMPLATE_DATABASE}"))
        .await
        .expect("the template pool connects");
    MIGRATOR
        .run(&template)
        .await
        .expect("the migrations apply to the template");
    template.close().await;

    let mut admin = admin_connection(base_url).await;
    execute(
        &mut admin,
        format!(
            "UPDATE pg_database SET datistemplate = true WHERE datname = '{TEMPLATE_DATABASE}'"
        ),
    )
    .await
    .expect("the template database is marked as one");
    let _ = admin.close().await;
}

/// A single connection to [`ADMIN_DATABASE`] on the shared server.
async fn admin_connection(base_url: &str) -> PgConnection {
    PgConnection::connect(&format!("{base_url}/{ADMIN_DATABASE}"))
        .await
        .expect("a connection to the maintenance database")
}

/// Run one statement that takes no arguments.
///
/// `sqlx::raw_sql` rather than `query!`: these are DDL statements built at run
/// time, and `CREATE DATABASE` is not something `.sqlx/` should ever have to
/// carry a cached plan for (`CLAUDE.md`, "Backend conventions"). The only
/// thing ever interpolated into one is a name this module generated itself,
/// which is the assertion `sqlx::AssertSqlSafe` asks for.
async fn execute(connection: &mut PgConnection, statement: String) -> sqlx::Result<()> {
    sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
        .execute(connection)
        .await
        .map(|_| ())
}

/// Create a database on the shared server, optionally cloned from a template.
async fn create_database(template: Option<&str>) -> TestDatabase {
    let shared = shared();
    // 32 hex digits and the prefix: 37 bytes, well inside the 63-byte limit
    // on an identifier.
    let name = format!("test_{}", Uuid::new_v4().simple());

    let mut admin = admin_connection(&shared.base_url).await;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(CREATE_DATABASE_LOCK_KEY)
        .execute(&mut admin)
        .await
        .expect("the create-database lock is taken");

    let statement = match template {
        Some(template) => format!("CREATE DATABASE \"{name}\" TEMPLATE \"{template}\""),
        None => format!("CREATE DATABASE \"{name}\""),
    };
    let created = execute(&mut admin, statement).await;

    // The advisory lock is a session lock, so closing the connection releases
    // it whether the statement succeeded or not.
    let _ = admin.close().await;
    created.expect("the test database is created");

    TestDatabase {
        url: format!("{}/{name}", shared.base_url),
        name,
    }
}

/// One test's database on the shared server.
///
/// What the pool helpers return beside the pool, and what `TestApp` holds: a
/// test has to keep it for as long as it uses the pool, because dropping it
/// takes the database away.
#[derive(Debug)]
pub struct TestDatabase {
    name: String,
    url: String,
}

impl TestDatabase {
    /// The database name, `test_` plus a UUID without hyphens.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The URL of this database, for a `DATABASE_URL` or a second pool.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for TestDatabase {
    /// Drop the database on the shared runtime, best effort.
    ///
    /// `DROP DATABASE` is asynchronous work and `Drop` is not, and the test's
    /// own runtime is being torn down at this point, so the statement is
    /// spawned on the shared server's runtime instead, which lives as long as
    /// the process. `WITH (FORCE)` is what makes it succeed while the test's
    /// pool is still closing its connections.
    ///
    /// Nothing waits for it and nothing asserts on it: a test that panicked
    /// before its guard dropped, or a process that exited before the task ran,
    /// leaves a database behind, and every leftover goes away with the
    /// container.
    fn drop(&mut self) {
        let Some(shared) = SHARED.get() else {
            return;
        };

        let base_url = shared.base_url.clone();
        let name = std::mem::take(&mut self.name);
        shared.runtime.spawn(async move {
            let Ok(mut admin) =
                PgConnection::connect(&format!("{base_url}/{ADMIN_DATABASE}")).await
            else {
                return;
            };
            let _ = execute(
                &mut admin,
                format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"),
            )
            .await;
            let _ = admin.close().await;
        });
    }
}

/// A pool on a fresh database with **no** migrations applied, so the caller
/// controls the migration state itself. The round-trip test is the reason this
/// exists; ordinary tests want [`test_pool`].
pub async fn raw_pool() -> (TestDatabase, PgPool) {
    raw_pool_with(DEFAULT_MAX_CONNECTIONS).await
}

/// [`raw_pool`] with an explicit pool size.
pub async fn raw_pool_with(max_connections: u32) -> (TestDatabase, PgPool) {
    let database = create_database(None).await;
    let pool = connect(&database, max_connections).await;

    (database, pool)
}

/// A pool on a fresh database with the crate's migrations applied.
///
/// The migrations are not run here: the database is a clone of the migrated
/// [`TEMPLATE_DATABASE`], `_sqlx_migrations` rows and all, so `MIGRATOR.run()`
/// on it would find nothing to do.
pub async fn test_pool() -> (TestDatabase, PgPool) {
    test_pool_with(DEFAULT_MAX_CONNECTIONS).await
}

/// [`test_pool`] with an explicit pool size.
pub async fn test_pool_with(max_connections: u32) -> (TestDatabase, PgPool) {
    let database = create_database(Some(TEMPLATE_DATABASE)).await;
    let pool = connect(&database, max_connections).await;

    (database, pool)
}

/// The pool every helper above hands back.
async fn connect(database: &TestDatabase, max_connections: u32) -> PgPool {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(database.url())
        .await
        .expect("the pool connects")
}

/// The container to remove at process exit, and the socket to remove it
/// through.
struct Reaped {
    engine: String,
    id: String,
}

static REAPED: OnceLock<Reaped> = OnceLock::new();

/// Arrange for `id` to be removed when the process exits.
fn register_removal(id: &str) {
    let registration = REAPED.set(Reaped {
        engine: engine_socket(),
        id: id.to_string(),
    });
    if registration.is_err() {
        // Only `boot` calls this and the `OnceLock` around it runs it once, so
        // a second registration would be a bug rather than a race; there is
        // nothing left to do either way.
        return;
    }

    // SAFETY: `atexit` asks for an `extern "C" fn()` that neither takes nor
    // returns anything and does not unwind. `remove_shared_container` is one,
    // and it catches its own panics. The `OnceLock` above makes this the only
    // registration.
    let registered = unsafe { libc::atexit(remove_shared_container) };
    assert_eq!(registered, 0, "the container removal hook registers");
}

/// Force the shared container away. Registered with `atexit`, so it runs on
/// the `std::process::exit` the test harness finishes with.
extern "C" fn remove_shared_container() {
    let Some(reaped) = REAPED.get() else {
        return;
    };

    let removed = std::panic::catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
        // A current-thread runtime, built and finished inside this handler: no
        // thread is spawned and nothing outlives the call, which is as much as
        // an exit handler should ask of the process.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;

        runtime.block_on(async {
            let engine = connect_engine(&reaped.engine).map_err(|error| error.to_string())?;
            let options = RemoveContainerOptionsBuilder::default()
                .force(true)
                .v(true)
                .build();

            engine
                .remove_container(&reaped.id, Some(options))
                .await
                .map_err(|error| error.to_string())
        })
    }));

    match removed {
        Ok(Ok(())) => {}
        Ok(Err(error)) => eprintln!(
            "could not remove the test Postgres container {}: {error}",
            reaped.id
        ),
        Err(_) => eprintln!(
            "removing the test Postgres container {} panicked",
            reaped.id
        ),
    }
}

/// A `bollard` client on the socket the container was started through, opened
/// the same way `BollardEngine::connect` and `tests/common/engine.rs` open
/// one.
fn connect_engine(host: &str) -> Result<Docker, bollard::errors::Error> {
    match host.strip_prefix("unix://") {
        Some(path) => Docker::connect_with_socket(path, REMOVE_TIMEOUT_SECS, API_DEFAULT_VERSION),
        None => Docker::connect_with_http(host, REMOVE_TIMEOUT_SECS, API_DEFAULT_VERSION),
    }
}

/// The engine socket `testcontainers` started the container on.
///
/// Its own resolution is `pub(crate)`, so the order is repeated here:
/// `DOCKER_HOST`, then the default Docker socket, then the rootless
/// alternatives, then the default again as a last guess. A
/// `~/.testcontainers.properties` override is the one case not covered, and
/// the `properties-config` feature that reads it is off.
fn engine_socket() -> String {
    const DEFAULT: &str = "unix:///var/run/docker.sock";

    if let Some(host) = std::env::var("DOCKER_HOST")
        .ok()
        .filter(|host| !host.trim().is_empty())
    {
        return host;
    }

    let candidates = [
        Some(PathBuf::from("/var/run/docker.sock")),
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join(".docker/run/docker.sock")),
        std::env::var_os("HOME").map(|dir| PathBuf::from(dir).join(".docker/run/docker.sock")),
        std::env::var_os("HOME").map(|dir| PathBuf::from(dir).join(".docker/desktop/docker.sock")),
    ];

    candidates
        .into_iter()
        .flatten()
        .find(|path| path.exists())
        .map(|path| format!("unix://{}", path.display()))
        .unwrap_or_else(|| DEFAULT.to_string())
}
